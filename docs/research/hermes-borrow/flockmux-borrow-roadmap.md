# flockmux 借鉴 roadmap（三 repo 综合, 2026-06）

> 数据源：`/tmp/hermes-borrow/all_borrow.json`（281 条原始借鉴点，字段 repo / subsystem / idea / value / effort / flockmux_fit / priority）。
> 三 repo 命中：`hermes-agent` 104 条（Python 后端引擎，最厚）/ `hermes-webui` 91 条（轻量 Rust+原生前端，与 flockmux 同构）/ `hermes-web-ui` 86 条（Vue3+Naive UI 重前端）。
> 已有调研交叉引用：`docs/research/hermes-web-ui-borrow-2026-06.md`（四维度全景 + 18 条路线图，下称 **HW-borrow**）、`docs/research/hermes-borrow/HW-chat.md`（聊天/群聊逐组件考古，下称 **HW-chat**）。本 roadmap 在它们之上做**跨三 repo 去重 + 对标 flockmux 现状重排优先级**，不重复复述其细节，只在备注 cross-ref。
>
> ⚠️ JSON 里 reader 给的 `priority`（P0=46/P1=125/P2=110）只是单点评分，**未对标 flockmux 现状**——很多被标 P0 的（实时活动行、markdown/diff、@mention、设计 token）flockmux 已经做过。本 roadmap 的 P0/P1/P2 是**重新裁定**的。

---

## 摘要

281 条原始借鉴点 → 去重合并为 **18 个主题**（跨 repo + repo 内）→ 重排为 **P0 5 条 / P1 7 条 / P2 6 条**（共 18，与主题一一对应，每主题给一个落地裁定）。

裁定原则：**已经做过的不进 P0**（实时活动行、群聊 markdown/图片/沙箱预览、黑板台账、方向 worktree、per-CLI 模型配给、MCP 管理页、读 JSONL 工具级进度、harness:check 均已落地）；**真正补空白且 ROI 高的才进 P0**。三 repo 高度共识、flockmux 完全空白的两块是 **Usage/Cost 可观测**（42 条命中，三 repo 全提）和 **Kanban 可写任务面板**（53 条命中，三 repo 全提）——它们构成 P0 的核心；其余 P0 是低成本补安全底线（鉴权/路径）与可靠性（会话事件日志/stale-worker 自愈）。

热点排序（命中条数）：Kanban/状态机 53 > Usage/Cost 42 > 鉴权/限流 28 > 设计/i18n/设置 18 > cron 17 > 实时活动渲染 16 > MCP 自暴露 14 > 上下文压缩 13 > 围栏/diff 渲染 12 > 内置终端 11 > 命令面板 10 > 会话恢复 10 > 文件浏览 7 > readiness 3 …

---

## P0 — 现在就做

补 flockmux 明确空白、三 repo 强共识、ROI 最高。

