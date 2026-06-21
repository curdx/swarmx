# swarmx × hermes 逐主题对比：哪些真的需要（带代码证据, 2026-06）

> 方法：5 个核查 agent **真打开 swarmx 代码**逐主题核实"到底有没有、缺不缺"，不是臆测。证据均为 `crate/文件.rs:符号` 或 `web/src/...tsx`。
> 与 `swarmx-borrow-roadmap.md` 的区别：那份的"swarmx 现状"是基于记忆的假设；本份是基于真代码——**有几处把 roadmap 的假设推翻了**（见末节"roadmap 修正"）。
> 判定图例：✅已有充分 · 🟡有地基可增量 · 🔴真空白需新建 · ⚪不适用
>
> **深度复核修正（2026-06，作者亲自逐文件读码）**：18 条承重论断 17 条对源码确认无误；**1 条推翻**——"worker env allowlist 比 hermes 更严、不漏父环境"是**错的**。真相：`swarmx-pty/src/lib.rs:76` 用 `CommandBuilder::new`，而 portable-pty 0.9 的 `CommandBuilder::new`（`cmdbuilder.rs:218 envs: get_base_env()`）用 `std::env::vars_os()`（**整个父环境**）做种子，且 swarmx **全仓无 `env_clear()`**——所以 worker **继承 swarmx-server 的全部环境变量**（含你导出的 `ANTHROPIC_API_KEY`/`OPENAI_API_KEY`/代理变量）；`spawn.rs:210` 那段"allowlist"只是叠加 override，注释写的"Drop everything else from the parent" **并未实现**。→ hermes 的"PTY env allowlist 剥 server token"借鉴点 **是真需要的**，且与本项目 per-agent 凭证隔离目标（见 CODEX_HOME 隔离）直接矛盾。修法见 §2 quick-win。

---

## 0. 一图速览

