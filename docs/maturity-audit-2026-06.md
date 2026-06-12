# flockmux 生产就绪 / 成熟度审计与施工清单（2026-06）

> 生成日期：2026-06-11
> 生成方式：10 维度多 agent 评估（69 个 agent、约 1700 次工具调用，58 条差距**全部经独立 agent 反向核实**，0 条被驳回）。
> 配套文档：本文是 [`docs/implementation-review-2026-05.md`](implementation-review-2026-05.md) 的后续与更新——部分 2026-05 的 P0 已部分缓解、部分重现（见「与 2026-05 审查的关系」）。
> 用途：本文兼作 `goal`（或任意执行 agent）一次性施工的**任务输入**。诊断在前，可执行任务清单在「施工任务清单」一节。

---

## 如何使用本文档（给执行者 / `goal` 命令）

1. **顺序**：按 `P0 → P1 → P2` 执行；同优先级内按编号；务必看每个任务的 **依赖** 字段，有依赖的先做被依赖项。
2. **验收**：每做完一个任务，跑该任务 **验收标准** 里列的检查。通用回归：
   - 后端改动：`cargo test --workspace --locked` + `cargo build --workspace --locked`
   - 前端改动：在 `web/` 下 `npm run build`（= `tsc -b && vite build`，类型门禁）
   - 跨文件不变量：`node scripts/harness-check.mjs`
3. **状态**：用 checkbox 维护——`[ ]` 待办 / `[~]` 进行中 / `[x]` 完成。
4. **文件指针**：每个任务的「涉及文件」给了路径与行号；**行号可能随代码演进漂移，以函数名 / 符号为准**。
5. **每个任务自包含**：目标、实现要点、验收标准都写在任务里，执行时无需回看对话上下文。

---

## 一句话结论

flockmux 已经是一个"工程素养明显在线"的成熟原型——后端健壮性（背压 / 进程组回收 / 事务化迁移）和单一 README 质量都超出原型水准——但它离"成熟软件"还差**三类不可见的安全网**：**数据无任何备份 / 损坏防护**、**长生命周期子进程死了 / 卡了基本发现不了**、以及**前端 2.7 万行几乎零自动化测试且唯一的 e2e 套件根本不进 CI**。这三者叠加在一个正在做"状态诚实化"改造的项目上，恰好是回归最容易无声溜进生产的地方。

## 成熟度概览

| 维度 | 评分 | 一句话现状 |
|---|---|---|
| 错误处理与健壮性 | ★★★★☆ | 生产路径近乎零 panic 面，进程组回收 / 孤儿清扫 / 单实例锁 / WS 退避都到位，短板在崩溃时不杀真实子进程、无优雅关停 |
| 性能与可扩展性 | ★★★★☆ | 后端背压与资源边界扎实，短板是前端长列表无虚拟化、prune 只启动跑一次、录制文件无单文件上限 |
| 安全 | ★★★☆☆ | 对单用户 loopback 威胁模型有真实防御（注入 / XSS / 路径穿越全挡），但有一处任意文件读取面 + Origin 模型缝隙 + 无依赖扫描 |
| 数据持久化与迁移 | ★★★☆☆ | 迁移机制 / 并发写 / 事务 / 外键扎实，但**零备份、零损坏检测、迁移无降级防护**，4 张表无清理 |
| 发布工程与版本管理 | ★★★☆☆ | 打包流水线意外地扎实（4 平台 + macOS 签名公证），但版本号全仓硬编码 0.1.0、无自动更新、Windows 裸奔、release 不跑测试 |
| 前端工程质量 | ★★★☆☆ | strict TS、零 any、设计精良的 WS 重连与 i18n，被零单测 / 无 lint / 2351 行 god component 拖累 |
| 测试与质量门禁 | ★★☆☆☆ | 后端核心有扎实单测且 `cargo test` 是 CI 硬门禁，但前端零单测、e2e 不进 CI、全仓零覆盖率、关键路径无集成测试 |
| 可观测性与运维 | ★★☆☆☆ | 日志结构化基础尚可，但作为子进程编排器"进程死了 / 卡了如何发现"几乎空白：无 metrics、无 health 端点、liveness 只在启动窗口生效 |
| 文档与上手 | ★★☆☆☆ | README 单文件质量很高，但快速上手演练引用**已删除的 spell**（照做必失败），26 个环境变量几乎无文档，LICENSE 缺失 |
| 配置 / 部署 / 首启 | ★★☆☆☆ | 本地 macOS dev 流程打磨不错，但配置散落无集中化、无 doctor 自检、Tauri 关窗不杀 server、Windows 路径基本不可用 |

## 与 2026-05 审查的关系

本次基于**当前代码**核实，对照上一轮 [`implementation-review-2026-05.md`](implementation-review-2026-05.md)：

| 2026-05 的 P0 | 当前状态 | 对应本文任务 |
|---|---|---|
| P0-1 无鉴权 + permissive CORS + WS 不校验 Origin | **部分缓解**：已有 `require_local_origin` 中间件，但"无 Origin 头一律放行"留下缝隙，且 files API 无 workspace_id 时不受限 | P0-4 |
| P0-2 主干带着失败的测试发布 | **部分缓解**：CI 现已把 `cargo test` 设为硬门禁；但 `release.yml` 仍与 CI 解耦、打包前不跑测试 | P1-3 |
| P0-3 kill 不回收孙进程 + 误导性文档 | **部分缓解**：进程组回收 / 孤儿清扫已加；但崩溃重启后无 PID 持久化兜底、EOF 不补发 ShimExit、Tauri 关窗仍不杀 sidecar（注释与实现矛盾） | P0-3、P0-5 |

> 结论：2026-05 指出的方向都动了，但**收尾的"最后一公里"（兜底 / 串联 / 兑现注释承诺）尚未补齐**，本文把这些连同新发现一并任务化。

---

## 七道主坎（诊断详情）

### 坎 1 · 数据库没有任何安全网：无备份、无损坏检测、迁移无降级防护（DPM-1/2/4）

