# flockmux-core 实现审查报告（2026-05）

> 多 agent + workflow 编排式审查。先**理解**（8 个 agent 测绘子系统）→ **挖掘**（6 个 agent 研读 swarm-ide / superpowers / openclaw / hermes-agent / gstack / golutra）→ **客观信号**（clippy / tsc / grep）→ **对抗式验证**（22 个 agent 逐条尝试证伪 + 3 项人工亲验）。
> 严重度均经对抗验证校准：很多初判被**下调**并标注了已有缓解，所以这里的分级偏保守、可辩护。

## 方法论与可信度

| 阶段 | 手段 | 结论 |
|---|---|---|
| 理解 | 8 agent 测绘 server/mcp/swarm/storage/spawn/spells/frontend | 83 条初步风险 + 数据流/并发模型 |
| 挖掘 | 6 agent 研读参考项目最新版（已 `git pull`） | 各家多 CLI / swarm 方案 + 可借鉴模式 |
| 客观信号 | `cargo clippy` / `tsc` / grep | clippy **干净**；tsc **通过**；268 `unwrap()`；45 `eslint-disable` |
| 亲验（人工） | 直接跑/读 | 3 项**全部坐实** |
| 对抗验证 | 22 agent 逐条证伪 | 13 confirmed / 10 partial / **0 refuted** |

**整体判断**：架构是健康的——9 crate 边界清晰、clippy 干净、关键并发用 `spawn_blocking`+`with_busy_retry` 处理得当、M6b/M6c/M.5 等历史坑都有针对性修复且有注释。问题集中在三类：**(1) 安全/进程生命周期的边界**、**(2) 协调（wake/blackboard）的静默失败模式**、**(3) 多 CLI 抽象只做了一半**（你最关心、也是 5 个参考项目都做得更彻底的地方）。没有发现数据损坏级的硬 bug，绝大多数问题是"在罕见窗口/误用下咬人"或"可维护性/扩展性债"。

---

## P0 — 关键（应尽快处理，成本都不高）

### P0-1　无鉴权 + permissive CORS + WS 不校验 Origin → 跨站 WebSocket 劫持（人工亲验 ✅）
- **证据**：`main.rs:270 CorsLayer::permissive()`（注释 "localhost dev convenience"）；全仓库 grep 无 `check_origin`/`Authorization`/`Sec-WebSocket-Origin`；`/ws/pty`、`/ws/swarm` 升级处无来源校验。
- **影响**：浏览器对 WebSocket **不做同源限制、无 CORS 预检**。用户在浏览器打开任一恶意网页期间，该页可 `new WebSocket('ws://127.0.0.1:7777/ws/pty/<id>')`，进而**注入任意按键到带 `--dangerously-skip-permissions` 的 live agent**、读写黑板、外泄 PTY 输出；配合 permissive CORS 还能读 REST JSON。这是回环服务上的 DNS-rebinding / 跨站 WS 劫持类 RCE。
- **诚实的边界**：本质是单用户本地工具；现代 Chrome 的 Private Network Access 会部分拦截"公网页→localhost"的连接，所以实际利用在变难——但不应依赖浏览器兜底。
- **修复（小）**：WS 升级 + REST 加 **Origin 白名单**（只放行 tauri 自身 origin / `127.0.0.1`）+ 启动随机 **session token**（前端启动时拿、每请求带）。参考 **openclaw** 的 WS 握手（`declaredCaps` + slow-consumer 断开）。

### P0-2　主干带着失败的测试发布（人工亲验 ✅）
- **证据**：`cargo test -p flockmux-mcp` → **41 passed / 1 FAILED**，`handlers.rs:102 tools_list_returns_full_tool_surface` panic `left: 8, right: 9`（删 spell 工具后没同步改断言）。
- **影响**：本身是 stale 断言（低危），但**说明 CI 没有 gate `cargo test`**——这才是 P0：它意味着上面所有"测试覆盖"的假设都不可信。
- **修复（小）**：改断言 `9→8`；把 `cargo test --workspace` + `tsc` 接入提交/CI 门禁。