| 主题 | 来源 repo（最佳实现） | 价值 | 工作量 | 落到 flockmux 哪个模块 | 备注 |
|---|---|---|---|---|---|
| **Usage/Cost 可观测全链路** | 三 repo 全提（**hermes-web-ui** 消费层最值得抄：`usage_store.ts` per-session input/output/cache_read/cache_write/reasoning 入 SQLite、`sessions-db.ts` totals/by_model/by_day 聚合 SQL、纯 CSS 堆叠/水平条图零图表库；**hermes-agent** 采集层最值得抄：`normalize_usage` 跨 API shape 归一化 + `PricingEntry` 官方定价快照 + `estimate_usage_cost`） | flockmux **完全空白**，程序员强需求；claude/codex 底层不吐成本，须自建采集 | 中（采集层自建，消费层直搬） | 采集：`transcript.rs` 读会话 JSONL 时抓 `usage.{input,output,cache_creation,cache_read}_tokens`；存储：新建 `usage` 表（维度加 `agent_id`/`thread`/`role`）；价格：打包 `models.dev` 快照或定期拉；API：`GET /api/usage`（totals/by_model/by_day）；前端：新增 Usage 页 StatCards+DailyTrend+ModelBreakdown 纯 CSS | **已是 HW-borrow P0#2 + HW-chat 多处**，本表确认并补 hermes-agent 采集细节（normalize_usage / PricingEntry）。聊天页加 context-window 环形进度（hermes-webui）作为这块的轻量前哨 |
| **Kanban 可写任务面板（台账→控制平面）** | 三 repo 全提（**hermes-webui** 定位最贴：「只读黑板 → 可写 CRUD 控制面板」+ Last-Event-ID 断线续传；**hermes-web-ui** 状态机最完整：triage/todo/ready/running/blocked/done/archived 七态 + 多看板 + SSE 实时；**hermes-agent** 引擎最硬：Task graph DAG（typed handoff → parent→child 依赖图 + recompute_ready 自动提升）、claim heartbeat + stale claim 回收、build_worker_context 结构化上下文） | flockmux 黑板台账目前**只读**；UX 审查已定调「台账升级成人机共写控制平面」（见 `project_qa_2026_06_ux_review.md`），这是它的成品形态 | 高（后端状态机 + 前端面板，可分批） | 后端：blackboard 之上加 task 状态机 + DAG（复用 typed handoff key + 角色注册表 + WakeCoordinator）；前端：新 Kanban 视图（WS 实时刷新，复用现有 WS 广播）；orchestrator 派单写 task、worker claim/done 改状态 | 与现有 typed-handoff/WakeCoordinator 是**同构升级而非另起炉灶**。先做「黑板可写 + 七态展示」，DAG/heartbeat 留 P1 增量。cross-ref UX 审查 P0 |
| **WS/HTTP 鉴权 + 网络安全底线** | **hermes-agent** 最系统（`POST /api/auth/ws-ticket` 单次 browser ticket + 进程级 internal credential、Host header middleware 防 DNS rebinding + WS 升级手动 Host/Origin 检查、`should_require_auth` loopback 免鉴权/公网 gate、PTY env allowlist 剥 server token；**hermes-webui** 补 PBKDF2+HMAC 签名 cookie 零依赖） | flockmux 现状裸跑 127.0.0.1；一旦绑 LAN/隧道/Tauri 远控即裸奔。WS ticket + Host 校验是**几十行的安全底线**，现在补成本最低 | 低（loopback 免鉴权 + 公网才 gate，约 200 行 axum middleware + ws-ticket） | `routes/ws.rs`（ws-ticket 换 WS 升级）；新增 axum Host/Origin middleware；spawn worker PTY env 走 allowlist 剥掉 server token（防 worker 读到主进程凭证） | PTY env allowlist 与 MEMORY 里「codex per-agent CODEX_HOME 隔离」同源，应一起做。三层登录限流（per-IP+全局+熔断）留 P1，仅公网模式需要 |
| **会话事件日志 + stale-worker 自愈** | **hermes-webui** 最贴 flockmux（Run journal = SSE 事件 append-only JSONL + seq cursor 增强现有 JSONL tail；Turn journal = 用户意图边界 + flock + 目录 fsync；**Stale worker 检测 + synthetic error 事件注入**；SSE/WS cursor + Last-Event-ID 断线续传；coalescing bounded-queue 事件总线；**hermes-agent** session/load 历史回放协议） | 直击 UX 审查 P0 盲区 **S5「卡死 worker 无兜底」**（worker hang 不退出 → 无 .error → orchestrator 永等）；同时让刷新/重连不丢进度 | 中（journal + cursor + stale 检测器） | `transcript.rs`（JSONL tail 升级为 seq cursor + Last-Event-ID 续传）；`wake.rs`/WakeCoordinator 加 stale-worker 探测器（last_activity 超时 → 注入 synthetic `<signal>.error` → orchestrator 自愈重派） | **S5 的结构性解法**：现有 `.error` fallback 只覆盖「退出有兜底」，hang 不退出是盲区（见 `project_m6c_error_fallback_design.md`）。synthetic error 注入正好补这一刀。cross-ref UX 审查 S5 |
| **路径安全 + 原子文件写** | 三 repo 全提（**hermes-webui** 最硬：TOCTOU 防御 anchored fd `openat`+`O_NOFOLLOW`、workspace 三重信任检查 + 系统目录黑名单、git env scrub + repo-root 互斥锁；**hermes-web-ui** `validatePath` resolve+normalize+二次 `isPathWithin`、token 防双包装下载；**hermes-agent**+**hermes-web-ui** `writeFileAtomic` tmp→rename + per-path 锁） | flockmux 已开 `GET /api/file` 图片白名单 + 文件浏览/合并将开更多文件访问面；防穿越 + 原子写是开放文件访问的**前置安全债**，现在欠后面要还 | 低（约 30 行校验函数 + tmp→rename 封装） | 已有 `GET /api/file` Host 守卫之上补 canonicalize+starts_with(base) 二次校验；blackboard/journal/git 托管文件写改 tmp→rename + per-path 锁（防并发损坏）；workspace 路径加系统目录黑名单 | flockmux 已踩过「托管 .claude/.codex 文件假 dirty」（见 `project_git_merge_closure.md`），原子写能减少这类竞态。与 P0 文件浏览/Usage 下载共用同一套校验 |