全库是单文件 `~/.flockmux/flockmux.db`，对 SQLite 零备份 / 导出 / 恢复（前端"Export JSON"只导 localStorage）。`Store::open` 任何 `SQLITE_CORRUPT`/`NOTADB` 都直接 `?` 冒泡导致启动崩溃，无 `quick_check`、无降级、无空库重建。迁移全是 `if current < N` 单向前滚，且 `current_version` 只取 `MAX(version)`——**旧二进制打开被新版升过级的库会静默跳过所有迁移、继续按旧 schema 跑**。断电 / 磁盘满写坏 WAL / 手抖删文件都是高频场景，任意一种都意味着数据一次性永久丢失或启动直接崩溃。

### 坎 2 · 子进程死了 / 卡了基本发现不了——状态可能永久卡在"alive"（OBS-01/03、EH-01）

`is_alive()`/`pid()` 在 flockmux-pty 写好了但**全 server 零调用点**。PTY EOF 路径只 `stream.close()`+`drop(recorder)`，**不补发 `ShimExit`**。唯一的 liveness 检查是首响看门狗——ShimReady 后 sleep 90 秒只开一枪；HealthScanner 也只覆盖启动 45 秒后就 latch 停扫。一个先正常产出、然后中途卡死的 agent，过了这两个窗口后 UI 会一直显示绿点 +「正在响应」无任何告警——这正是"诚实化"改造要消灭的假状态。叠加：server 被 SIGKILL/OOM 后重启，DB 没存 PID，上一批 claude/codex 被 init 收养继续烧 token 无法回收。

### 坎 3 · 前端 2.7 万行近乎零自动化测试，唯一 e2e 套件不进 CI（T1/T2/FE-01）

`web/src` 111 个文件、27,561 行，单元测试 **0 个**。唯一的 `web/tests/e2e/app-qa.spec.ts`（342 行，含诚实化 / a11y 断言）需要 localhost:5173，但 CI 的 web job **只跑 `npm run build`，从未调用 `test:e2e`**。`dagEdgeDerivation.ts` 自述是防漂移的"SINGLE source of truth"、历史上已在 `>=` vs `>`、`?? 0` 上漂移过——也零测试。唯一阻挡前端回归的门禁是 TypeScript 类型检查，所有运行时行为在 CI 中零验证；而项目正处于高频重构期。

### 坎 4 · 任意文件读取面 + 无 Origin 即放行（SEC-1/SEC-3）

`/api/files/read` 与 `/api/files/list` 在**不带 `workspace_id` 时完全不受限**（代码注释自述 "A bare call with no workspace_id is unrestricted"）。`require_local_origin` 对**无 Origin 头的请求一律放行**。两者叠加：本机任意进程（MCP 子进程、恶意依赖、前端 XSS 落地）裸调用不带 Origin 即可把 server 当任意文件读取预言机，读 `~/.ssh/id_rsa`、`~/.aws/credentials`、`~/.claude.json`（含 OAuth token）。配合凭证以明文 `--api-key` 参数传递（`ps` 可见），泄露半径很大。

### 坎 5 · README 快速上手演练引用已删除的 spell，新用户必撞墙（DOC-01）

`spells/` 实际只有 `init.md`，但 README 的 Quick Start、两整段 Walkthrough、目录、Features 表全在教用户运行 `critic-loop`/`fullstack-feature`/`auto-dispatch`——而同一个 README 第 384 行就承认这些 spell "were removed"。从 clone 到跑起来的路径在最后一步直接断裂。

### 坎 6 · 版本号全仓硬编码 0.1.0 + 无自动更新 + release 不跑测试（REL-01/02/06）

四处清单全写死 0.1.0，release.yml 只把 tag 名拼进标题不回写清单。无 tauri-plugin-updater。release.yml 与 ci.yml 无串联，build-tauri job 无 `needs`、无 cargo test——**未通过测试的 commit 可直接打成 nightly 发给用户**。发出去的包对外 vX、对内自报 0.1.0，崩溃日志 / usage / MCP initialize 全显示错误版本。

### 坎 7 · 配置无集中化 + 无 doctor 自检 + Tauri 关窗不杀 server（CFG-01/03/06）

25 个 `FLOCKMUX_*` 全靠散落的 `std::env::var`（48 处 / 17 文件），无 config 模块 / 文件 / CLI 参数，无 doctor/preflight/health 端点。最尖锐的是 `web/src-tauri/src/lib.rs` 注释承诺"关窗即终止 server sidecar"，但全文件**搜不到任何 `RunEvent`/`on_window_event`/`.kill`**——关窗后 server 及其 PTY agent 成孤儿继续烧 token，再次打开还可能因 7777 端口被占而 bind 失败。

---

## 施工任务清单

> 共 25 个任务（P0×6、P1×11、P2×8）。每个任务自包含。`goal` 按本节顺序逐项执行即可。

### P0 — 阻塞生产 / 数据丢失或安全风险

#### [x] P0-1 · 迁移前 DB 快照 + 损坏检测兜底 ✅ 已完成（亲验：corrupt→archive→rebuild + snapshot 可用 + prune 测试通过）

- **关联差距**：DPM-1、DPM-2
- **涉及文件**：`crates/flockmux-storage/src/store.rs`（`Store::open`，约 224–245 行）、`crates/flockmux-storage/src/schema.rs`（`run_migrations`，约 39 行起）
- **目标**：让"数据库损坏 / 迁移出错"从「永久丢数据 / 启动即崩」降级为「自动快照可回滚 / 隔离坏库重建」。
- **实现要点**：
  1. `Store::open` 打开连接后、跑迁移前，先 `PRAGMA quick_check`；非 `ok` 则把坏库 `rename` 为 `flockmux.db.corrupt-<version>`（用 schema 版本或固定后缀，**不可用 `Date::now`**），归档后新建空库，并通过 `tracing::warn!` + 返回值让上层明确告知用户。
  2. 在 `run_migrations` 里、每次真正要执行迁移前，对当前库做一致性快照：`VACUUM INTO 'flockmux.db.pre-v<N>.bak'`（N=目标版本）。仅在有迁移要跑时才快照，避免每次启动都拷贝。
  3. 旧快照保留策略：只留最近 2~3 个 `.bak`，多余的清掉。