### P0-3　kill 不回收孙进程 + 误导性文档（人工亲验 ✅）
- **证据**：进程树是 `server → (PTY) → flockmux-shim → 真实 CLI(claude/codex)`。`pty/lib.rs:164-171 kill()` 只 `child.kill()`(SIGKILL shim) + `child.wait()`，**无 setsid / 无 process group / 无 killpg**（grep 证实）。文档注释（`pty.rs:163-167`、`pty.rs:38`）声称"先关 master 发 SIGHUP→SIGTERM→SIGKILL 升级"，**代码完全没做**。
- **影响**：SIGKILL 掉 shim 不会连带杀掉它的子进程（真实 CLI）——真实 CLI 被 reparent 到 init **继续运行**，持续烧 API token、占着 workspace、可能写坏文件。server 崩溃时所有孙进程全部泄漏；`main.rs:139` 的 boot orphan-settle **只改 DB 行、不扫不杀 OS 进程**。
- **修复（中）**：shim `setsid` 建独立进程组；kill 时 `killpg(pgid, SIGTERM)` → 宽限 → `SIGKILL`，兑现文档承诺的升级序列。直接照搬 **openclaw `ProcessSupervisor.signalProcessTree` + force-kill-wait**。同时**修正/删除**那段说谎的文档注释。

---

## P1 — 多 CLI 可扩展性（你的头号关切，单列一章在下方"多 CLI 重构方案"）

核心结论先放这里：**`cli-plugins/<id>.toml` 现在只是一张"功能开关表"，真正的 per-CLI 行为硬编码在 Rust 里**。详见后文。相关已确认发现：

- **F7（confirmed/medium）加第三个 CLI 不是改配置**：`pre_spawn.rs:544 match plugin.id` 命中 `other => debug-noop`——gemini 能 spawn 但**无 trust patch / 无 MCP 注入（=没有 swarm_* 工具，无法协调）/ 无 Stop hook（=永远不会被 wake）**，成为协调死岛。真正落地需改 Rust ~3 个承重点（`run_<id>_patches`+match 臂、per-CLI 写 MCP/trust/hook 的函数、`spawn.rs` argv 分支）+ `tools.rs:149` 的 spawn_worker enum 提示。
  - 校准：`cliInputPolicy.ts`/`GraphPanel` 颜色有兜底、enum 只是给 LLM 的 schema 提示（服务端真正的门是 `PluginRegistry::get`），所以是"3 处 Rust + 1 处 schema"，而非"5+"，且失败是静默降级而非崩溃。
- **F8（confirmed/low）死配置**：`plugins.rs:19-23` 的 `mcp_inject`/`ready_detect` 字段**全仓库零功能读取**（grep 证实），`codex.toml:17` 自己都注明"仅作文档"。它们长得像派发机制，实际派发是 `match id`——误导后来者。
- **F9（confirmed/low）一个坏 TOML 拖垮全部 CLI**：`plugins.rs:102-106 load_dir` 用 `?`，单个 `cli-plugins/*.toml` 解析失败→`main.rs:90` 传播→**server 启动失败、claude+codex 全没**。对比 `roles.rs:109-122` 是 warn+skip。两个同级 registry 韧性策略不一致。

---

## P1 — 协调正确性（wake / blackboard 的静默失败）

这是产品核心价值所在，问题都属于"静默地不工作"——最难排查。

- **F3（confirmed/medium）wake 精确字符串匹配 + 零订阅者无任何诊断**　`wake.rs:204 keys.iter().any(|k| k == key)` 纯字节相等，无 trim/规范化/模糊匹配；派发循环 `wake.rs:422-428` 在 `select_targets` 返回空时**什么都不打印**——哪怕同一 path 存在 ExitKey。`handoff_signal`/`depends_on` 是 LLM 自选字符串（`tools.rs:553-565`），任何漂移（少前缀、尾斜杠、typo、大小写）→ 依赖方永远收不到 wake，且**生产者已成功退出（写了信号），所以 `.error` fallback 也不触发**。运维 tail 日志看不到任何信号，只看到一个 idle agent。
  - 缓解：mailbox wake 是 source-of-truth + 下次自然 Stop 消费、`detect_depends_on_cycles` 校验 manifest 图、`append_wake_sub` 兜住"spawner↔自己 worker"最常见拓扑、有手动 ⚡ wake 兜底。**真正的洞是"漏匹配时零诊断"**。
  - **修复（小）**：`select_targets` 返回空但该 path 有 ExitKey 时 `warn!`（已经是把 wake 漏掉的唯一可观测点）。**进阶**：引入 gstack 的"带 schema 的 finding envelope"——黑板写入用结构化键，coordinator 按结构匹配而非自由文本精确匹配。
