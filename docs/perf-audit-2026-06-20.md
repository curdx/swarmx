# 性能审计与修复报告 — 2026-06-20

> 一次性、全面的 flockmux 核心性能审计。覆盖前端（React/Vite/xterm）、后端（axum/PTY/tokio）、
> 存储层（SQLite/FTS5）、以及后台循环（wake/reaper/billing/probe/cron）四层。
> **方法论**：4 路并行静态审计 → 亲自读码核实每条论断 → 用真实 5 万行 SQLite 库 `EXPLAIN QUERY PLAN`
> 确定性复现 → 落地修复 → `cargo test` / `npm build+test` / `cargo build --workspace` 全门禁验证。

---

## 0. 结论速览（TL;DR）

- **后端架构是健康的，没有 P0。** 逐一核实：无锁跨 `.await`、无 busy-poll、wake 完全事件驱动、
  reaper 5s 廉价、PTY 用专用 OS 线程而非阻塞池、DB 全走 `spawn_blocking`、广播容量充足。
  背景里担心的三件事（busy-poll / probe 阻塞启动 / cron 每秒扫）**实测都不成立**。
- **真正的性能债在两处，都已修复并验证：**
  1. **前端 P0** — 最近的「无损累积」修复（commit `82ef570`）修对了丢消息，但 `recentMessages` 这个
     派生缓存**漏了上界**，随会话单调增长，且 statusDot 每成员每渲染全扫它 → 成员侧栏 `O(成员 × 会话长度)`。
  2. **存储层 P1** — `messages` 表（只追加、未读 wake 永不 prune）的三类高频查询因缺索引退化成**全表扫描**：
     唤醒/未读（每回合）、静默看门狗（每次 spawn 探测）、用户消息重指派。
- **已修 3 项**（全部带确定性证据）：migration 0025 加 2 个索引、`recentMessages` 加 200 上界、连接池 8→16。
- **诚实放弃 1 项**：Shell `ctx` 的 `useMemo`——位于条件 early-return 之后会是非法 hook，且收益接近零。
- **记录但暂不改的 P1/P2 一批**：见 §5，均附「为何暂不改」的风险/收益判断。

---

## 1. 审计方法与范围

| 层 | 范围 | 排除 |
|---|---|---|
| 前端 | `web/src`（React/Vite/xterm.js） | `node_modules` `dist` `target` `.claude/worktrees` |
| 后端 | `crates/flockmux-{server,pty,shim}` | 同上 |
| 存储 | `crates/flockmux-{storage,swarm}`（SQLite+FTS5） | 同上 |
| 后台循环 | `flockmux-server` 的 wake/reaper/billing/probe/cron | 同上 |

每条论断都**亲自读码核实**，不信 agent 转述。涉及 SQL 的用真实数据 `EXPLAIN QUERY PLAN` + `.timer` 复现。

复现库构造（`/tmp/perf_repro.db`）：模拟长会话 **5 万条消息 / 20 个 agent / 5 个 thread**，
1/50 未读、1/11 是 wake、1/7 来自 user —— 贴近真实「跑久了」的 swarm 库形态。

---

## 2. 已修复问题（含复现证据）

### FIX-1 [存储 P1] messages 表热点查询全表扫描 → migration 0025 加索引

**问题机制。** `messages` 表是**只追加**的，且 prune 只删「已读+已投递」的行，**未读 wake 永不被 prune**，
所以它随会话单调增长。0001 只建了 `(to_agent, delivered_at)` 和 `(to_agent, id)` 两个索引。以下三类
高频查询因此退化：

| 查询 | 触发频率 | 缺的列 | 修复前 | 修复后 |
|---|---|---|---|---|
| `count_unread` / `consume_wakes` / `mark_read`<br>`WHERE to_agent=? AND read_at IS NULL` | **每 agent 每回合**（Stop hook → wake-check） | `read_at` | 走 `to_agent` 索引后**扫该 agent 全部历史**（复现库 2500 行/agent）测 `read_at` | 部分索引只含未读工作集（复现库 0~50 行） |
| `agent_silent_since_ready`（静默看门狗）<br>`NOT EXISTS(SELECT 1 ... WHERE from_agent=a.id)` | 每个新 spawn 的 agent 定时探测 | `from_agent` | **全表扫描 50000 行** | `COVERING INDEX` 探测 |
| `reassign_unread_user_messages` / `latest_user_message_for_agents`<br>`WHERE from_agent='user' ...` | 重指派 / 看门狗 | `from_agent` | **全表扫描** | 索引探测 |