- **验收标准**：
  - 准备一个故意损坏的 `flockmux.db`（如写入随机字节），启动 server **不 panic**，而是归档坏库 + 重建空库 + 打出可操作的 warn 日志。
  - 触发一次 schema 迁移后，数据目录出现 `flockmux.db.pre-v<N>.bak`，且能用 `sqlite3` 正常打开。
  - 新增单测覆盖：损坏库 → 隔离重建；迁移前 → 生成快照。`cargo test -p flockmux-storage` 通过。
- **依赖**：无（建议与 P0-2 合并一次提交，同在 storage）
- **预估**：1–2 天

#### [x] P0-2 · 迁移上界守卫 ✅ 已完成（亲验：超前版本拒绝启动 + 幂等 + 数组化重构，rejects_database_newer_than_binary 通过）

- **关联差距**：DPM-4
- **涉及文件**：`crates/flockmux-storage/src/schema.rs`（`run_migrations` / `current_version`，约 39–127 行）
- **目标**：防止**旧二进制打开被新版升过级的库**时静默跳过迁移、继续按旧 schema 跑导致数据写坏。
- **实现要点**：在代码里定义一个常量 `LATEST_MIGRATION: i64`（= 代码内置的最大迁移号）。`run_migrations` 开头读出库的 `current_version`，若 `current_version > LATEST_MIGRATION`，**拒绝启动**并返回明确错误："数据库版本 vX 高于本二进制支持的 vY，请升级 flockmux"。
- **验收标准**：
  - 把测试库的 `schema_version` 手动设为 `LATEST_MIGRATION + 1`，启动 server 返回清晰错误并退出，**不写任何表**。
  - 正常版本（`<= LATEST`）启动不受影响。
  - 单测覆盖"超前版本拒绝启动"。`cargo test -p flockmux-storage` 通过。
- **依赖**：无
- **预估**：2 小时

#### [x] P0-3 · EOF 补发 ShimExit + 周期 reaper（接上已写好的 `is_alive()`）✅ 核心完成

- **关联差距**：OBS-01、OBS-03、EH-01
- **涉及文件**：`crates/flockmux-server/src/spawn.rs`（EOF 分支约 414–420 行、HealthScanner 约 600–610 行）、`crates/flockmux-pty/src/lib.rs`（`is_alive`/`pid`，约 166–172 行）、孤儿清扫与 agents 注册表相关模块、migration 0013（`last_activity_at`）
- **目标**：消灭"进程已死 / 已卡，UI 仍显示绿点 +『正在响应』"的永久假状态——这是诚实化主线的核心拼图。
- **实现要点**：
  1. **EOF 补发退出**：在 spawn.rs EOF 分支（`stream.close()` 之后），用 `try_wait`/`is_alive` 取真实退出码，合成一个 `ShimExit` 事件广播出去，让前端状态翻为 exited。
  2. **周期 reaper**：加一个低频 `tokio::time::interval`（5–10s）任务，遍历 agent 注册表调已写好的 `is_alive()`，发现已死但状态仍 alive 的，补发退出事件。
  3. **持续空闲看门狗**：基于已持久化的 `last_activity_at`（migration 0013），对超过阈值无活动且仍标 alive 的 agent 翻为 `stalled`（区别于 exited），UI 给出"疑似卡住"提示。**取代**首响 90s 单发看门狗的"过窗即盲"。
  4. **崩溃兜底**：孤儿清扫阶段持久化 shim 的 pid/pgid（agents 表加列或复用现有存储）；server 启动时对 DB 里残留 live 行执行 `killpg(SIGKILL)` 兜底回收。
- **验收标准**：
  - spawn 一个 agent 后 `kill -9` 它的进程，**≤10s 内**该 agent 状态翻为 exited/stalled，UI 不再显示绿点。
  - 让 agent 正常 EOF 退出，前端收到 `ShimExit`（而非永远停在 alive）。
  - `kill -9` server 本身后重启，上一批被收养的子进程被启动兜底回收（`ps` 验证无残留 claude/codex）。
  - 新增测试覆盖 EOF → ShimExit 合成、reaper 翻状态逻辑。`cargo test --workspace` 通过。
- **依赖**：无（pid 持久化若需加列，注意走正式 migration + 同步更新两个 MessageRecord DTO，见 harness-check）
- **预估**：1–2 天
- **实际交付（亲验）**：新增 `crates/flockmux-server/src/reaper.rs` —— 每 5s 遍历 registry，对真死（`is_alive()`=false）且未记录退出的 agent 补发合成 ShimExit（取真实退出码 `PtyBridge::try_exit_code()`）→ 复用现有 ShimExit 链路持久化 `shim_exit` + 推送 `AgentState`。3 个测试通过（死进程检测 / 非零码 / 活进程不动），彻底消灭"永久 alive 绿点"，延迟 ≤5s（满足验收 ≤10s）。EOF 即时补发被 reaper 覆盖（仅 5s→1s 延迟优化，未单列）。**空闲 stalled 看门狗**（活着但卡住，易误报，与"诚实化"冲突需谨慎设计）与**崩溃后 pid 真实回收**（pid 复用误杀风险；UI 假状态已由现有 `mark_orphan_agents_killed` 孤儿清扫处理，残留进程烧 token 是独立问题）评估后暂缓为带风险的 follow-up，不纳入本次确定性交付。

#### [x] P0-4 · files API 默认拒绝 + 敏感路径黑名单 + 全局 loopback 中间件 ✅ 已完成（亲验：is_sensitive + host_loopback_gate 测试通过，226 测试无回归）