- **F12（confirmed/medium）broadcast lag 永久丢一次性 wake**　`swarm.rs:65` 单条 `broadcast(256)` 承载所有 SwarmEvent；`wake.rs:448-455` 的 `Lagged` 臂**只 warn、不重放**。注释说"下次写同 key 会补上"，但**一次性 handoff key 没有下次**→依赖方永久 idle。mailbox 兜底无效，因为 mailbox 行是在 `deliver_wake` 内写的，事件被丢了就根本不会执行到。
  - **修复（中）**：按事件类型拆分 channel（或显著加大 cap）；lag 后**重新扫描已订阅 key 对 `blackboard_ops` 做一次对账重放**。叠加 **golutra 的合并式 trigger 调度器**（min-heap，同 agent 多次 reschedule 折叠成一次）抗 wake 风暴。
- **F6（confirmed/medium）黑板 fs::write 与 DB insert 非原子**　`swarm.rs:211-231` 两次独立 await、无事务。若 insert 失败（重试预算耗尽 / 磁盘满）或进程在中间死掉：**文件在盘上、但无 op 行、无 FTS、无 BlackboardChanged**→该 path 从 `swarm_list_blackboard` 发现里消失、依赖它的 agent 永不被唤醒。`read_blackboard` 读实时文件所以内容不丢，但无对账/修复 pass。
  - **修复（中）**：启动时做一次 blackboard 目录 vs op-log 的**对账**；或把 fs+insert 收进单 tx（fs 写在 spawn_blocking 内、insert 失败回滚删文件）。
- **F13（confirmed/medium）auto-kill 按信号 path 匹配、无"同一 agent"守卫**　`wake.rs:484-494` 仅 `if ek.handoff_signal == path`，不比 writer、不查 `spawned_at` 新鲜度（`select_targets` 排除 writer、`handle_agent_exit` 查新鲜度，**唯独这里没有**）。两个 agent 声明同一 `handoff_signal`（ad-hoc `/spawn` 对 `handoff_signal` 零校验）时，A 写信号会把**还没干完的 B 也杀掉**。
  - **修复（小）**：`maybe_auto_kill_on_handoff` 把 `writer` 传进去、要求 `aid == writer`（+ 可选新鲜度），照抄 `select_targets` 的 writer 排除逻辑。
- 相关 partial（已下调，记录备查）：**F2** depends_on 不过 `{workspace_id}` 替换——真实 worker 路径是对称裸字符串流不会失配，仅"未来手写多 agent spell 在 manifest depends_on 里用 `{workspace_id}`"会静默失配（**建议加 lint/doc**）。**F10** seen_sha 把字节相同的**带外**写当自写丢弃（agent 走 API 路径不受影响）。**F11** consume_wakes 先标已读再投递、hook 在窗口内死掉会丢这一次 wake（但 wake 是无状态触发器、数据在 blackboard、下次写会补；仅"最后一次事件 + 无后续写"才永久搁浅，有手动 ⚡ 兜底）。

---

## P2 — 进程 / PTY 生命周期

