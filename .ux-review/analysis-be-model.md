# 后端数据模型 / 生命周期 / API 能力分析（工作空间重设计弹药库）

来源：深读 `crates/swarmx-server/src`（routes/、spawn.rs、wake.rs、pre_spawn.rs、transcript.rs、registry.rs）与 `crates/swarmx-storage`（migrations 0001–0021、models.rs、store.rs）。对照 `.ux-review/firsthand-findings.md` 的 P0「状态撒谎」问题逐条核实。

---

## ① 数据层实体与生命周期状态机

### 1. workspace（`workspaces` 表，migration 0004）
- 字段：`id, slug, name, cwd, accent, created_at, deleted_at`。**纯元数据行**：一个"主项目目录 + 名字 + 颜色"。
- 生命周期：创建（校验 cwd 存在且是目录，否则 4xx）→ 活跃 → **软删除**（`deleted_at`）。
- ⚠️ 关键语义：`DELETE /api/workspaces/:id` **故意不杀 agent**（workspaces.rs:1583 注释"by design"）。删除空间后其 agent 继续在后台跑、烧 token，UI 完全失明。
- 创建时自动建 `main` 方向（shared/ready，cwd=workspace cwd）；main 不可删。

### 2. 源码上下文（`workspace_roots` 表，0006/0007）
- 字段：`path, role(project|tool|dependency), label, parent_id`。是**逻辑树**（parent_id 指向另一 root），非物理嵌套。
- 服务端把整棵树渲染成 swarmx 托管块写进主项目的 **CLAUDE.md + AGENTS.md**（HTML 注释定界，幂等替换；最后一个 root 删除时整块剥离，若整文件是 swarmx 写的则删文件并 git 本地 exclude）。**即"源码上下文"的真实作用 = 给 orchestrator 的上下文文件注入**，UI 从未解释这一点。
- 有 `GET /api/workspaces/:id/root-suggestions`：扫 package.json file:/link:、Cargo.toml path、go.mod replace、pyproject uv sources、pom.xml 模块/兄弟工程，自动推荐可挂载依赖 —— 现有 UI 基本没用上的能力。

### 3. 方向 = thread（`threads` 表，0009/0014/0015）
- 字段：`slug`（**创建即冻结**，是黑板 key 命名空间 `<ws_id>/<slug>/…` 的一段）、`name`（显示名，可改）、`isolation`、`branch`、`cwd`、`state`、`model_tier`、`reasoning_effort`、`deleted_at`。
- **isolation 状态机**（UI 没讲清的核心）：
  - `shared`：未隔离，cwd=主项目（未命名方向的默认）。
  - `worktree`：已 git worktree 隔离，cwd=worktree 目录，branch=slug（或挂已有分支）。
  - `degraded`：**尝试隔离但失败**（非 git 项目/git 出错），实际仍共享主 cwd —— 服务端特意区分这个值就是为了让 UI 警告"你以为隔离了其实没有"（workspaces.rs:1350 注释），两个方向的 agent 会互踩文件。
- **state 状态机**：`ready` ⇄ `preparing`（后台 git takeover + worktree add 期间）；失败→degraded+ready。
- **命名即隔离 + 重栽 orchestrator（P5-D）**：PATCH 改名一个非 main、未隔离方向 → preparing → 后台建 worktree → 成功后**杀掉该方向全部活 agent**、在新 cwd 重跑 `init` spell、把未读用户消息 reassign 给新 orchestrator、把最近一条用户消息 seed 成新 orchestrator 的 `{task}`。UI 看到的现象是"成员闪断重生"，没有任何解释性事件——"状态撒谎"的一个来源。
- 删除方向：先杀该 thread 全部 agent → 软删 → 删黑板前缀 `<ws>/<slug>` → 后台删 worktree + 分支。
- 合并回主线：`GET …/diff`（base/branch/files/base_dirty 预览）、`POST …/merge`（干净→Merged；冲突→**自动 spawn 一个 merge-resolver agent** 在主 worktree 解冲突，返回 Resolving{agent_id, files}）。
- `/api/workspaces` 列表会现场算每个方向的 git `dirty/branch/ahead/behind`（3s TTL 缓存）。