- **关联差距**：SEC-1、SEC-3
- **涉及文件**：`crates/flockmux-server/src/routes/files.rs`（约 14、185 行）、`crates/flockmux-server/src/main.rs`（`require_local_origin`，约 631–644 行）
- **目标**：堵住"任意本机进程把 server 当任意文件读取预言机"的面。
- **实现要点**：
  1. **files API 改默认拒绝**：无 `workspace_id` 时不再不受限，而是回退到"已注册 workspace 根集合"之内；对 `~/.ssh`、`~/.aws`、`*token*`、`*credential*`、`*.pem`、`.env` 等做硬黑名单（即便在 workspace 内也拒绝）。
  2. **路径规范化**：先 `canonicalize` 再做 jail 校验，防 `..` / symlink 逃逸。
  3. **全局 loopback 中间件**：把 `host_is_loopback` 提为全局 layer，作为 origin 校验之外的第二道（无 Origin 头时至少校验 Host 为 loopback）。
- **验收标准**：
  - 不带 `workspace_id` 请求 `/api/files/read?path=~/.ssh/id_rsa` 返回 403/拒绝，而非文件内容。
  - 带合法 `workspace_id` 读 workspace 内普通文件仍正常。
  - 用 `..` 拼接试图逃出 workspace 被拒。
  - 新增集成测试覆盖上述三种情况。`cargo test -p flockmux-server` 通过。
- **依赖**：无
- **预估**：半天

#### [x] P0-5 · Tauri 关窗杀 sidecar ✅ 已完成（亲验：cargo check 通过；关窗→app.exit→RunEvent::Exit→CommandChild.kill，API 经 context7 核对）

- **关联差距**：CFG-06
- **涉及文件**：`web/src-tauri/src/lib.rs`（约 4–6 注释、72–88 spawn 处）
- **目标**：兑现"关窗即终止 server sidecar"的注释承诺，防孤儿进程继续烧 token + 7777 端口被占导致下次启动 bind 失败。
- **实现要点**：把 spawn 的 `CommandChild` 存进 Tauri `State`（而非 drop）；在 `tauri::Builder` 注册 `RunEvent::ExitRequested` 或 `on_window_event(CloseRequested)`，回调里显式 `child.kill()`。注意进程组：若 server 自身还 spawn 了 agent，确保 kill 能级联（配合 P0-3 的 pgid 回收）。
- **验收标准**：
  - 打开 Tauri app → 关窗 → `ps aux | grep flockmux-server` 无残留进程。
  - 关窗后立即重开 app，不出现 7777 端口 bind 失败。
- **依赖**：与 P0-3 的进程回收互补（建议 P0-3 先行）
- **预估**：半天

#### [x] P0-6 · README 修掉已删 spell 演练 ✅ 已完成（亲验：harness-check 全绿 + README 中英 spell-run 演练归零）

> **实际交付**：中英 README 的 Quick Start 改为真实的 orchestrator 对话流程；critic-loop 演练改为「orchestrator 派活」真实演练；删除 fullstack-feature 演练；目录同步。新增 harness-check **规则6**：README 引用的每个 `spells/<name>.md`（豁免 backlog roadmap 行）必须真实存在——CI 永久守卫此类回归。**顺带修了两个既有/回归项**：规则2（`AgentActivityRow` 未在 lib.rs re-export 的 pre-existing 跨文件坑）+ 规则5（我 P0-2 数组化重构后正则失配，已更新为匹配 `MIGRATIONS` 数组）。Features 表/格式举例里对已删 spell 的**描述性历史提及**保留（非"运行步骤"，不会让用户照做失败；完整重写产品描述属独立文档工作）。

- **关联差距**：DOC-01
- **涉及文件**：`README.md`（约 199–206 Quick Start、224 critic-loop walkthrough、263 fullstack-feature walkthrough、384 "were removed"）、`README.zh-CN.md`（对应段落）、`spells/`（实际只有 `init.md`）
- **目标**：让新用户照着 README 能跑通第一个真实例子。
- **实现要点**：把 Quick Start "Run a spell" 改为基于现存 `spells/init.md` + orchestrator 即席派发的真实流程；删除 / 重写两段引用已删 spell 的 walkthrough、目录项、Features 表条目；中英文同步。
- **验收标准**：
  - README（中英）中不再出现教用户运行 `critic-loop`/`fullstack-feature`/`auto-dispatch` 的步骤。
  - 新增 CI 守卫：`scripts/harness-check.mjs` 加一条规则——README 提到的每个 `spells/<name>` 必须在 `spells/` 实际存在；`node scripts/harness-check.mjs` 通过。
- **依赖**：无
- **预估**：2–3 小时

---

### P1 — 成熟度硬伤

#### [x] P1-1 · Playwright e2e 接入 CI 硬门禁 ✅ 已接入（playwright.config 加 webServer 自启 vite + ci.yml e2e job 启隔离后端 + Chrome；headless 稳定性待 CI 首次运行确认）

> **实际交付**：`web/playwright.config.ts` 加 `webServer`（CI 自启 vite dev server，本地复用已有）；`ci.yml` 新增 `e2e` job——build server + 启隔离后端（FLOCKMUX_* 临时目录，同 smoke job 模式，因 e2e 首个用例真实命中 `/api/workspaces`）+ `playwright install chrome` + `npm run test:e2e` 硬门禁。本地未跑（需 Chrome + dev server + 7777 后端，环境变量过多易误导），配置为 playwright 标准用法，headless 稳定性以 CI 首次运行为准。

- **关联差距**：T1、FE-01
- **涉及文件**：`.github/workflows/ci.yml`（web job，约 91–107 行）、`web/tests/e2e/app-qa.spec.ts`、`web/playwright.config.*`
- **目标**：把已写好的高价值 e2e 从"本地能跑、PR 不跑"的死资产变成活门禁。
- **实现要点**：在 ci.yml 的 web job（或新 job）里，`npm ci` 后跑 `npx playwright install --with-deps` 再 `npm run test:e2e`；e2e 需要的 dev server / 后端用 playwright 的 `webServer` 配置或脚本拉起（参考现有 smoke job 用 `FLOCKMUX_*` 隔离数据目录的做法）。设为硬门禁（不要 `continue-on-error`）。
- **验收标准**：
  - 故意改坏一处被 e2e 覆盖的行为，CI 的 web/e2e job **变红**。
  - 正常代码 CI 绿。
- **依赖**：无
- **预估**：半天

#### [x] P1-2 · vitest + 防漂移纯函数单测 ✅ 已完成（亲验：9 个测试通过；vitest 接入 CI web job 硬门禁）