- **268 个 `.unwrap()`（非测试，`pre_spawn.rs` 独占 120）**　这些文件大量做文件 IO + JSON/TOML 解析 + 配置写入，`unwrap()` 会 **panic-on-bad-input**。虽然跑在 `spawn_blocking` 里只 abort 该任务，但用户的 `~/.claude.json`/`~/.codex/config.toml` 一旦是非常规但合法的结构，就可能让 spawn 直接挂掉且无友好报错。**建议**：pre_spawn 的配置读写全部 `?`/`warn` 化（它本来就是 best-effort）。
- **F1（partial/low）WS `ClientControl::Kill` 泄漏**　`pty_ws.rs:365-368` 只 `bridge.kill()`、不 `registry.remove`/不 unregister/不 record_kill，和 DELETE 全量拆除（`rest.rs:460-484`）发散。**但前端从不发送 `kill` 这个 WS control**（`XtermPane.sendCtrl` 只发 ack/resize，UI 杀 agent 走 DELETE），只有手搓 WS 客户端能触发；且 ShimExit 仍会记录所以**不会显示为 live**。建议：要么删掉这个无人调用的 control，要么让它复用全量拆除。
- **F17（confirmed/low）`binary_supports_flag` 阻塞探测无超时**　`spawn.rs:581` 用 `std::process::Command::output()` 同步跑 `<binary> --help`、**无超时**，在 async spawn 路径上。注释说"超时按 false 处理"是假的。缓解：仅 codex 触发、按进程缓存（每进程最多一次）、`codex --help` 通常毫秒级。**修复（小）**：包 `tokio::time::timeout` 或 `wait-timeout`，让注释成真。
- **F20（partial/low）boot orphan-settle 是全表 blanket UPDATE**　`store.rs:537 UPDATE agents SET killed_at WHERE killed_at IS NULL` 无实例/PID 范围，靠 `main.rs:128` 的 flock 单实例保证。校准：lockfile 与 DB 同目录、flock 在开库前就拒绝第二实例，所以正常本地 FS 上**两个前置条件耦合、难独立触发**；残留风险仅 NFS/篡改 lockfile/未来绕过 lock 的代码路径。对应 MEMORY 里记的 .app sidecar 多实例隐患。

---

## P2 — 存储

- **F5（confirmed/medium）无限增长、无保留策略**　`blackboard_ops` 每次写都存**整个文件内容**（`content TEXT NOT NULL`）、无 dedup、无 upsert；同一文件改 1000 次 = 1000 份全文。`messages`/`pty_recordings` 也只增不减。全 crate **无 prune/retention/VACUUM**（唯一 DELETE 是用户手动 detach root）。校准：FTS5 是 external-content 模式（不存第二份正文）、读路径都 `LIMIT 200`（查询不会爆）——所以是**磁盘占用问题**，对长跑 server 才明显。
  - **修复（中）**：加保留策略（按 workspace/时间裁剪）+ 写时 dedup-vs-prev-sha + 周期 `VACUUM`/`wal_autocheckpoint` 调优。
- 其他（来自子系统测绘，低危但值得清）：`list_blackboard_ops(None)` 的 `MAX(id) GROUP BY path` **无配套索引**（现有 `(path, at)`，缺 `(path, id)`）；`mark_delivered`/`mark_read` 事务内 **N+1**（应 `WHERE id IN(...)`）；`0003` 迁移注释说 FK off 但 `connection.rs:25` 实际 `PRAGMA foreign_keys=ON`，且无 `ON DELETE` 行为定义；`workspaces.slug` 仅 32 位熵、碰撞时 `with_busy_retry` 不重试 ConstraintViolation。

---

## P2 — 前端

- **F15（confirmed/low）两套完整 DAG 实现且已漂移**　`Dag.tsx`（ReactFlow+dagre，主 `/dag` tab）vs `GraphPanel.tsx`（手搓 SVG，仅 `/debug`）。已实测漂移：satisfied 判定 `>=` vs `>`、`spawned_at != null` vs `?? 0`、**边方向相反**、producer 查找 Map vs 线性 find。两者都在近期 commit 里被同时维护。**修复**：删 `GraphPanel`/`SwarmPanel`（/debug 专用），或抽出共享的 edge-derivation。
- **F19（partial/low）http.ts 无请求取消**　全 `api.*` 无 `AbortController/signal`；`Shell` 的 refreshAgents/recomputeUnread/refreshWorkspaces、`Dag.refresh`、`GraphPanel.refresh` 无 cancelled 守卫（`Chat.tsx` 有）。校准：Shell 不按 wsId remount、这些端点是**全局数据**（无 wsId 入参，前端按 workspace 过滤）、React18 下卸载后 setState 是 no-op——所以是"短暂自愈的 stale 渲染"，非跨 workspace 错乱。**修复（小）**：`request()` 加 `AbortController` + cancelled 守卫，纯卫生。
- **F18（partial/low）AgentDrawer 切 tab 全量重放**　切到非终端 tab 卸载 `XtermPane`→`clearLastSeq`，切回触发整 ring（≤1MiB）重放 + WS 重连。校准：这是**故意设计**（一次性 agent 的空白面板 bug 的修复）、有 1MiB 上限、人为点击节奏——仅是反复切 tab 时的冗余重连/重绘小毛病。
- **其他**：`Shell.tsx` **1439 行 god-file**（route + sidebar 树 + ManageRootsDialog CRUD + 乐观级联删除全挤一处）；45 处 `eslint-disable`（多为 `react-hooks/exhaustive-deps`，stale-closure 温床）；33 个 `any`；`main.tsx` 全局关 StrictMode（关掉了 effect 泄漏检测）；per-CLI 知识在前端硬编码（`cliInputPolicy.ts` 的 `startsWith('codex-')`、`GraphPanel` 颜色）——应改为从服务端 `CliPluginInfo` 取（见多 CLI 章）。