---

## P1 — 推荐

高价值，明显提升体验/可靠性，但不补「致命空白」或成本中等。

| 主题 | 来源 repo（最佳实现） | 价值 | 工作量 | 落到 flockmux 哪个模块 | 备注 |
|---|---|---|---|---|---|
| **内置终端（旁观/接管 worker PTY）** | 三 repo 全提（**hermes-web-ui** 协议最巧：单 WS 多 session + 首字节 `0x7B`=JSON 控制帧、xterm DOM 搬移切 tab、13 主题；**hermes-webui** PTY env allowlist + spawn supervisor；**hermes-agent** 终端选区→注入聊天） | 程序员想直接看/接管 worker 终端；flockmux 已有 PTY 基础设施，前端纯代码可直搬 | 中（前端直搬，Rust 后端用 portable-pty 复刻协议） | 新 `/ws/terminal`（portable-pty + tokio-tungstenite，协议原样照搬）；前端 XtermPane 抽屉 | **已是 HW-borrow P1#4**。env allowlist 与 P0 鉴权同源。worktree 删除守卫要检 active terminal（hermes-webui） |
| **上下文压缩 / 历史治理** | **hermes-web-ui** ContextEngine 双路径（增量快照 + 全量 + CJK-aware token 估算）+ per-workspace 可配 triggerTokens/maxHistoryTokens/tailMessageCount；**hermes-agent** 14 段结构化摘要 prompt + SUMMARY_PREFIX + sanitize_tool_pairs（压缩后修孤立 tool 结果）+ strip_historical_media；多视角 mapToHistory（他人发言→user+`[sender]:`） | blackboard/message 无限增长，orchestrator 汇总进展缺压缩；用 per-CLI 廉价 aux 模型摘要 | 中高（算法移植 Rust） | 新建 `context_compressor.rs`（移植双路径 + mapToHistory）；触发点 = worker 会话 JSONL token 估算超 threshold；用 per-CLI 模型配给的 aux tier | **已是 HW-borrow P1#5**。注意 PTY 模式 CLI 自管上下文，这块只用于 orchestrator 汇总层，不碰 worker 自身上下文 |
| **命令面板 / slash 命令语言** | **hermes-webui** 最完整（斜杠命令声明式注册表 + noEcho + fall-through 协议、补全下拉命令名+子参数+文件路径三路聚合、Steer 中途注入不中断 turn）；**hermes-web-ui** bridgeCommands `/compress /steer /plan /goal`、Ctrl+K SessionSearchModal 防抖+键盘环绕+`++requestSeq` 防竞态 | 多 workspace/方向场景的 Linear/Vercel 级专业感；把「派 worker/合并方向/压缩/steer」做成 slash 命令 | 中 | 前端命令面板（Ctrl+K）+ 输入框 slash 下拉；后端 steer 中途注入复用 WakeCoordinator 的 PTY inject | **已是 HW-borrow P1#7**。声明式注册表（hermes-webui）比硬编码更可维护，优先抄它的形状 |
| **围栏修复 + unified diff 渲染** | **hermes-web-ui**/**hermes-webui** 同源（`markdownFenceRepair` 剥 LLM ```md 外层围栏、`highlight.ts` unified diff 行号+折叠 >8 行未变更上下文、从 tool result JSON 递归抽 diff/patch/stdout、`truncateJsonValue` 六维安全截断）；**hermes-webui** renderMd stash-token diff/csv/json/yaml 特判 | flockmux 是 coding agent 工具，展示 diff 是核心；ChatMarkdown 已有但缺围栏修复 + diff 折叠；merge-resolver 改文件场景刚需 | 低（纯函数移植到 ChatMarkdown） | `ChatMarkdown` 加 fence-repair 预处理 + unified-diff 渲染器；tool 结果展示加 truncateJsonValue 六维截断防卡死 | **已是 HW-borrow P1#8 + HW-chat 2.3/2.4**。flockmux 已有 markdown 渲染（commit 6f17d2b），此为增量。围栏坑与「Tailwind v4 灭 list-style」同类真实坑 |
| **把 flockmux 自身暴露成 MCP server** | **hermes-agent**（`hermes_tools_mcp_server` 把工具经 MCP stdio 暴露给 codex；把引擎状态暴露成可读 MCP）；**hermes-webui**/**hermes-web-ui** 配套 | 让 worker（claude/codex）能经 MCP 读 blackboard/agent 状态、发消息——把 flockmux 编排能力反向开放给 worker 自己调度 | 中（新建 mcp_server 模块 + axum/stdio） | 新建 `mcp_server.rs`（暴露 blackboard 读/agent 状态读/消息发为 MCP tool）；spawn worker 时把它注入 per-agent MCP config | 与现有「spawn 注入 per-agent MCP config」「MCP 管理页」天然衔接。flockmux-swarm 已是雏形，这是把内部 swarm 能力标准化为 MCP |
| **实时活动渲染细化（占位/回填 + 工具结果摘要）** | 三 repo（**hermes-web-ui** mapGroupMessages 先 running 占位、后按 tool_call_id 合并 done + content/reasoning 双增量流；**hermes-agent** 工具护栏 exact_failure/same_tool_failure/idempotent_no_progress 三维检测 + summarize_tool_result + StreamingThinkScrubber） | flockmux 已有实时活动行（读 JSONL），但渲染契约较粗；占位/回填 + tool 结果摘要让进度更细、更省 token | 中（后端 transcript.rs 加摘要 + 前端分发） | `transcript.rs` AgentActivity 广播点加 summarize_tool_result（覆盖 terminal/read/write/search/patch）；前端按 tool_name 分发 formatter；工具护栏写入 agent error_meta 供 orchestrator 决策 | **flockmux 已做实时活动行**（commit e29f49a），此为**质量增量非新建**，故降到 P1。护栏三维检测对「codex 空跑/重复失败」自愈有用（cross-ref `reference_codex_worker_stall_diagnosis.md`） |
| **多 profile / 凭证集隔离** | **hermes-agent** 最厚（多 profile 后端池 LRU+keepalive、CredentialPool 多账号轮换 + exhausted/dead 状态机、Borrowed vs Owned 凭证磁盘净化）；**hermes-web-ui** smartCloneCleanup 克隆剥独占凭证 + user_profiles 权限表 | flockmux 现状 per-agent CODEX_HOME 隔离已是雏形；profile 可作比 thread 更上层的「凭证集」隔离，多账号轮换缓解 503/限流 | 中 | 抽象 profile = 凭证集目录；CredentialPool 轮换接到 spawn 汇聚点；config 共享密钥 vs 独占 token 区分（对齐 MEMORY「共用密钥」诉求） | 印证现有 per-agent CODEX_HOME/`--mcp-config` 隔离思路。CredentialPool 轮换可直接缓解 88code.ai/nowcoding.ai 限流（cross-ref codex stall playbook） |