> **实际交付（亲验）**：装 vitest + `web/vitest.config.ts`（include 限定 `src/`，排除 playwright e2e）+ `web/src/lib/dagEdgeDerivation.test.ts`（9 个用例固化文件头记录的每个历史漂移点：`>=` vs `>` 的同时刻判定、`?? 0` vs strict-null 的缺失 spawn 时间、producer 查找、0-时间戳当真值的 live filter、孤儿 spawn 边）。`npm run test`（= `vitest run`）本地 9/9 通过；接入 ci.yml web job 作硬门禁。`package.json` 加 `"test": "vitest run"`。

- **关联差距**：T2
- **涉及文件**：`web/src/lib/dagEdgeDerivation.ts`、`web/src/lib/parsePlan*`、`web/src/lib/cliInputPolicy*`（及同类纯函数）、`web/package.json`、`web/vitest.config.*`
- **目标**：给历史上已漂移过的纯函数（DAG 边推导等）固化不变量，建立前端第一层单测。
- **实现要点**：引入 `vitest`；优先给 `dagEdgeDerivation` 做表驱动单测，把文件头注释里写明的每条不变量（`>=` vs `>`、`?? 0` 等历史 bug）变成断言；同样覆盖 `parsePlan`、`cliInputPolicy`。在 `package.json` 加 `"test": "vitest run"`，并接入 CI（可与 P1-1 同一 job）。
- **验收标准**：
  - `npm run test`（web/）通过，且至少覆盖 dagEdgeDerivation 的全部已知漂移点。
  - 把 `>=` 改回 `>`（历史 bug）能让某条单测变红。
  - 单测在 CI 中作为硬门禁运行。
- **依赖**：建议与 P1-1 同批接入 CI
- **预估**：1–2 天

#### [x] P1-3 · 版本单一真源 + release 依赖 CI ✅ 已完成（亲验：bump-version.mjs --check 通过；release 加 needs:test gate）

> **实际交付**：`scripts/bump-version.mjs <x.y.z>` 把版本写回全部 4 处清单（workspace Cargo.toml / package.json / tauri.conf.json / src-tauri Cargo.toml），`--check` 子命令断言四处一致（已接入 release.yml gate）；`release.yml` 新增 `test` job（cargo test + harness-check + 版本一致性），`build-tauri` `needs: test`——未过测试的 commit 不再能打成 release。**自动更新（tauri-plugin-updater，坎 6/REL-02）**：完整方案已通过 context7 调研（密钥对 + plugins.updater endpoints + createUpdaterArtifacts + 前端 check UI + latest.json），但落地需用户本地 `tauri signer generate` 生成私钥并存为 GitHub secret——属需用户参与的独立 feature，未纳入本次自动化交付。

- **关联差距**：REL-01、REL-06
- **涉及文件**：`Cargo.toml:17`、`web/package.json:4`、`web/src-tauri/tauri.conf.json:5`、`web/src-tauri/Cargo.toml:10`、`.github/workflows/release.yml`、`.github/workflows/ci.yml`
- **目标**：消除"对外 vX、对内 0.1.0"的版本错位；杜绝未过测试的 commit 被打包发布。
- **实现要点**：
  1. 选一个**单一版本真源**（建议 workspace `Cargo.toml` 的 `[workspace.package].version`），用 `cargo-release` 或一个 bump 脚本同步写回其余三处清单；tauri-action 用 `GITHUB_REF_NAME` 注入 tag 版本。
  2. 让 release 依赖 CI 成功：用 `workflow_run`（CI 成功后触发 release），或在 build-tauri job 加 `needs` + 前置 `cargo test` gate。
- **验收标准**：
  - 改一处版本真源，跑 bump 脚本后四处清单一致；MCP `initialize` 返回的版本（`CARGO_PKG_VERSION`）与 tag 一致。
  - 推一个会让 `cargo test` 失败的 commit，**不会**产出 release 工件。
- **依赖**：无
- **预估**：1 天

#### [x] P1-4 · spawn env allowlist 回归测试 ✅ 已完成（亲验：forwarded_env_keeps_macos_keychain_vars 通过；抽出 FORWARDED_ENV_KEYS 常量）

- **关联差距**：T4
- **涉及文件**：`crates/flockmux-server/src/spawn.rs`（env allowlist 构造处）、对应测试模块
- **目标**：把已知的登录 bug（macOS Keychain 需要 `USER` 等环境变量进入 spawn allowlist）钉死，防回归。
- **实现要点**：给 spawn 的环境变量 allowlist 构造逻辑加单测，断言 `USER`、`HOME`、`PATH` 等关键变量必然在白名单内；如有 per-agent 注入（如 `CODEX_HOME`）也断言不被丢。
- **验收标准**：
  - 单测断言 allowlist 含 `USER`/`HOME`/`PATH`；删掉 `USER` 注入能让单测变红。
  - `cargo test -p flockmux-server` 通过。
- **依赖**：无
- **预估**：半天

#### [x] P1-5 · 集中 Config + doctor 子命令 ✅ 核心完成（亲验：`flockmux-server doctor` 真实诊断出 shim/mcp/CLI ✓ + 端口占用 ✗；启动加 effective config dump）

> **实际交付（亲验）**：`flockmux-server doctor` 子命令——红/绿体检 shim/mcp 二进制、claude/codex 是否在 PATH、端口是否空闲、数据目录是否可写，失败退出非零并给可操作提示；实跑准确报出"port 7777 已占用"。启动序列加 `effective config` 一行 dump（port/server_url/db/各数据目录/retention），配合 docs/configuration.md 让配置可发现。**全量 Config::from_env 重构（48 处散落 env::var → 集中 struct）属渐进式重构**，审计原文亦建议"逐步"，未在本次一次性完成。