**确定性复现（`EXPLAIN QUERY PLAN`，真实 5 万行库）：**

```
修复前：
  [count_unread]      SEARCH messages USING INDEX idx_messages_to_id (to_agent=?)   ← 扫 agent 全部历史
  [silence_watchdog]  SCAN m                                                        ← 全表 50000 行
  [reassign]          SCAN messages                                                 ← 全表

修复后（应用发布的 0025_message_perf_indexes.sql）：
  [count_unread]      SEARCH messages USING INDEX idx_messages_to_unread (to_agent=?)
  [consume_wakes]     SEARCH messages USING INDEX idx_messages_to_unread (to_agent=? AND kind=?)
  [silence_watchdog]  SEARCH m USING COVERING INDEX idx_messages_from_agent (from_agent=?)
  [reassign]          SEARCH messages USING INDEX idx_messages_from_agent (from_agent=?)
```

**为什么这是最佳方案。**
- **部分索引（partial index）`WHERE read_at IS NULL`** 是关键设计：它只索引「当前未读」这个工作集
  （复现库里 1000 / 50000 = 2%），`read_at` 一被置位该行立刻移出索引。所以**索引体积小、写放大极低**——
  完美匹配「查的就是未读、未读量恒定小、消息只增不减」这个访问模式。普通全列索引会随表无限增长。
- `from_agent` 用单列普通索引即可让看门狗的相关子查询命中 `COVERING INDEX`（不回表）。
- **刻意不加 `messages(thread_id)` 索引**：聊天主查询是
  `(thread_id=? OR thread_id IS NULL) ORDER BY id DESC LIMIT 200`。这个 `OR ... IS NULL` + `LIMIT` 形态下，
  对**活跃 thread**，规划器选择「反向 PK 扫描 + 提前终止」本就是最优，实测**不会采用** thread_id 索引
  （已验证：加了复合 `(thread_id,id)` 索引规划器仍 `SCAN m`）。加它在这张高写入表上是**纯写放大、零收益**。
  仅「打开稀疏旧 thread」冷路径会扫全表——非热路径，留待真有投诉时把 `OR` 改写成 `UNION` 再处理。
  （这条体现了「索引不是越多越好」：只加被 EXPLAIN 证明会被采用的索引。）

**改动**：`crates/flockmux-storage/migrations/0025_message_perf_indexes.sql`（新）+ `schema.rs` 注册。

```sql
CREATE INDEX idx_messages_to_unread ON messages(to_agent, kind) WHERE read_at IS NULL;
CREATE INDEX idx_messages_from_agent ON messages(from_agent);
```

**验证**：`cargo test -p flockmux-storage` 44 测试绿（含 `fresh_db_migrates_to_latest`=25、
`migrations_are_idempotent`）；并把发布文件应用到 5 万行真库确认 EXPLAIN 采用索引、版本记 25、两索引建成。

> ⚠️ 踩坑记录：项目的 migration 约定是**每个 SQL 文件末尾必须 `INSERT INTO schema_version VALUES (N);`**
> 来记录版本（runner 靠 `MAX(version)` 门控）。首版漏了这句导致版本没 bump、二次运行重复建索引报错。
> 测试 `migrations_are_idempotent` 当场抓到——这正是「真实测试」的价值。

---

### FIX-2 [前端 P0] `recentMessages` 无界增长 + 成员侧栏 O(成员 × 会话长度) 扫描

**问题机制。** commit `82ef570` 把 WS→列表中转改成「无损累积数组」修复了丢消息，但
`Chat.tsx` 的 `recentMessages` 这个**派生缓存**在 append 时漏了上界：

```js
// 修复前（Chat.tsx）—— 唯一一个无界累积器
return fresh.length === 0 ? prev : [...prev, ...fresh];   // 永不截断
```