### 4. agent（`agents` 表 + 内存 Registry）
- 表字段：`id(cli-uuid8), cli, role, workspace(cwd串), spawned_at, killed_at, shim_ready_at, shim_exit_at, shim_exit_code, workspace_id, spell_run_id, thread_id, last_activity_at`。
- 内存 `AgentSlot`：PtyBridge、PtyStream 重放缓冲、`Lifecycle{shim_ready, shim_exit}`（OSC 633;A / 633;D 扫描）、`paused` 原子位、`mcp_ready` watch。
- **服务端实际广播的 AgentState 只有 4 个**：`Spawning`（spawn 时）、`Ready`（ShimReady）、`Error`（shim 非 0 退出）、`Exited`（0 退出/被杀）。协议里还定义了 `Thinking / Idle / WaitingDep` —— **grep 证实从未被 publish**。前端"绿点在线"的全部依据只是"shim 拉起了 CLI 进程"，与"agent 能干活"零相关。这就是 P0 撒谎的机制根源。
- 两类 agent：
  - **orchestrator**：`init` spell 拉起，不在 workers 表（DAG 树根，parent=None）。
  - **worker**（`workers` 表，0005/0011/0017）：orchestrator 经 MCP `swarm_spawn_worker` 拉的临时工。行内有 `parent_agent_id`、`role_slug`（角色注册表校验，带 did-you-mean 400）、`system_prompt` 全文留档、**服务端铸造**的 `handoff_signal = <ws>/<slug>/<role>.<kind>`、`depends_on_json`（由 typed consumes 解析铸造）、`produces/consumes_json`、`task_status`（看板人工覆盖）。
- **worker 生命周期（= 全局"任务"页的本体，tasks.rs 纯函数推导）**：人工覆盖 > `.error` 在黑板→`blocked` > handoff 已写→`done` > killed→`archived` > 有 last_activity→`running` > `todo`。**工作台账（blackboard ledger）与全局任务页（workers 表）是两套东西**——前者是 orchestrator 自由书写的 markdown，后者是结构化派工记录，UI 概念混淆有数据层依据。

### 5. agent 启动管线（bootstrap，"撒谎"的完整事实链）
spawn_with_bookkeeping → spawn_agent 流程：
1. **fork-bomb 双闸**：活 agent ≤256（SWARMX_MAX_LIVE_AGENTS）、委派深度 ≤6。
2. **CLI 选择 + 自动回退**：请求的 CLI 未安装→自动换装好的（codex 优先），仅 `tracing::warn`，**响应里不告诉前端发生了回退**。
3. pre_spawn 补丁：trust 项目、消 update 弹窗、写 per-agent MCP 配置（claude `--mcp-config --strict-mcp-config`；codex 隔离 CODEX_HOME + auth.json 软链）、注入 Stop hook、claude 强制 `--session-id`（给转录 tailer 定位）。
4. PTY 起进程（空环境+白名单 env），录像（asciicast）开写。
5. **ShimReady**（OSC）→ 持久化 + 广播 Ready ← 绿点至此点亮，*但 CLI 可能正打印 "Not logged in"*。
6. `ready_plan`：插件清单声明的 needle/response 自动应答状态机（codex "Hooks need review"→"2\r"）。**通用的"扫 PTY 输出找特征串"基础设施已存在**——完全可以加 "Not logged in / Please run /login / rate limit" needle 做登录态检测，但今天没做。
7. **mcp_ready**：agent 自己的 swarmx-mcp 回 ping `POST /api/agent/:id/mcp-ready`；6s 兜底。仅内存 watch + debug 日志。
8. **P1-D readiness gate**：worker 的 deps 没齐时第一条 prompt 不注入，750ms 轮询黑板（带 spawned_at 新鲜度校验防陈旧 key），300s 兜底强注。日志有 "holding first turn until deps land"。
9. **注入**：paste + 按字节缩放的 settle（150ms+1ms/100B）+ `\r` + 400ms 后补一个 `\r`。成功只打 `tracing::info!("bootstrap prompt injected")`，超时/中止只打 warn。**全程无任何 DB 行或 SwarmEvent**。
10. orchestrator 的"打招呼"= 它自己调 swarm_send_message(to="user")。**服务端没有"注入后 N 秒无任何消息/活动"的看门狗**。