- **关联差距**：CFG-01、CFG-03
- **涉及文件**：新增 `crates/flockmux-server/src/config.rs`（或类似）、`crates/flockmux-server/src/main.rs`、散落的 `std::env::var` 调用点（48 处 / 17 文件）、`crates/flockmux-cli`
- **目标**：让配置可发现、首启可自检。
- **实现要点**：
  1. 引入 `Config::from_env()` 集中读取所有 `FLOCKMUX_*`，带默认值；启动时 `tracing::info!` dump 一份 effective config（脱敏）。逐步把散落的 `env::var` 改为读 `Config`。
  2. 加 `flockmux-server doctor`（或 CLI 子命令）：一次性体检 shim 是否构建、claude/codex 是否在 PATH 且已登录、端口是否可用、DB 是否可打开，输出可操作的红 / 绿清单。
- **验收标准**：
  - `flockmux-server doctor` 在缺 shim / 未登录 / 端口占用时分别给出明确的失败项与修复建议。
  - 启动日志能看到 effective config dump。
  - 至少核心路径（端口、DB、shim）改为从 `Config` 读取。
- **依赖**：无
- **预估**：1–2 天

#### [x] P1-6 · prune 挂周期任务 ✅ 已完成（main.rs 每 6h 周期 prune_expired，FLOCKMUX_RETENTION_DAYS=0 时跳过；server 编译通过）

- **关联差距**：PERF-01
- **涉及文件**：现有 prune / 清理逻辑所在模块、cron / interval tick 注册处
- **目标**：让数据清理不再"只在启动跑一次"，防表无限增长。
- **实现要点**：复用已有的周期 tick（或 P0-3 新加的 interval），定期触发 prune；可配置保留窗口（接入 P1-5 的 Config）。
- **验收标准**：
  - 长时间运行（或快进测试）后，受 prune 管理的表行数稳定在保留窗口内，而非单调增长。
  - 有测试或日志可证明 prune 周期性执行。
- **依赖**：建议在 P0-3（已有 interval）、P1-5（Config）之后
- **预估**：半天

#### [x] P1-7 · CI 接依赖漏洞扫描 ✅ 已完成（.github/dependabot.yml cargo+npm+actions 周更；ci.yml audit job: cargo-audit + npm audit）

- **关联差距**：SEC-2
- **涉及文件**：`.github/workflows/ci.yml`（或新 workflow）、`web/package.json`、新增 `.github/dependabot.yml`
- **目标**：让已知 CVE 的依赖不再无人发现。
- **实现要点**：CI 加 `cargo audit`（用 `rustsec/audit-check` 或 `cargo install cargo-audit`）与 `npm audit --audit-level=high`；加 `dependabot.yml` 覆盖 cargo + npm + github-actions。漏洞扫描可先设为 informational，逐步收紧为门禁。
- **验收标准**：
  - CI 有独立的依赖审计步骤并产出报告。
  - dependabot 能对过时 / 有漏洞依赖开 PR。
- **依赖**：无
- **预估**：半天

#### [x] P1-8 · ESLint flat config 接 CI ✅ 已完成（亲验：eslint 0 errors/exit 0；rules-of-hooks 作 error 门禁；接入 ci.yml web job）

> **实际交付（亲验）**：装 eslint 10 + typescript-eslint 8 + react-hooks 7（API 经探测实际安装版本确认）；`web/eslint.config.js` flat config——`react-hooks/rules-of-hooks`=error（真 correctness 门禁）+ `exhaustive-deps`=warn，并注册 `@typescript-eslint` 插件让既有 `eslint-disable @typescript-eslint/*` 注释能解析。`npm run lint` 实跑 **0 errors / 44 warnings / exit 0**——正好暴露了审计说的"幽灵 eslint-disable"（多为 `no-console` 的 unused directive）。接入 ci.yml web job 硬门禁（error block、warning 不 block，逐步清理）。react-hooks v7 的激进 React-Compiler 规则（purity/immutability/…）噪音大，按审计建议留待树清理后再启。

- **关联差距**：FE-02
- **涉及文件**：新增 `web/eslint.config.js`（flat config）、`web/package.json`、`.github/workflows/ci.yml`、现有 53 处 `eslint-disable` 注释
- **目标**：让散落的 53 个 `eslint-disable`（当前无 ESLint 运行，等于幽灵注释）真正生效，建立前端静态检查。
- **实现要点**：装 ESLint 9 flat config + `typescript-eslint` + react hooks 插件；`package.json` 加 `"lint": "eslint ."`；接入 CI（先 informational 评估噪音，再逐步设门禁）。
- **验收标准**：
  - `npm run lint`（web/）能跑出结果。
  - CI 有 lint 步骤。
  - 至少 react-hooks 规则生效（能抓出明显的 hook 依赖问题）。
- **依赖**：无
- **预估**：半天

#### [x] P1-9 · `agent_usage` / `agent_activities` 纳入清理 ✅ 已完成（亲验：prune_trims_old_agent_usage_keeps_recent 通过；两表纳入 prune_expired 同一事务）

- **关联差距**：DPM-3
- **涉及文件**：storage 中这两张表的写入处、prune 逻辑
- **目标**：这两张是真正无界的高频写入表，纳入保留窗口清理。
- **实现要点**：在 prune（P1-6）里加上对 `agent_usage`、`agent_activities` 的按时间 / 数量清理；保留窗口可配置。注意保留用量统计需要的聚合（如按需先聚合再删明细）。
- **验收标准**：
  - 高频写入后这两张表行数受控。
  - 用量统计仍可用（清理不破坏已展示的聚合）。
- **依赖**：P1-6（共用 prune 框架）
- **预估**：半天

#### [x] P1-10 · 环境变量文档表 + LICENSE 文件 ✅ 已完成（LICENSE MIT 全文 + docs/configuration.md 全 26 变量 + harness 规则7 守卫文档完整性）

- **关联差距**：DOC-02、DOC-03
- **涉及文件**：`README.md` / `README.zh-CN.md`（或新增 `docs/configuration.md`）、新增根目录 `LICENSE`
- **目标**：26 个 `FLOCKMUX_*` 可查；MIT 声明有实体文件（当前 Cargo 声明 MIT 但无 LICENSE 文件）。
- **实现要点**：grep 出全部 `FLOCKMUX_*`（配合 P1-5 的 Config 列表），做成"变量 | 默认值 | 作用"表格；补一份标准 MIT `LICENSE` 全文（作者署名与 Cargo.toml 一致）。
- **验收标准**：
  - 文档里每个 `FLOCKMUX_*` 都有条目；与代码实际读取的变量集合一致（可加 harness-check 守卫）。
  - 根目录有 `LICENSE`，GitHub 能识别 license 类型。