对比其他三处同类 live 缓存（`MessagesPanel` / `SwarmPanel` / `useWorkspaceShellData`）**全都** `slice(-200)`，
唯独这里漏了。而 `recentMessages` 被成员侧栏 `.map` 里的 `statusDot(a, live, recentMessages, t)` 消费，
`statusDot` 内部对整个数组**反向全扫**找「该 agent 最后一条 inbound/outbound」（`lib/agent.ts:163`）。
成员 `.map` 是 JSX 内联 IIFE、未 memo，每次 ChatView 渲染都重跑；而 ChatView 因 `liveMessages` 变化在
**每条 WS 消息**都渲染。于是复杂度 = `O(成员数 × recentMessages.length)`，且后者随会话单调增长**永不回落**。

**为什么是真问题。** 长会话（几千条消息）+ 多成员时，每来一条新消息就触发一次对全量历史的扫描。
这正是「跑得越久越卡」的典型签名——而 flockmux 的核心场景就是长时间挂着的 swarm 会话。

**修复。** 与其他三处对齐，加 200 上界（`Chat.tsx`）：

```js
if (fresh.length === 0) return prev;
const next = [...prev, ...fresh];
return next.length > 200 ? next.slice(-200) : next;   // 与其他 live 缓存一致
```

**为什么这是最佳方案。** `recentMessages` 是**派生缓存**，statusDot 只需要「每 agent 最近一条收发」，
200 条尾部绰绰有余（与 MessagesPanel 自己拉 200 的语义一致）。这让成员侧栏的每次扫描从
`O(无界)` 降到 `O(成员 × 200)`（几千次迭代 → 上限千次），且不随会话增长。零行为变化、零正确性风险。

> 注意区分：`MessagesPanel` 的 `items`（聊天主历史）也是无界累积，但**那是用户要滚动回看的真实历史、
> 本就该全量保留**，且已用 `@tanstack/react-virtual` 虚拟化把渲染成本钉死——所以**不动它**。
> 只有「只需最近、却被写成无界」的派生缓存才该截断。这是判断的关键。

**验证**：`npm run build`（tsc 类型检查 + vite build）绿；`vitest` 58 测试绿。

---

### FIX-3 [后端 P1] DB 连接池 8 → 16

**问题机制。** `store.rs` 的 r2d2 池 `max_size(8)` 是**全局 DB 并发硬上限**。WAL 下 SQLite 允许多读单写，
但多 agent 同时落 usage/activity（每个 agent 的 transcript tailer 700ms 一次）+ 消息写入时，8 个连接
易被打满，新请求在 `pool.get()` 阻塞。tokio 默认 blocking 池有 512 线程，**池连接才是更紧的瓶颈**。

**修复。** `max_size(16)`（SQLite 连接每个才几 KB，极轻）。

**为什么这是最佳方案 / 边界。** 16 给**读**留了余量；**写**仍在单 WAL writer 上串行，调大连接数不会
增加写竞争（只会让更多读并发）。这是「移除一个无谓的人为上限」，低风险、有上界——不是无脑调大。

**验证**：`cargo build --workspace` 绿（server crate 消费正常）。

---

## 3. 诚实放弃的修复（investigated, not shipped）

### SKIP-1 [前端] Shell `ctx` 的 `useMemo`

并行审计提的「memoize Outlet ctx 减少子树重渲染」**经核实不可行也不值得**：

1. **不可行（干净地）**：`const ctx` 位于 `Shell.tsx` 的一个**条件 early-return 之后**（「workspace 未加载/
   不存在」分支先 `return`）。在那里加 `useMemo` 会是**非法条件 hook、违反 Rules of Hooks**。要做对得把
   组件大段逻辑上提到所有 early-return 之前，改动大、易引入 bug。
2. **不值得**：`ctx` 包含 `liveMessages` / `agentStateById` / `agentActivityById` 这些**每条消息都变**的字段，
   所以「ctx 内容变了」几乎在 Shell 每次重渲染时都为真——`useMemo` 给出的稳定 identity 收益**接近零**。
   ChatView 随消息重渲染是「实时聊天」的固有需求，且 MessagesPanel 已虚拟化把渲染成本钉死。