### 6. 唤醒机制（wake.rs，全内存）
- `wake_subs: agent→keys`、`exit_keys: agent→期望 handoff`，**进程内 HashMap，重启即失、无任何查询端点**。
- WakeCoordinator 订阅广播：BlackboardChanged → 给订阅者发 `kind=wake` 邮箱消息（meta `{subtype:"wake", reason:"blackboard", key}`）+ PTY kick（`\x15正文`+150ms+`\r`）；`.error/.failed` 别名 fan-out 唤醒 base key 等待者。
- **auto-kill on handoff**：worker 写出自己的 handoff key → 5s 宽限 → 自动杀 + 发 farewell 消息（meta `{subtype:"completion", signal}`）。
- **producer 死亡兜底**：Exited 且没写 handoff（含与上次运行残留 key 的 freshness 区分）→ 系统写 `<signal>.error` + 直接唤醒原 key 订阅者。
- **orphaned handoff 诊断**：写了 handoff 但零订阅者匹配（key 漂移）→ 只有 `tracing::warn` —— "为什么没动静"的最常见答案躺在日志里。
- 广播 Lagged → 对黑板做全量 reconcile 重唤醒（防一次性 handoff 事件丢失永久挂死）。
- pause/resume：`POST /api/agent/:id/interrupt`（Ctrl-C + paused 位，auto-wake 静默丢弃）、`/resume`（清位 + 一次手动唤醒）、`/wake`（⚡手动）、`/agent/interrupt-all?workspace_id=`。

### 7. 转录 tailer（transcript.rs）
- tail claude `~/.claude/projects/<enc-cwd>/<session-id>.jsonl` / codex per-agent `CODEX_HOME/sessions/.../rollout-*.jsonl`，700ms 轮询，**定位窗口长达 600s**（codex 文件可能几分钟后才出现）。
- 产出：`SwarmEvent::AgentActivity{kind:tool|system, label, phase:running|ok|error, seq, duration_ms}`（低频）+ 内存 ring（`GET /api/agent/:id/activity` 冷启回填）+ `agents.last_activity_at` 持久化 + `agent_usage` token 计量（model、in/out/cache、context_peak）。
- **"文件一直没出现，放弃"只有 debug 日志** —— tail 活性本身不可查询。

### 8. 其他实体
- `messages`：from/to/kind/body/meta/thread_id/in_reply_to/delivered_at/read_at + `thought_traces`（产品级推理摘要，0021）。wake 类消息 UI 按 meta 过滤。`consume_wakes` 端点原子认领 wake（Stop hook 用）。
- `blackboard_ops`：append-only 操作日志（write/external，notify watcher 捕外部编辑），历史可查（`/api/blackboard-history/*path`）。
- `pty_recordings`：asciicast v2 全程录像（含 finalize 元数据）。
- `goals`+`goal_evidence`（0019/0020）、`cron_jobs`、`spell_runs`（谱系）。

---

## ② 服务端掌握但 UI 未消费的事实（修"状态撒谎"的弹药）

| # | 事实 | 在哪里 | 现状 |
|---|------|--------|------|
| 1 | **CLI 登录态** | 无任何检测。`/api/plugins` 只有 `installed/resolved_path/version`（探 `--version`，3s 超时）+ install_hint（含 `login_command`） | "AI 引擎就绪"语义=「二进制在 PATH 上」。需要新探针（ready_plan needle 扫 "Not logged in"，或 spawn 前探测 auth） |
| 2 | **bootstrap 注入是否成功/超时/中止** | rest.rs spawn_bootstrap_inject，仅 tracing（injected / timed out waiting for ShimReady / aborted / mcp-ready fallback / has_unsubstituted_placeholders） | 不入库、不广播。UI 永远不知道 prompt 有没有真正送进去 |
| 3 | **mcp_ready**（swarm 工具对模型可见了） | AgentSlot.mcp_ready watch（内存） | 无 REST/WS 暴露 |
| 4 | **CLI 自动回退**（请求 claude 实给 codex） | select_spawn_plugin，仅 warn 日志 | 响应不带 fallback 字段 |
| 5 | **readiness gate 卡住中**（"holding first turn until deps land"，及 300s 超时强注） | 日志 | 协议有 `WaitingDep` 状态但**从未 publish** |
| 6 | **`Thinking/Idle` 状态** | 协议已定义 | 服务端从不发；绿点只剩"进程活着" |
| 7 | **last_activity_at** | 已入库、已在 `GET /api/agent` 返回 | UI 可直接算"spawn 后 N 分钟零工具活动 = 异常"，目前没算 |
| 8 | **token 用量 = 活性铁证** | `/api/usage`（by model/day/agent，context_peak） | spawn 后 60s 用量为 0 ≈ bootstrap 失败/未登录，UI 未用 |
| 9 | **wake_subs / exit_keys**（谁在等哪个 key、谁欠哪个 key） | 内存 HashMap | 无端点；DAG 只从 workers 表回填 |
| 10 | **orphaned handoff**（key 漂移→永久挂死） | orphaned_handoff_diagnosis，warn 日志 | UI 不可见 |
| 11 | **转录 tail 活性**（定位成功/600s 后放弃） | 日志 | 不可查询 |
| 12 | **isolation="degraded"**（隔离失败仍共享） | ThreadInfo 已返回 | 字段已在 API；专为警告设计，UI 是否强提示待查 |
| 13 | **方向命名→重栽 orchestrator**（杀旧拉新、消息转移） | AgentState+ThreadChanged 事件有，但无"为什么"事件 | UI 现象=成员闪断，无解释 |
| 14 | **软删空间的幽灵 agent** | DB 可查（workspace_id 指向已删行） | UI 无入口 |
| 15 | **farewell/completion/wake 结构化 meta** | messages.meta | 已部分消费 |
| 16 | **shim_exit_code**（非 0=异常死 → Error 状态） | 已入库已广播 | 可用作红色状态 |
| 17 | **PTY 原始输出里的红字真相** | PtyStream 重放缓冲（用户点抽屉才看到） | 服务端可自动扫 needle（基建已有），今天不扫 |