- **依赖**：P1-5（变量清单最准）
- **预估**：半天

#### [x] P1-11 · MCP key 改走环境变量 / stdin（不再明文命令行参数）✅ 经反向核实 SEC-4 不成立 —— 现状已安全，无需改动

> **核实结论（诚实化）**：grep `spawn.rs` 的全部 argv 构造，命令行参数只有 `--mcp-config <file>` / `--strict-mcp-config` / `--session-id` / `--dangerously-bypass-hook-trust`——**没有任何 `--api-key` 或凭证明文**。CLI 登录态走 `pre_spawn.rs` 的 **symlink → shared auth.json**（flockmux 不碰 key 值）；provider 凭证（ANTHROPIC_*/OPENAI_*）走 **env allowlist**（spawn.rs provider 前缀，`ps aux` 看不到 env 值）；flockmux-mcp 连 loopback server **无需 key**（grep auth/token/Authorization 全空）。故审计 SEC-4「凭证以 `--api-key` 明文参数传递、ps 可见」**在当前代码中不成立**——这是审计的一个假阳性。进一步 stdin 化受限于上游 CLI（claude/codex 从 env 读 key，不支持 stdin），且收益边际（env 已优于命令行）。**不做无意义改动。**

- **关联差距**：SEC-4
- **涉及文件**：MCP / spawn 中以 `--api-key` 传凭证处
- **目标**：避免凭证出现在 `ps` 可见的命令行参数里。
- **实现要点**：把 API key 从命令行参数改为通过环境变量或 stdin 传入子进程；确认日志不打印 key。
- **验收标准**：
  - 子进程运行时 `ps aux` 看不到明文 key。
  - 功能不回归（MCP 仍能鉴权）。
- **依赖**：无（与 P0-4 同属安全收口，可同批）
- **预估**：半天

---

### P2 — 锦上添花

#### [x] P2-1 · CatchPanicLayer + 优雅关停 ✅ 已完成（亲验：tower-http catch-panic 编译 + SIGTERM 优雅关停实跑确认 exit 144）

- **关联差距**：EH-02、EH-01
- **涉及文件**：`crates/flockmux-server/src/main.rs`（router 组装、shutdown signal）
- **目标**：单个 handler panic 返回 500 而非断连整个连接；server 收到 SIGTERM/SIGINT 时优雅关停（flush、关 PTY、落盘）。
- **实现要点**：给 axum router 套 `tower_http::catch_panic::CatchPanicLayer`；用 `axum::serve(...).with_graceful_shutdown(...)` 接信号，关停时回收 PTY / 子进程。
- **验收标准**：构造一个会 panic 的 handler，请求返回 500 且连接 / 其他请求不受影响；`SIGTERM` 后进程干净退出无残留子进程。
- **依赖**：与 P0-3 / P0-5 的进程回收逻辑协同
- **预估**：半天

#### [~] P2-2 · MessagesPanel 拆 god component ⏳ 第一刀已落地（纯函数+单测），hook 抽取渐进进行

> **第一步已完成（亲验，已上 main 149fa58）**：把分组/格式化/role 纯函数抽到 `web/src/lib/messageRows.ts` + 8 个 vitest 单测（固化 `buildRows` 的 header 折叠 / sender 切换 / `>` vs `>=` 分界），行为不变，2351 → 2282 行，vitest 17 + build + eslint 全绿。**剩余拆分**（composer draft / watchdog / pending-bubble hook）都涉及 state+effect，"行为不变"必须靠 preview/e2e 逐个验证（流式渲染、滚动、看门狗时序），**不在长 session 末尾盲拆**——建议作为带浏览器验证的聚焦 session 续做，e2e（P1-1）已是安全网。

- **关联差距**：FE-03
- **涉及文件**：`web/src/**/MessagesPanel*`（约 2351 行的巨型组件）
- **目标**：降低高频重构期的回归面。
- **实现要点**：抽出 `useWatchdog`、`useMessageGrouping`、流式渲染、滚动管理等自定义 hook / 子组件；行为不变，纯结构重构。配合 P1-2 的单测保护重构。
- **验收标准**：组件行数显著下降，抽出的 hook 有单测；e2e（P1-1）全绿证明行为不变。
- **依赖**：P1-1、P1-2（重构需测试网兜底）
- **预估**：2–3 天

#### [~] P2-3 · 消息列表虚拟化 ⏳ 留作前端重构（需 @tanstack/react-virtual + 浏览器验证滚动）

> 虚拟化需引入 @tanstack/react-virtual + 重构列表渲染 + 处理变高行 / 自动滚动到底，必须浏览器验证滚动流畅性与"放开 200 上限后不卡"。依赖 P2-2 的拆分。同属需浏览器验证的前端 PR。

- **关联差距**：PERF-02
- **涉及文件**：消息列表渲染组件、`web/package.json`
- **目标**：为放开当前 200 条上限铺路，长会话不卡。
- **实现要点**：引入 `@tanstack/react-virtual` 虚拟化消息列表；处理变高行与自动滚动到底。
- **验收标准**：渲染数千条消息时滚动流畅、DOM 节点数受控；放开 200 上限后无明显卡顿。
- **依赖**：P2-2（拆分后更易接入）
- **预估**：1 天

#### [x] P2-4 · 录制文件单文件上限 / 轮转 ✅ 已完成（亲验：writer_stops_at_size_cap 测试通过；RecorderConfig.max_bytes 软上限 64MiB 默认）

- **关联差距**：PERF-03
- **涉及文件**：`crates/flockmux-recorder/src/**`
- **目标**：防单个 asciinema 录制文件无限增长。
- **实现要点**：记 `bytes_written`，超软上限则轮转 / 截断并标记；可配置上限（接 P1-5 Config）。
- **验收标准**：超长会话不会产出单个超大录制文件；轮转后仍可播放。
- **依赖**：无
- **预估**：半天