→ 审计高估了这条的独立价值。**正确的工程判断是不做。**

---

## 4. 核实为「无问题」的点（排除臆测，留作记录）

后端逐项核实，确认健康，**不要在这些地方浪费改动**：

- **无锁跨 `.await`**：reaper / wake / pty_ws / swarm / pty_stream 所有持锁点都在 await 前 `drop`/clone 句柄。
  全用 `parking_lot::Mutex`（同步短临界）+ `tokio::sync::RwLock`（仅 wake subs，正确 async）。
- **无 busy-poll**：wake 阻塞在 broadcast `recv().await`（零空转、纯事件驱动）；reaper 5s tick +
  `MissedTickBehavior::Delay`、已退出 agent 第一次 sweep 后 latch short-circuit。
- **cron 30s 一次**（不是每秒扫），时区分解 `fields_from_unix` 是纯算术、极廉价。
- **engine-probe** 不在启动热路径、有 `PROBE_IN_FLIGHT` 原子锁防并发 sweep、异步触发——设计正确。
- **PTY** 用专用 OS 线程（不占 blocking 池）；`Bytes::clone` 在广播是 refcount 不复制；广播容量充足
  （swarm 1024 / lifecycle 16）。
- **DB** 全走 `spawn_blocking`；批量写包事务；WAL + `busy_timeout` + `synchronous=NORMAL` 已设；
  FTS5 是 external-content、增量维护、无 rebuild；单个递归 watcher（非每 agent 一个）。
- **XtermPane** scrollback 上界 5000、resize 150ms debounce、ACK 节流、WebGL 上下文池——健康。

---

## 5. 记录但暂不改的 P1/P2（附「为何暂不改」）

这些是真实的次要优化点，但在一个**正确性来之不易**（历史上大量丢消息 bug 刚修完）的系统里，
**无实测 P0、改动有回归风险**时，正确的做法是记录+排期，而不是为「显得全面」去动它。

| # | 位置 | 问题 | 严重度 | 为何暂不改 |
|---|---|---|---|---|
| D1 | `swarm.rs:360` `send_message` | 每条消息 2–4 次串行 `spawn_blocking` DB 往返；wake fan-out 串行 | P1 | 收益需多 agent 压测量化；合并查询/并发化 fan-out 触及消息正确性路径，应单独验证 |
| D2 | `MessagesPanel.tsx:376` | activity backfill effect 在每个活动事件 `prev.map` 全 items | P1 | 触及活动回填逻辑（subtle）；应配真机 profiler 确认收益后再动 |
| D3 | `transcript.rs:32` | 每 agent 700ms 文件轮询，空闲仍转；共享 1024 broadcast 放大 Lagged 风险 | P1 | 注释说刻意轮询避开 rotation 边界；可做自适应退避，但需真机验证不漏增量 |
| D4 | `opencode_tui.rs:184` | probe/bootstrap 期 2–3s 固定轮询、每轮拉全量 session 列表 | P1 | 仅 probe/bootstrap 期非常驻；正解是订阅 opencode `/event` SSE，工作量较大 |
| D5 | `pty/lib.rs:284` | 每 PTY chunk `Bytes::copy_from_slice` 一次堆分配 | P1 | portable-pty 阻塞 Read 接口的固有税，分配器能扛；BytesMut 池收益有限 |
| D6 | `spawn.rs:450` `scan_osc` | 每 chunk 三个 scanner 各自 `windows().position()` 朴素子串扫 | P1 | 常数小（buf ≤4-8KB），稳态 CPU 非延迟问题；可换 `memchr::memmem` |
| D7 | `wake.rs:540/864` | 每个黑板事件 clone 整张 subs 表；Lagged 恢复 O(agents×keys×3) 串行 fs 读 | P2 | subs ≤10 项 clone 便宜；Lagged 是罕见路径 |
| D8 | `usage.tsx:167` | 用量页 8s 轮询全量 reload | P2 | 用量数据变化慢，拉长到 30-60s 即可，低优先 |