---

## P3 — spells / roles

- **整套 spells/roles 实际是 dead-weight**：磁盘上只有 **1 个 spell（init）+ 1 个 role（orchestrator，handoff_signal 空、depends_on 空）**。`role_ref` 合并/override、`system_prompt_prefix`(HITL gate)、`allow_cycles`、`{<role>_id}` 互引、PerAgent layout、多 agent spawn——**全没被在线数据驱动，只有单测覆盖**。`swarm_run_spell` MCP 工具已移除，改用 `swarm_spawn_worker`。**这块代码很可能已对着 live worker 路径腐烂**。建议：明确**复活（补 roles/spells）还是删**，别让 UI 暗示后端没强制的能力。
- **F22（confirmed/low）bootstrap 注入两份近似拷贝**　`rest.rs:664-721`(spawn_worker) ≈ `rest.rs:2043-2149`(run_spell)：同样的 ShimReady 等待、`2500ms` MCP-settle、`paste+150ms+\r`。作者注释自认"copied near-verbatim"。**关键 WHY 注释只在 run_spell 那份**，改 spawn_worker 的人看不到这些 magic number 的约束 → 漂移风险不对称。**修复**：抽 `async fn` 共用。
- **F21（confirmed/low）frontmatter 闭合检测过于天真**　`spells.rs:357 after_open.find("\n+++")` 取第一处 `+++`，不感知 TOML 三引号字符串上下文。`system_prompt = """..."""` 里粘了 diff 行（`+++ b/path`）会**提前截断 frontmatter→TOML 解析失败→spell 被静默 warn-skip**。对一个 prompt 里常嵌 diff/代码的产品是真实脚枪。**修复**：toml-aware split，或闭合 fence 要求行首独占。
- **F14（partial/low）spawn_worker 不过 render_prompt**：与 run_spell 不对称，但这是**设计使然**（worker prompt 由 LLM 在运行时用具体值现写，没有模板变量要展开）；仅当 LLM 误抄 spell 模板语法才有"字面 `{token}`"的提示性脚枪。

---

## 多 CLI 重构方案（综合 5 个参考项目）

> 你的诉求：claude / codex 已有，未来还要加别的。现状是抽象只做了一半——下面是把 5 家最佳实践融合后的目标形态，**从小到大分层**，可增量推进。

### 现状诊断
`cli-plugins/<id>.toml` 能声明的只有 `binary / default_args / home_env` + 一堆 `auto_*` 布尔开关；而**真正 per-CLI 的行为**（MCP 配置写哪、什么格式；trust 记哪；Stop hook 文件+超时单位 ms/s；argv 怎么注入；codex "Hooks need review" 对话框自动答 `2\r`）**全硬编码在 Rust**，按字面 `plugin.id` 分散在 `pre_spawn.rs`(match) + `spawn.rs`(inline if) + `tools.rs`(enum) + 前端。manifest 里 `mcp_inject`/`ready_detect` 是**死字段**。这正是 5 个参考项目都刻意避开的反模式。