| # | 主题 | 判定 | swarmx 现状（证据） | 真缺口（一句话） |
|---|---|---|---|---|
| 1 | Kanban 任务面板/状态机/DAG | 🔴 | `workers` 表无 status 列；`Ledger.tsx:15` 注释"orchestrator 唯一 writer 用户只读"；`Dag.tsx` 画的是 agent 拓扑非任务 | 无七态任务状态机、无可写控制平面、无 task-level DAG |
| 2 | Usage/Cost 可观测 | 🔴 | `transcript.rs:10` 注释"for future cost stats"但 parse 从不读 `usage`；15 个 migration 无 usage 表 | 完全空白：不采集 token、无成本、无展示 |
| 3 | 上下文压缩（orchestrator 层） | 🔴 | 无 `context_compressor`；`transcript.rs:summarize()` 只生成 UI 标签不压历史 | orchestrator 汇总消息无限增长会 overflow |
| 4 | cron / 定时编排 | 🔴 | 无 scheduler；migration 无 cron 表（`CronCreate` 是 Claude Code 工具非 swarmx 功能） | 无定时 spawn orchestrator 能力 |
| 5 | 围栏修复 + diff 渲染 + 工具结果截断 | 🔴 | `ChatMarkdown.tsx` 有 react-markdown 链，但无 fence-repair / diff 折叠 / `truncateJsonValue` | LLM ```md 嵌套围栏不展开；大 JSON 工具结果撑爆 DOM |
| 6 | virtual 滚动 / 消息列表性能 | 🔴 | `MessagesPanel.tsx` 全量 `rows.map`，靠 limit:200 截断，无虚拟化 | 工具调用量大时全量 DOM 卡 |
| 7 | 会话搜索 / 草稿 / @mention | 🔴 | `messages_fts`/`blackboard_fts` 后端存在但前端没接；@mention 仅 `MessagesPanel.tsx:1184` 注释占位 | 后端有 FTS 前端完全未接；草稿切 tab 丢失 |
| 8 | 会话恢复/事件日志/stale-worker 自愈 | 🟡 | PTY 层 seq resume 完整（`pty_ws.rs:181` 优于 hermes）；`.error` fallback（`wake.rs:744`）；但 SwarmEvent 层无 cursor | **worker hang 不退出无兜底（S5 盲区）**；`last_activity_at` 字段有但无定时扫描 |
| 9 | 鉴权 / 限流 / 网络安全 | 🟡 | `main.rs:746` `require_local_origin`（有 Origin 强制本地、无 Origin 放行；挡浏览器跨站 WS，有测试）+ 写死 loopback bind（`main.rs:449`） | **worker 继承 server 全环境（无 env_clear，见顶部修正）→ 可读全部 secret**；另公网场景缺 ws-ticket/redact/resize-clamp |
| 10 | 文件路径安全 + 原子写 | 🟡 | `swarmx-swarm/src/path_safe.rs` 完整（lexical+canonicalize+starts_with，有测试）；config 写已 tmp→rename | **blackboard 写非原子（`swarm.rs:288` 直接 fs::write）**；无 per-path 锁 |
| 11 | 命令面板 / slash 命令 | 🟡 | `CommandPalette.tsx` 已用 cmdk + Ctrl+K + Settings tab↔URL 深链 | 缺输入框 `/cmd` 补全 + steer 中途注入 |
| 12 | 内置终端 | 🟡 | `XtermPane.tsx` 质量高（WebGL pool + 首字节 JSON 控制帧 + seq 恢复），接在 agent PTY | 缺通用 shell 终端（不绑 agent）+ 旁观/接管语义 |
| 13 | 实时活动 / 工具调用渲染 | 🟡 | `transcript.rs:TailState.pending` running→done 按 tool_id 合并 + seq；`swarm.rs:record_activity` 64 环形 + 冷启动回填 | 缺 `summarize_tool_result` 语义摘要、工具护栏三维检测 |
| 14 | readiness / 重启 / 降级 | 🟡 | OSC_READY marker（`spawn.rs:386`）+ `ReadyPlanRunner`（数据驱动应答对话框）优于 hermes 文本正则 | 缺指数退避重试 + binary 候选链查找 |
| 15 | 多 profile / 凭证集隔离 | 🟡 | per-agent `CODEX_HOME` 隔离（`pre_spawn.rs:418`）是雏形 | 缺多账号轮换 CredentialPool + 429 熔断状态机 |
| 16 | 模型管理 / 发现 / 配给 | 🟡 | `models_config.rs` tier→model per-CLI 映射 + effort + 原子写 + 单测（核心已做） | 缺 context_length 检查、provider-aware 熔断、429 跨 session 断路器 |
| 17 | 设计系统 / 主题 / i18n / 设置 | 🟡 | `main.tsx:11` 闪屏预防（render 前应用 theme）；`settings.tsx` 有 Field/ToggleRow/ChoiceCard 原子；tab 深链已做 | 缺 i18n deep-merge fallback、MCP 页草稿门控 |
| 18 | 把 swarmx 暴露成 MCP server | 🟡 | 内向 MCP 已全：`swarmx-mcp/src/tools.rs:55` 暴露 10 个 `swarm_*` 给 worker；`pre_spawn.rs:712` 自动注入 | 缺外向 endpoint（给外部 Claude Code/IDE 用） |
| 19 | harness / 工程化校验 | 🟡 | `scripts/harness-check.mjs` 4 规则已落地（commit e714bc3） | 可加 Rule 5-7：migration/role 注册表/wake-alias |

---

## 1. 🔴 真空白 —— 需要新建（7 项）

### 1. Kanban 任务状态机 + 可写控制平面（最大工程，价值最高）
- **现状**：任务靠黑板自由文本承载。`workers` 表（`0005_workers.sql`）有 `handoff_signal`/`depends_on_json`，`0011` 加了 `role_slug`/`produces_json`/`consumes_json`（类型化 handoff），但**无 status 列、无 task_links 表**。台账 `Ledger.tsx` 纯只读渲染 `task.ledger.md`/`progress.ledger.md`。`Dag.tsx` 画的是 **agent 拓扑**（spawn 亲子边 + handoff 边），不是任务依赖图。
- **缺**：triage→todo→ready→running→blocked→done 七态；从 UI 认领/阻塞/完成；task-level DAG；`consecutive_failures` circuit breaker（现 `.error` 只兜底一次，无重派上限）；stranded-in-ready 检测；claim heartbeat / stale claim 回收。
- **抄**：状态机 + circuit breaker → hermes-agent `kanban_swarm`；Kanban UI（7 态 + WS 实时 + 终态保护 + 两步 complete/block）→ hermes-web-ui `KanbanView`；diagnostic rules（无状态只读规则）思路同 swarmx harness-check。
- **增量路径**：先给 `workers` 加 `task_status` 列 → 再建 Kanban UI → 不破坏现有 agent 拓扑 DAG。

### 2. Usage/Cost 可观测（surgical，价值高）
- **现状**：`transcript.rs:10` 自己写了"carries token usage (for future cost stats)"，但 `parse_claude`/`parse_codex` 只处理 tool_use/tool_result，**从不读 `usage` 字段**；测试甚至断言 `token_count` 行产生空结果（`transcript.rs:658`）。无 usage 表、无聚合 API、无前端。
- **为什么 surgical**：transcript.rs 已经在逐行读 JSONL，只差多读一个 `usage` 对象 → 写一张表 → 一个聚合 API → 一个纯 CSS 图表页。
- **抄**：采集 → hermes-agent `normalize_usage`（Anthropic/Codex/ChatCompletion 三路归一）+ `PricingEntry` 定价表 + `estimate_usage_cost`；消费 → hermes-web-ui `usage_store.ts`（聚合 SQL + 纯 CSS 堆叠条，零图表库）。

### 3. 上下文压缩（仅 orchestrator 汇总层）
- **现状**：无任何压缩。注意 **PTY 下 worker 自身上下文由 claude/codex CLI 自管（有原生 /compact），swarmx 不需要碰**。真空白在 orchestrator 汇总层：读黑板 + 给 worker 分发 + 拼 system prompt，worker 一多 orchestrator 自己 overflow。
- **抄**：hermes-agent `ContextEngine` 双路径 + `sanitize_tool_pairs`（20 行纯函数）；hermes-web-ui per-workspace `triggerTokens`/`maxHistoryTokens`。

### 4. cron / 定时编排
- **现状**：无 scheduler、无 cron 表。`wake.rs` 的 timer 是 WakeCoordinator 轮询不是用户定时任务。
- **抄**：hermes-agent `cron/scheduler.py`（in-flight dedup + wake-gate）+ hermes-web-ui `JobCard/JobFormModal`（7 预设 + 运行历史）；Rust 用 tokio-cron。

### 5. 围栏修复 + diff 渲染 + 工具结果截断（含一个 P0 quick win）
- **现状**：`ChatMarkdown.tsx` 渲染链完整（react-markdown@10 + gfm + highlight + sandbox 预览），但无 `markdownFenceRepair`、无 unified diff 行号/折叠、**无 `truncateJsonValue`**（`AgentActivityLog.tsx` 直接展示 raw）。
- **缺口里的刺**：`truncateJsonValue`（六维截断）是**防卡死 P0**——worker 读大文件时工具 payload 会撑爆前端 DOM。纯函数可直接搬。
- **抄**：hermes-web-ui chat 子系统 `markdownFenceRepair`（217 行）、`highlight.ts`（diff 369 行）、`truncateJsonValue`。

### 6. virtual 滚动
- **现状**：`MessagesPanel.tsx` 全量 `rows.map` + `overflow-y-auto`，靠 `limit:200` 截断；有置底/锚点/auto-read 但非虚拟化。
- **抄**：hermes-web-ui `VirtualMessageList`（置底/翻页保位/切会话保位三算法）；React 侧用 `@tanstack/react-virtual`。

### 7. 会话搜索 + 草稿持久化 + @mention（后端已就绪，只差前端）
- **现状**：`messages_fts`/`blackboard_fts` 表已建但**前端零接入**（MessagesPanel 顶栏只是本地字符串 filter）；无 composer 草稿持久化；@mention 仅 `MessagesPanel.tsx:1184` 注释 + i18n 文案占位。
- **抄**：hermes-web-ui `SessionSearchModal`（防抖+键盘）、草稿按 `wsId+threadSlug` 存 localStorage、`@mention` DOM mirror（IME 安全）。

---

## 2. 🟡 已有地基 —— 外科手术式增量

按"投入小、价值高"排序，几个是 **quick win**：

| 增量点 | 现状基线 | 要补的那一刀 | 量级 |
|---|---|---|---|
| **worker env 隔离（安全真缺口）** | spawn.rs 已构造干净 env map，但 portable-pty 默认继承父全环境、无 `env_clear` | `swarmx-pty/src/lib.rs:76` 建完 `CommandBuilder` 后调 `cmd.env_clear()` 再 apply allowlist；同时把 allowlist 补全（claude/codex 真正需要的：HOME/PATH/LANG/TERM/CODEX_HOME/SWARMX_*，按需加 HTTPS_PROXY/NO_PROXY/NODE_*）。**实现 spawn.rs 注释里已声明的意图**，与 per-agent 凭证隔离目标一致 | quick win（一行 + 调 allowlist） |
| **blackboard 原子写** | `path_safe.rs` 路径安全已全；config 已 tmp→rename | `swarmx-swarm/src/swarm.rs:288` 的 `fs::write` 改 tmp→rename（黑板是最关键持久态，crash 会写出截断文件） | quick win |
| **stale-worker hang 自愈** | PTY seq resume + `.error` fallback 已有；`last_activity_at` 字段已有 | 加定时扫描：`now - last_activity_at > N 且 agent alive → 注入 wake 或强杀重派`，**结构性补 S5 盲区** | 小 |
| **summarize_tool_result** | `TailState.pending` running→done 合并已有 | 给工具结果加语义摘要（`ran npm test → exit 0, 47 lines`）替代裸标签 | 小 |
| **i18n deep-merge fallback** | zh/en + i18next fallbackLng 已有 | 加逐 leaf 递归 merge，避免半翻译 key 直接渲 key 字符串 | quick win |
| **指数退避重试 + binary 候选链** | OSC marker + ReadyPlanRunner 优于 hermes | spawn 失败加退避重试；`locate_shim` 加 brew/which 多路查找（现在找不到 CLI 直接 500） | 小 |
| **模型 context_length + provider 熔断** | `models_config.rs` tier→model 已全 | 加 context_length 字段 + 跨 session 429 断路器文件 | 中 |
| **命令面板 slash 命令** | `CommandPalette.tsx` Ctrl+K 已全 | composer 加 `/cmd` 下拉 + steer 中途注入（复用 WakeCoordinator PTY inject） | 中 |
| **多账号轮换 CredentialPool** | per-agent CODEX_HOME 隔离雏形 | 加账号池 + 429 cooldown 状态机（直缓解 88code/nowcoding 限流） | 中 |
| **harness Rule 5-7** | 4 规则已落地 | 加 migration↔schema.rs / role↔registry / wake-alias 三条 | 小 |

---

## 3. 🟡 核心已到位 —— 低优先 / 按需

这几项 hermes 借鉴清单里看着诱人，但**代码证据显示 swarmx 核心已做**，只在特定场景才需补：

- **鉴权（网络层）**：loopback 跨站威胁**已防住**（`require_local_origin` 覆盖 WS 升级，浏览器跨站 WS 带 Origin→403，有测试）。ws-ticket/redact/resize-clamp **只在绑 LAN / Tauri 远控 / 公网时才需要**。⚠️ 但**进程隔离层没防住**：worker 继承 server 全环境（见顶部修正 + §2 quick-win），这条不是"已到位"，是真缺口。
- **内置终端**：worker PTY 终端（XtermPane）**已有且质量高**。只缺"用户自己开个 shell"的通用终端——价值中等。
- **MCP 暴露**：给 worker 用的内向 MCP **已全**（10 个 swarm_* tool）。外向（给外部 IDE/Claude Code 用）是新场景，价值看你要不要在 IDE 里操控 swarm。
- **设计/主题/设置**：闪屏预防 + SettingRow 原子 + tab 深链**都已做**，只差 i18n deep-merge（已列入上表）和 MCP 草稿门控。
- **路径安全**：`path_safe.rs` 已完整，唯一真缺口是上表的 blackboard 原子写。

---

## 4. roadmap 修正（代码证据推翻的假设）

上一份 `swarmx-borrow-roadmap.md` 基于记忆假设，把这几项列得过高/过宽，**真代码核实后应下调或收窄**：

1. **WS/HTTP 鉴权（原 P0）→ 网络层降级为"公网场景才做"，但 env 隔离要升回**。网络层：`require_local_origin` + loopback bind 已闭合浏览器跨站威胁，ws-ticket 让位到公网场景。**但深度复核推翻了"env allowlist 已实现更严"——实际 worker 继承 server 全环境**（portable-pty 默认 + 无 env_clear）。所以 hermes 的"PTY env allowlist 剥 server token"**仍是真需要**，且是 quick-win（一行 env_clear + 调 allowlist），优先级不低（直接关系凭证隔离）。
2. **路径安全（原 P0）→ 收窄为单点"blackboard 原子写"**。`path_safe.rs` + config tmp→rename 已做；不需要整块"路径安全工程"，只差 `swarm.rs:288` 一行改法。
3. **会话事件日志/重连（原 P0 的一半）→ 收窄为"stale-worker hang 自愈"**。PTY 层 seq resume 已有且优于 hermes 的纯 SSE；真正值 P0 的只剩 hang-not-exit 的 S5 兜底（且 `last_activity_at` 字段已备好）。
4. **实时活动渲染** swarmx 已有完整 running→done+seq 链路，hermes 的只是语义摘要增量——确认是 P1 质量增量非新建。

**修正后的真 P0（按真缺口 + ROI）**：① Usage/Cost 采集+展示 ② Kanban 状态机+可写台账 ③ `truncateJsonValue` 防卡死（quick win）④ 会话搜索/草稿/@mention 接上已有 FTS。
**几乎零成本的 quick win**：**worker env 隔离（env_clear，安全真缺口）**、blackboard 原子写、i18n deep-merge、stale-worker 定时扫描、summarize_tool_result。