---

## P2 — 锦上添花 / 思路参考

低成本高观感，或与 Rust+PTY 差异大只作思路参考。

| 主题 | 来源 repo（最佳实现） | 价值 | 工作量 | 落到 flockmux 哪个模块 | 备注 |
|---|---|---|---|---|---|
| **cron / 定时编排** | 三 repo（**hermes-web-ui** 运行历史文件化 + synthetic 补洞 + 7 预设表单填充不锁定；**hermes-webui** Tasks 面板；hermes-web-ui 会话重置 idle_timeout/daily_reset/at_hour） | 定时唤醒 orchestrator（夜间跑、整点错峰）；调度核心须自建 | 中（Rust cron + tokio timer） | 新建 cron 模块（tokio timer）；产物文件化 + synthetic 补洞 + 预设表单 | **已是 HW-borrow P2#18**。flockmux CronCreate 已有「整点错峰」雏形，按其理念扩 |
| **文件浏览器** | **hermes-web-ui** FileTree+List+Editor+Preview+ContextMenu+Toolbar、统一 FileProvider trait + 错误码归一化 + 工厂 TTL 缓存；**hermes-agent** react-arborist 懒加载目录树 + 拖入终端自动 quote；**hermes-webui** FolderPicker 扁平 FlatNode+childrenCache | 程序员想在 UI 里浏览 workspace；多后端是过度设计，统一 trait + 错误码归一值得抄 | 中 | 新 `/api/files` + 前端文件树抽屉；local 一种后端够用，留 trait 扩展点；复用 P0 路径安全校验 | **已是 HW-borrow 维度四**。与内置终端共用右侧抽屉。git 状态徽章（branch/dirty/ahead/behind，hermes-webui）可附加 |
| **设计系统 / 主题 / i18n / 设置原子化** | 三 repo（**hermes-web-ui** SCSS 委托 CSS 变量 + 双轴主题 + RGB 分量 + SettingRow 71 行 + Settings tab↔query.tab 深链 + i18n 深合并 fallback；**hermes-webui** 闪屏预防 head 内联 IIFE 同步恢复 theme/font/panel + 双轴 theme×skin + 纯 CSS data-tooltip） | 设计地基 + 无刷新切主题 + 漏译回退；flockmux 已有部分设计 token，缺闪屏预防 IIFE + SettingRow 统一 | 低 | 前端 head 内联 IIFE 恢复偏好；抽 SettingRow 原子组件；i18n 加深合并 fallback；Settings tab 写 query.tab | **已是 HW-borrow P2#9/10/11**。flockmux i18n 已专业化（commit 9738f60），加 fallback 是架构保证。闪屏 IIFE 是即时观感提升 |
| **readiness 双信号 + 桥接退避重启 + 降级** | **hermes-agent** readiness 双信号（stdout ready marker + TCP 探测）+ 指数退避「曾就绪才重启」门槛 + 日志 summarize；**hermes-webui** SSE→polling fallback + backend 候选链 smoke-probe | 直击 codex 启动卡死/503；日志只记长度/计数避免爆量 | 低中 | spawn worker 时 readiness 双信号 + 退避重启门槛；日志 summarize | **已是 HW-borrow P2#16**。flockmux 已修过 codex 503（commit 8570436），此为系统化加固。cross-ref codex stall playbook |
| **会话搜索 + 草稿持久化 + @mention** | **hermes-web-ui** SessionSearchModal 防抖+键盘+动态注入 session、输入草稿按 sessionId 存 localStorage、@mention DOM mirror 下拉 IME 安全、mention-options 纯函数；**hermes-webui** composer 草稿存服务端、多维未读双轨 | 长会话/多方向导航 + 不丢草稿 + 群聊指名 worker；flockmux 群聊已有但缺这些细节 | 低中 | 前端会话搜索 modal（可并入命令面板）；composer 草稿持久化；@mention 候选纯函数移植 Rust regex | **HW-chat 1.4/1.5/2.7**。@mention 可强化 WakeCoordinator（多次 wake 合并最新意图）。flockmux 已修未读绿点撒谎（commit 2f7e9fd），双轨未读是进一步细化 |
| **思路参考（不直接搬 / 差异大）** | **hermes-agent** Ralph-style goal loop（per-worker 循环 + judge 判 done）、session fork 分支探索、CodexAppServerClient JSON-RPC 三队列、ContextVar per-session 隔离、Bitwarden/SOUL.md/Skill frontmatter；**hermes-webui** PWA + SW 版本号注入 + 首启引导 probe-gate；虚拟滚动 VirtualMessageList | 多为内核差异大或 flockmux 已有等价物，留作未来重提 | — | — | VirtualMessageList（HW-chat 2.1）蜂群消息成百上千条时再上。CodexAppServerClient 等 ACP 已 park（见 `project_acp_support_reality.md`）。goal loop 与 flockmux orchestrator 重叠 |