**结论**：修"撒谎"不需要大改存储——80% 是把已有日志事实升格为 ①一个新 `SwarmEvent`（bootstrap 阶段/健康）广播 + ②`GET /api/agent` 返回体里的 `bootstrap_stage/last_error` 字段 + ③一个登录态探针。

---

## ③ API 现状 vs 缺口

### 已有（可直接支撑的 UI 改造）
- REST 全量（routes/api.rs）：plugins、agent(spawn/list/kill/wake/activity/interrupt/resume/mcp-ready/interrupt-all)、worker、roles、message(list/send/read/unread_count/consume_wakes)、blackboard(+history/compact)、recording、workspaces(+roots/root-suggestions/branches/threads/:tid model/diff/merge)、spells、spell/run、goals(+evidence)、tasks(+status 覆盖)、usage(+pricing)、cron、files、mcp admin、prompt/optimize、attachment。
- WS：`/ws/swarm`（全事件直播，**lossy、无 resume**，lag 即断线要求重连重拉 REST 快照）、`/ws/pty/:agent_id`（带重放缓冲 + Hello 生命周期快照）、`/ws/terminal`（人用 shell）。
- 据此**今天就能做**：成员行"最近活动 x 分钟前 / token=0"侦测（#7/#8）；direction degraded 强警告（#12）；任务看板（tasks API 完整）；合并预览/AI 解冲突（diff/merge）；依赖挂载推荐（root-suggestions）；录像回放；活动流冷启回填（/activity）。

### 缺的接口（按修复优先级）
1. **`GET /api/agent/:id/health` + 新 SwarmEvent（AgentHealth/BootstrapStage）**：暴露管线阶段 `spawned→shim_ready→mcp_ready→(waiting_deps)→prompt_injected→first_output/first_message` + 失败原因。数据全在内存/日志，纯接线工作。这是 P0 的直接解。
2. **CLI 登录态探针**：`/api/plugins` 加 `logged_in/auth_error`；spawn 后用 ready_plan 式 needle 扫 "Not logged in"/限流串 → 升格为 AgentState::Error + AgentActivity(kind=system, phase=error)。
3. **首响看门狗**：orchestrator 注入后 N 秒无 message/activity → 广播可诊断事件（替代前端永远转圈的"AI 正在了解你的项目"）。
4. **`GET /api/workspaces/:id/health` 聚合**：成员存活/orchestrator 阶段/最近消息时间/ledger 是否已建/CLI 就绪 —— 一次请求供侧栏与空状态消费。
5. **wake 状态查询**（如 `GET /api/wake-state?workspace_id=`）：wake_subs+exit_keys 快照，回答"现在到底谁在等谁"；顺带把 orphaned-handoff warn 升格为事件。
6. **WS 无 resume 的纪律**：所有新增"健康"事实必须同时给 REST 快照（与现架构惯例一致），否则 lag 断线后 UI 又会撒谎。
7. **回退告知**：spawn/worker/spell 响应加 `fallback_from` 字段。
8. **幽灵 agent**：list_agents 已含 workspace_id，缺的是"已删空间仍有活 agent"的 UI 入口或删除时的可选级联杀。