### 五家怎么做（对照）
| 项目 | 多 CLI 抽象 | 可直接借鉴 |
|---|---|---|
| **openclaw** | 两层：声明式 `CliBackendConfig`(90%) + 类型化 `CliBackendPlugin` 钩子(10%)；`bundleMcpMode` 枚举(`claude-config-file`/`codex-config-overrides`/`gemini-system-settings`)；统一 `ProcessSupervisor`(child\|pty + watchdog + SIGTERM→SIGKILL + run registry) + 并发 lanes | **几乎就是 flockmux 该长成的样子**；且独立落地了和 M6b 一样的 `--strict-mcp-config --mcp-config` 修复 |
| **gstack** | 单一模板 + 每 CLI 一个声明式 `HostConfig`（toolRewrites/suppressedResolvers/frontmatter 规则/anti-injection 边界串/self-invocation guard）；`validateAllConfigs` 编译期查重 | 加 CLI = 一个声明文件 + 编译期校验；"host ≠ model"axiom |
| **superpowers** | 一份 `skills/` + N 个薄 per-CLI adapter 指向它；per-CLI **工具词表映射**；host 探测 + 幂等 bootstrap 注入；**每 CLI golden 验收测试** | 内容零分叉；"装了但 bootstrap 没到达模型"这类回归用测试卡住 |
| **hermes** | 一个 `BaseEnvironment` ABC 驱动 6 后端；**ACP / app-server JSON-RPC** 结构化驱动外部 CLI（而非刮 PTY）；bundled + `$HOME` 分层 override | 未来对支持 ACP 的 CLI 走结构化协议；分层 registry override |
| **golutra** | 最像 flockmux（Tauri+portable_pty+OSC633 shim）；声明式 `post_ready_plan` DSL(Input/WaitForPattern/ExtractSessionId/Introduction)；自适应 PTY 字节率 idle 检测；合并式 trigger 调度 | **post_ready_plan 把 codex 信任门/会话 id 抓取/MCP-ready 做成数据**；其编译期枚举多 CLI 反而**不如** flockmux 的运行时 TOML——别学这点 |

### 目标分层（增量）

**L1 — 把 manifest 从"开关表"升级成"行为描述符"（消灭 `match id` 的大头）**
```toml
# cli-plugins/gemini.toml （示意）
bundle_mcp_mode = "gemini-settings-json"     # 取代死字段 mcp_inject + match id，驱动用哪个写 MCP 的 writer
trust = { mode = "settings-json", path = "..." }
stop_hook = { file = ".gemini/hooks.json", format = "json", timeout_unit = "s", timeout = 10 }
input_policy = { settle_ms = 300, bracketed_paste = true }   # 取代前端 startsWith('codex-')
node_color = "#16a34a"                        # 取代 GraphPanel 硬编码

[[ready_plan]]                                # golutra DSL：把 DialogAutoAnswer / 2500ms 外置成数据
step = "answer_dialog"; needle = "Trust this folder"; response = "1\r"
[[ready_plan]]
step = "wait_for"; pattern = "MCP servers? ready"; timeout_ms = 8000
```
要点：把 claude(ms)/codex(s) 的超时单位、codex 的对话框 needle/response、MCP 注入通道**统统变成数据**——这些是"目标 CLI 的属性"，本就不该当 Rust 常量。

**L2 — 给不可约的 10% 留一个类型化 adapter trait（openclaw `CliBackendPlugin` / hermes ABC）**
```
trait CliAdapter {
    fn prepare_home(&self, ctx) -> Result<()>;        // 写 trust / dismiss update
    fn write_mcp_config(&self, ctx) -> Result<()>;    // 按 bundle_mcp_mode 选实现
    fn resolve_argv(&self, ctx) -> Vec<String>;       // 取代 spawn.rs 的 inline if
    fn write_stop_hook(&self, ctx) -> Result<()>;
}
```
派发从 `match plugin.id` 变成 `registry.adapter_for(plugin)`；加 CLI = 新 TOML +（仅当配置格式全新时）一个 adapter 实现，在**一个地方**注册。启动跑 `validate_all`（gstack）：声明了 `bundle_mcp_mode="..."` 却没有对应 adapter → 启动即报错而非 spawn 时静默降级。**顺带修 F7/F8/F9**。

**L3 — 每 CLI golden 验收测试（superpowers `run-test.sh`）**
对每个 plugin：headless 拉起该 CLI、发一条标准 prompt、grep 其事件流断言"swarm bootstrap 已加载 + 某个 `swarm_*` 工具被调用"。把"加一个 CLI"变成**可测合约**，专杀"装了却没接上"这类最常见回归。

**L4 — 结构化协议传输（hermes ACP/app-server，面向未来）**
manifest 加 `transport = "pty" | "acp" | "app-server"`。对暴露 ACP/JSON-RPC 的 CLI（Codex app-server、Copilot `--acp`），走结构化协议拿**真正的 tool-call/permission/streaming 事件**，取代"刮 PTY + 按英文 needle 自动答对话框"（现状 codex `"Hooks need review"`→`2\r` 极脆、绑死 0.132 版本、换语言/换菜单序就答错）。PTY 作为通用兜底保留。**把 JSON-RPC-over-stdio 抽成一个组件**（hermes 的反面教材：它重复实现了 3 份）。