---

## 主题聚类（281 条 → 18 主题，按命中条数降序）

> 命中数由对 idea+subsystem+value+flockmux_fit 文本的关键词归类得出（互斥，首个匹配优先），用于看热点；个别边缘条目归类可能 ±1。

| # | 主题 | 命中数 | 三 repo 覆盖（A=agent / W=web-ui / w=webui） | flockmux 现状 | 裁定 |
|---|---|---|---|---|---|
| 1 | Kanban 任务面板 / 状态机 / DAG | 53 | A W w | 黑板只读 | **P0** |
| 2 | Usage/Cost 可观测 | 42 | A W w | 空白 | **P0** |
| 3 | 鉴权 / 限流 / 网络安全 | 28 | A W w | 裸跑 loopback | **P0**（底线）+ P1（限流） |
| 4 | 设计系统 / 主题 / i18n / 设置 | 18 | A W w | 部分有 | P2 |
| 5 | cron / 定时编排 | 17 | A W w | CronCreate 雏形 | P2 |
| 6 | 实时活动 / 工具调用渲染 | 16 | A W | **已有活动行** | P1（质量增量） |
| 7 | MCP / 把自身暴露成 server | 14 | W w | 有 MCP 管理页 | P1 |
| 8 | 上下文压缩 / 历史治理 | 13 | A W w | 空白 | P1 |
| 9 | 围栏 / markdown / diff 渲染 | 12 | A W w | **已有 markdown** | P1（增量） |
| 10 | 内置终端 (xterm/PTY) | 11 | A W w | 有 PTY 基建 | P1 |
| 11 | 命令面板 / slash 命令 | 10 | A W w | 空白 | P1 |
| 12 | 会话恢复 / 事件日志 / 重连 | 10 | A W w | JSONL tail 广播 | **P0**（stale 自愈）+ P1 |
| 13 | 文件浏览 / 预览 / 路径安全 | 7+ | A W w | /api/file 图片白名单 | **P0**（路径安全）+ P2（浏览器） |
| 14 | virtual 滚动 / 消息列表性能 | 2 | W w | 无虚拟化 | P2（量大时） |
| 15 | 模型管理 / 发现 / 配给 | 2 | A W | **已有配给页** | 已做，零增量 |
| 16 | @mention / 消息路由 | 1（+散落） | W | 群聊已有 | P2 |
| 17 | readiness / 桥接重启 / 降级 | 3 | A w | 修过 503 | P2 |
| 18 | harness / 工程化校验 | 5 | A W w | **已落地** | 已做（commit e714bc3），新坑加规则 |
| — | 散落（profile 池/凭证/PWA/skill/memory 等） | ~17 | A W w | — | 并入 P1 多 profile / P2 思路参考 |