**建议排期**：若未来要再做一轮，优先 D1（多 agent 压测后合并 send_message 往返）和 D3（tailer 自适应退避），
这两条在「agent 数量上来」时收益最直接。其余 D2/D4–D8 属锦上添花。

---

## 6. 改动清单与验证矩阵

**代码改动（4 个文件）：**

| 文件 | 改动 |
|---|---|
| `crates/flockmux-storage/migrations/0025_message_perf_indexes.sql` | 新增：2 个索引 + 版本记录 |
| `crates/flockmux-storage/src/schema.rs` | 注册 MIGRATION_0025 |
| `crates/flockmux-storage/src/store.rs` | 连接池 `max_size(8)` → `16` |
| `web/src/routes/workspace/views/Chat.tsx` | `recentMessages` 加 `slice(-200)` 上界 |

**验证矩阵（全绿）：**

| 验证 | 命令 | 结果 |
|---|---|---|
| 存储层测试（含迁移幂等/最新版本=25） | `cargo test -p flockmux-storage` | ✅ 44 passed |
| workspace 全量构建（CI 硬门禁） | `cargo build --workspace` | ✅ Finished 8.88s |
| 前端类型检查 + 构建（CI 硬门禁） | `npm run build` | ✅ tsc + vite 绿 |
| 前端单测 | `npm test` (vitest) | ✅ 58 passed |
| 索引确定性复现（5 万行真库 EXPLAIN） | 见 §2 FIX-1 | ✅ 全表扫描 → 索引查找 |
| 发布文件端到端（应用 0025 + EXPLAIN + 版本=25） | 见 §2 FIX-1 | ✅ |

---

## 7. 经验沉淀

1. **「修对了 bug」和「修得没留性能债」是两件事。** `82ef570` 的无损累积修对了丢消息，但派生缓存漏上界
   留下了「越跑越卡」的债。改 live 数据流时，**append 必问上界**——而且要区分「该全量保留的真实历史」
   （虚拟化解决）和「只需最近的派生缓存」（截断解决）。
2. **索引不是越多越好。** 只加 `EXPLAIN QUERY PLAN` 证明会被采用的索引；规划器对 `OR ... IS NULL + LIMIT`
   会选反向 PK 扫描，盲加 thread_id 索引就是纯写放大。部分索引（partial index）匹配「未读工作集恒定小」
   的访问模式，是这里的点睛之笔。
3. **静态审计要亲自复现核实。** 4 路 agent 的清单里，有论断（drain liveMessages / ctx memo）核实后
   是**错的或不值得的**——盲信就会引入回归。真实库 EXPLAIN、读 early-return 结构，才挡住了这些。

---

## 8. 第二轮：深入复核 + 真实运行时测量（2026-06-20 续）

第一轮后又做了一轮「继续深入」，目标是：(a) 把 defer 的 D1/D2 真正读透决定是否该修；(b) 起真实
全栈做运行时测量（不止静态）；(c) 专项排查「还有没有别的同类无界累积器」。结论：**第一轮已抓住真正的
大头；这一轮用实测证据确认了修复价值、排除了一整类隐藏问题，未发现需要再改的 P0/P1。**

### 8.1 深读 defer 项 → 维持「不改」判断（带依据）

- **D1 `send_message` DB 往返 / wake fan-out 串行**：读码确认 `agent_thread_id` 只有 user→agent 才触发
  第二次查询（agent→agent 仅一次）；wake fan-out 串行 `deliver_wake` 触及**有丢消息 bug 历史**的唤醒
  正确性路径。收益小（省一次往返 / 并发几个 kick）、风险触及敏感路径 → **不为小收益动它**。
- **D2 `MessagesPanel` activity 回填 effect**：读码确认它对绝大多数 items **早返回**
  （`!trace || to_agent !== USER` 一行判掉），只有「agent→user 且 trace 未 finalized」的极少数近期消息进入
  真正的 filter；常数极小，且是 30s grace 的敏感回填逻辑。改它收益小、风险大 → **维持 defer**。

### 8.2 首屏 bundle → 已切分良好（核实非问题）