**L5 — 配套加固**
- **分层 registry override**（hermes）：扫 bundled `cli-plugins/` + 用户 `~/.flockmux/cli-plugins/`，按 id last-writer-wins，用户不 fork 仓库即可改 CLI。
- **spawn 上限 + 能力交集**（hermes + openclaw lanes）：spawn 深度上限、fan-out 上限、并发 lane——**修 F4 fork-bomb**，加固 auto-trust/skip-permissions 姿态。
- **model 与 CLI 解耦**（gstack "host ≠ model"）：别把 model 身份焊进 CLI id；留一个 model overlay 轴，让行为微调不分叉 role。

---

## 可借鉴模式速查表（按主题）

| flockmux 痛点 | 借鉴来源 | 模式 |
|---|---|---|
| 多 CLI 抽象只做一半 | openclaw / gstack | 声明式配置(90%) + 类型化钩子(10%) + 编译期 validate_all |
| 加 CLI 要改代码 | superpowers | 一份内容 + 薄 adapter + 工具词表映射；golden 验收测试 |
| per-CLI 拉起握手硬编码 | golutra | `post_ready_plan` DSL（数据驱动的 WaitForPattern/AnswerDialog/ExtractSessionId） |
| 刮 PTY + 答对话框脆弱 | hermes | ACP/app-server 结构化传输（拿真 permission/tool-call 事件） |
| wake 漏匹配静默 | gstack / swarm-ide | 结构化 finding envelope（按 schema 匹配）；per-reader read-cursor unread |
| wake 风暴 / 一次性 wake 丢失 | golutra | 合并式 min-heap trigger 调度；one-inflight-per-agent dispatch batcher |
| kill 不回收进程树 | openclaw | `ProcessSupervisor.signalProcessTree` + force-kill-wait + no-output watchdog |
| fork-bomb | hermes / openclaw | spawn 深度上限 + 能力交集 + 并发 lanes |
| 无 CJK 历史搜索 | hermes | 双 FTS5（unicode61 + trigram）路由 |
| 跨 agent prompt 注入 | gstack | DATA_START/END 边界 + 临时文件传参 + reviewer 只读 |
| 自我评审不独立 | gstack | self-invocation guard（CLI 不能当自己的独立 reviewer） |

**别学的反模式**：swarm-ide 提交 live API key、给每个 agent 开无沙箱 bash；openclaw `reconcileOrphans()` 故意 no-op（Tauri 会重启，flockmux 必须真对账）；golutra 编译期枚举多 CLI + 靠 TUI glyph 刮输出；hermes 把 JSON-RPC plumbing 复制 3 份 + god-file（`gateway/run.py` 900KB）。

---

## 建议行动顺序

1. **P0 一周内**（成本都很低）：修失败测试 + 接 CI 门禁；WS/REST 加 Origin 白名单 + session token；kill 走进程组 + 兑现 SIGTERM→SIGKILL（顺手删谎言注释）。
2. **P1 协调正确性**：F3 加零订阅者 `warn` 诊断（最便宜的可观测性提升）；F13 传 writer id；F12 拆 channel + lag 后对账重放；F6 启动对账。
3. **P1 多 CLI L1+L2**：manifest 升级为行为描述符 + adapter trait + validate_all——这是把"未来加 CLI"成本从"改 3 处 Rust"降到"加 1 个 TOML"的关键，同时清掉 F7/F8/F9。
4. **P2 收尾**：存储 retention/VACUUM/索引；pre_spawn unwrap → `?`；删一套 DAG；http AbortController；拆 `Shell.tsx`。
5. **P3 决策**：spells/roles 复活还是删；bootstrap 抽公共函数；frontmatter toml-aware split。
6. **远期**：L3 golden 测试 / L4 ACP 传输 / L5 spawn 上限 + model 解耦。

---

### 附：本次审查产物
- 子系统测绘原始数据：`.tmp/wf1.json`（8 子系统 + 6 参考项目）
- 对抗验证裁决：`.tmp/wf2.json`（22 条，含证据/触发/缓解/修正严重度）