**读法**：热点全在 1-3（Kanban/Usage/鉴权，共 123 条 ≈ 44%），且三 repo 全提——这是三个项目的共识刚需，正好命中 flockmux 三块空白。4 之后的主题里，6/9/15/18（实时活动/markdown/模型配给/harness）flockmux **已做过**，故不进 P0。

---

## 建议落地顺序（4 批，结合 Rust + PTY + 多 CLI 约束 + 现有架构 + 依赖关系）

### 批次 1 — 安全底线 + 可观测地基（先打地基，互相解锁）
1. **路径安全 + 原子文件写**（P0，低）——后续文件浏览/Usage 下载/journal 写都依赖它，最先做。
2. **WS/HTTP 鉴权底线 + PTY env allowlist**（P0，低）——独立、几十行，与 CODEX_HOME 隔离同源一起做。
3. **Usage/Cost 采集层**（P0 上半）——在 `transcript.rs` 读 JSONL 时同步抓 token，写 `usage` 表。**依赖**：复用现有 JSONL tail（已有）。
> 批次 1 全是低/中成本、互不阻塞，可并行；2 与 3 都挂在已有的 transcript/spawn 汇聚点。

### 批次 2 — 可观测可见化 + 可靠性自愈（让批次 1 的数据/状态浮现）
4. **Usage/Cost 消费层**（P0 下半）——聚合 SQL + `/api/usage` + 纯 CSS 图表页。**依赖** 批次 1#3 的采集表。
5. **会话事件日志 + stale-worker 自愈**（P0）——JSONL tail 升级 seq cursor + Last-Event-ID；stale 探测注入 synthetic error。**依赖** 批次 1#1 原子写（journal fsync）。**直接修 UX 审查 S5**。
6. **围栏修复 + diff 渲染**（P1，低）——纯前端，无后端依赖，可与 4/5 并行；为后面 Kanban/merge-resolver 的 diff 展示备好。