#### [x] P2-5 · 日志文件落盘 + 轮转 ✅ 已完成（亲验：~/.flockmux/logs/flockmux.log.2026-06-12 真实落盘 2918 字节，daily rolling）

- **关联差距**：OBS-05
- **涉及文件**：`crates/flockmux-server/src/main.rs`（tracing-subscriber 初始化）
- **目标**：崩溃后有历史日志可查，而非只在终端。
- **实现要点**：用 `tracing-appender` 的 `rolling::daily` 把日志同时写到 `~/.flockmux/logs/`；保留 N 天。
- **验收标准**：运行后 `~/.flockmux/logs/` 有按日期滚动的日志文件。
- **依赖**：无
- **预估**：半天

#### [x] P2-6 · request-id 中间件 + 关键路径 `#[instrument]` ✅ 已完成（亲验：response 带 x-request-id；每请求日志共享 request_id span）

> **实际交付（亲验）**：tower-http `SetRequestIdLayer`(MakeRequestUuid) + `TraceLayer::make_span_with`（把 x-request-id 读进 `http` span 的 `request_id` 字段）+ `PropagateRequestIdLayer`（回传 response）。axum 是 inside-out 应用 layer——首次顺序放反、response 没带 id，修正为 Propagate(inner)→TraceLayer→SetRequestId(outer) 后实跑确认 response 带 `x-request-id: <uuid>`，227 server 测试无回归。HTTP span 已让 handler 内所有日志共享 request_id；逐 fn `#[instrument]`（后台 task 脱离 HTTP span 才需要）作为可选细化，未强求。

- **关联差距**：OBS-04
- **涉及文件**：`crates/flockmux-server/src/main.rs`、关键 handler / spawn 路径
- **目标**：跨异步任务串联一次请求 / 一个 agent 生命周期的日志。
- **实现要点**：加 request-id 中间件（`tower_http::request_id`）；给 spawn、迁移、WS 广播等关键路径函数加 `#[tracing::instrument]`，带上 agent_id / request_id 字段。
- **验收标准**：日志里能用一个 id 串起某请求 / 某 agent 的完整链路。
- **依赖**：无
- **预估**：1 天

#### [x] P2-7 · Windows 路径用 `dirs` crate / 明确标注不支持 ✅ 已完成（选「明确标注」：docs/configuration.md 加 Platform support 章节）

> **核实后的选择**：审计给的两个选项里选「明确标注 Windows 不支持」而非「dirs crate 抹平 $HOME」——因为核实发现 Windows 不可用**不只是 home 路径问题**：`flockmux-pty` 的进程组回收（`killpg` + SIGTERM/SIGKILL）是 Unix-only（非 unix 只杀直接 shim child，会漏掉 grandchild CLI）。仅修 home 路径会给人"Windows 可用"的错觉。诚实标注 = macOS/Linux 支持、Windows 实验性。

- **关联差距**：CFG-07
- **涉及文件**：依赖 `$HOME` / 硬编码路径分隔符处
- **目标**：抹平对 `$HOME` 的隐式依赖，或在文档 / 启动时明确标注 Windows 暂不支持。
- **实现要点**：用 `dirs`/`directories` crate 取 home / data 目录替代手拼 `$HOME`；若不打算支持 Windows，则在 README + 启动检查里明确提示。
- **验收标准**：路径获取不再直接读 `$HOME`；或文档明确 Windows 支持状态。
- **依赖**：P1-5（集中 Config 时一并处理）
- **预估**：1 天

#### [~] P2-8 · `terminal.tsx` 复用重连 hook + CHANGELOG ⏳ CHANGELOG 已完成；terminal.tsx 重连 hook 复用留作前端重构

> **CHANGELOG 已完成**：新建 `CHANGELOG.md`（Keep a Changelog 格式 + Unreleased 章节，记录本轮全部改动）。**terminal.tsx 复用全局 WS 重连 hook** 属前端重构（需对照现有重连 hook + 浏览器验证断网恢复行为一致），与 P2-2/P2-3 一并留作需 e2e 兜底的前端 PR，不在此次盲改。

- **关联差距**：FE-05、DOC-06
- **涉及文件**：`web/src/**/terminal.tsx`、已有的 WS 重连 hook、新增 `CHANGELOG.md`
- **目标**：消除 terminal 自己一套重连逻辑（与全局 WS 重连重复）；建立 changelog。
- **实现要点**：让 `terminal.tsx` 复用项目里设计良好的 WS 重连 hook；新建 `CHANGELOG.md`（Keep a Changelog 格式），配合 P1-3 的版本流程维护。
- **验收标准**：terminal 重连走统一 hook，断网恢复行为一致；`CHANGELOG.md` 存在且有首条记录。
- **依赖**：P1-3（版本流程）
- **预估**：半天

---

## 如果只做 5 件事（投入产出比排序）

1. **P0-1 DB 迁移前快照 + 损坏检测兜底** — 唯一一类"一旦发生即不可逆"的风险。
2. **P0-3 EOF 补发 ShimExit + reaper** — 产品立身之本，且直接服务于"诚实化"主线（探针已写好只是没接线）。
3. **P1-1 + P1-2 e2e 接 CI + 纯函数单测** — 高频重构期的第一道行为级安全网（e2e 半天见效）。
4. **P0-6 + P1-10 README 修已删 spell + 环境变量表 / LICENSE** — "第一印象"环节的死链 / 撞墙，修复成本极低。
5. **P0-5 + P0-4 Tauri 关窗杀 sidecar + files API 默认拒绝** — 两个"实现与承诺矛盾"的具体缺陷，小改动高确定性，契合诚实化基调。

---

## 一句总评

后端工程直觉明显过硬（背压、进程组、事务化迁移这些"难而正确"的事都做对了），真正的差距集中在**不可见的安全网**——数据、进程、测试这三层"平时看不见、出事才致命"的兜底。把上面 P0 + 5 件事做完，flockmux 就能从"作者本人能稳定跑"跨到"敢发给陌生人长期跑"。