build 输出里的大库（cytoscape 435KB / charts 483KB / katex 258KB / xterm 403KB）**都不在首屏**：
路由级 `lazy()`（Dag/Usage/Settings… 全部 R2-006 已做）、`ChatMarkdown` 在 MessagesPanel 内 `lazy`
（markdown+katex 随首条消息异步加载）、`XtermPane` 不在聊天落地页路径。**首屏 bundle 不是问题，无需改。**

### 8.3 真实运行时测量（起隔离全栈 :7788/:5188 + chrome-devtools）

| 测量 | 方法 | 结果 |
|---|---|---|
| **后端修复线上生效** | 真实运行后端的 `d.db` | schema 版本 = **25**、`idx_messages_from_agent` + `idx_messages_to_unread` 均在、WAL、synchronous=NORMAL、日志**零** Lagged/busy/lock |
| **FIX-2 价值量化（真实 V8）** | 在浏览器里复现 statusDot 反向扫描，6 成员 × 200 渲染、不活跃成员最坏扫到底 | 见下表——无界版随会话长度**线性增长到 31×**，FIX-2 钉死在 200 行水平 |
| **冷启动 CWV** | chrome-devtools performance trace | LCP **271ms**、CLS **0.00**、TTFB 4ms，无 long-task 告警（dev server，生产更优） |
| **运行时健康** | console | **零** error/warning |

**FIX-2 在真实 V8 引擎里的扫描成本（每来一条消息同步阻塞主线程一次）：**

| 会话消息数 | 成员侧栏扫描耗时（200 渲染合计） | 相对 |
|---|---|---|
| **200（FIX-2 上界）** | **2.3 ms** | 1× |
| 2000（修复前无界） | 8.4 ms | 3.6× |
| 10000（修复前无界） | 31.2 ms | 13× |
| 30000（修复前无界） | 71.1 ms | **31×** |

→ 证实这正是「**会话跑得越久越卡**」的来源，而 FIX-2 让它与会话长度**解耦**。

### 8.4 专项排查：还有没有别的无界累积器？（关键负向结论）

「WS 驱动、每消息追加、无上界」正是 FIX-2 那类 bug。逐个核实**所有** `[...prev` / `ref.set` 累积点：

| 累积器 | 位置 | 是否有界 |
|---|---|---|
| `liveMessages` ×3（Shell / SwarmPanel / Chat 间接） | `useWorkspaceShellData:340` / `SwarmPanel:101` | ✅ 均 `slice(-200)` |
| `recentMessages` | `Chat.tsx:862` | ✅ **本次 FIX-2 加 `slice(-200)`** |
| `agentActivityById`（每 agent 活动） | `useWorkspaceShellData:302/341` | ✅ `MAX_ACTIVITY` cap + `slice(-200)` |
| `idToFromRef`（Map） | `useWorkspaceShellData:221/343` | ✅ 每次 recompute 从 200 条 fetch 整体重建 |
| `countedUnreadRef`（Set） | `useWorkspaceShellData:357/374` | ✅ 读消息时 `delete` 自清理 + recompute 重置 |
| `items`（聊天主历史） | `MessagesPanel:485` | ⚠️ 有意无界，但**已虚拟化**渲染、是用户要回看的真实历史 → 正确保留 |

**结论：`recentMessages` 是唯一真正无界、且被每渲染全扫的累积器（已修）。其余全部有界或为有意保留的
虚拟化历史。不存在隐藏的第二颗同类炸弹。** 这条负向结论本身有价值——它把「越跑越卡」的风险面收口了。

### 8.5 第二轮总评

- **未新增代码改动**：深入复核后没有发现值得再改的 P0/P1；剩余 D1–D8 维持原排期判断（见 §5）。
- **新增的是证据**：后端修复**线上端到端验证**、前端修复**真实 V8 量化**（31× → 解耦）、
  以及「无第二颗无界炸弹」的专项负向结论。
- 真正需要再投入的，仍是 §5 的 D1（多 agent 压测后合并 send_message 往返 + 并发 wake fan-out）和
  D3（tailer 自适应退避）——它们的收益只有在「agent 数量真正上来」时才显现，且需要真实多 agent 压测
  环境来量化，属于下一阶段而非现在。