### 批次 3 — 控制平面升级（最大投入，建在前两批之上）
7. **Kanban 可写任务面板**（P0，高）——黑板可写 + 七态展示先行；DAG/heartbeat/stale-claim 作增量。**依赖** 批次 2#5 的事件日志（实时刷新走 cursor 续传）+ 现有 typed-handoff/WakeCoordinator/角色注册表。
8. **命令面板 + slash 命令**（P1）——派 worker/合并方向/compress/steer 做成命令；steer 中途注入复用 WakeCoordinator PTY inject。**与 7 协同**（命令面板是操作 Kanban 的快捷入口）。
9. **上下文压缩**（P1）——orchestrator 汇总层，用 per-CLI aux 模型。**依赖** 批次 1#3 的 token 估算（判断何时触发）。

### 批次 4 — 体验铺开 + 反向开放（锦上添花，随时插队）
10. **内置终端**（P1）——前端直搬 + Rust portable-pty 复刻；env allowlist 已在批次 1#2 备好。
11. **文件浏览器**（P2）——与终端共用右侧抽屉；路径安全已在批次 1#1 备好。
12. **flockmux 自暴露 MCP server**（P1）——独立，接现有 MCP 管理页 + per-agent MCP config 注入。
13. **多 profile / CredentialPool 轮换**（P1）——缓解限流，接 spawn 汇聚点；与现有 CODEX_HOME 隔离收口。
14. **cron / 设计 token / readiness 加固 / 会话搜索 / @mention**（P2）——零散低成本，按观感诉求随时插。

**关键依赖链**：原子写/路径安全 → journal fsync + 文件浏览/下载；Usage 采集 → Usage 页 + 压缩触发判断；事件日志 cursor → Kanban 实时刷新 + 前端 reload 恢复；env allowlist → 内置终端 + worker 凭证隔离。**约束提醒**：所有「采集/状态/压缩」逻辑落 Rust（PTY 透传，无法 hook 工具执行——护栏只能读 JSONL 事后判断，不能拦执行前）；多 CLI 下凡涉及模型的（压缩 aux 模型、Usage 价格）都走 per-CLI 模型配给汇聚点解析，勿硬编码。
