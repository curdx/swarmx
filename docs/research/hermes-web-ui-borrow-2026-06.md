# Hermes Web UI 可借鉴功能研究（2026-06）

> 研究对象：`github.com/EKKOLearnAI/hermes-web-ui`（克隆于 `/Users/wdx/opc/hermes-web-ui`，commit `32a386a`）
> 方法：4 个并行子代理分别深挖「多 agent 协同/chat 引擎」「可观测/模型/cron/profile」「前端 UX/设计系统」「基础设施/架构/工程化」四个维度，读实际代码并对照 flockmux 评估可搬性。
> 目的：从 Hermes Web UI 提炼可搬到 flockmux 的设计与功能。

## Hermes Web UI 是什么

围绕单个 **Hermes Agent**（Nous Research 的 AI agent）的全功能管理面板：Vue 3 + Naive UI + Pinia + Koa 2 BFF + Socket.IO + SQLite + node-pty + Electron。
架构上是 **BFF 薄壳 + 底层 Hermes CLI 重逻辑**：Web UI 自己几乎不算成本、不调度 cron、不管 profile 生命周期——都委托给底层 `hermes` CLI，自己只做「表单→CLI 参数」「读 CLI 磁盘产物」「拦截 bridge 响应补 token 记录」。

**与 flockmux 的关系**：同域（AI agent 的 Web 管理 UI），但定位互补——
- Hermes Web UI = 单 agent 的**全功能仪表盘**（用量分析、模型发现、cron、Web 终端、文件浏览、日志、多 profile、IM 渠道）。
- flockmux = 多 CLI agent 的**编排/协同**（swarm、blackboard、worktree 隔离、typed handoff、WakeCoordinator）。
- 因此 Hermes 的「外围能力」（可观测、终端、设置体验、工程化）恰好是 flockmux 的短板，值得搬；而 Hermes 的「单 agent chat 引擎」与 flockmux 的「多 worker 编排」内核差异大，多为思路参考。

**关键前提**：Hermes 底层 CLI 会吐结构化 `estimated_cost_usd`、自带 cron 调度、管 profile；而 flockmux 底层是 claude/codex CLI，**不吐成本、不带调度、不管 profile**。所以 Hermes「委托给 CLI」的部分，flockmux 大多得自建；但其「消费层/工具层」（聚合 SQL、价格源、URL 探测、纯 CSS 图表、文件化历史、机械化校验）可大量直搬。

---

## 全局搬运路线图（跨四维度综合排序）

### P0 — 最高价值（填补明确空白 / 投入产出比最高）

| # | 项目 | 价值 | 落地成本 |
|---|------|------|---------|
| 1 | **`harness:check` 机械化校验**：把 MEMORY.md 里十几条「改 A 必须改 B」的坑写成 CI 断言 | 把机构记忆从"靠人记"变"CI 强制"；与语言无关、立即可落地 | 低（一个 .mjs 或 Rust xtask） |
| 2 | **Usage/Cost 可观测全链路**：采集→聚合 SQL→models.dev 价格表→纯 CSS 图表 | flockmux 当前完全空白；程序员强需求 | 中（采集层需自建） |
| 3 | **实时活动面板 + 工具调用折叠行 + context_status 进度**：让 worker "正在干嘛"可见 | 直击 flockmux 可观测性核心；体验提升最大 | 中 |

### P1 — 推荐（高价值，体验/质量明显提升）

| # | 项目 | 价值 | 落地成本 |
|---|------|------|---------|
| 4 | **Web Terminal（旁观/接管 worker PTY）**：单 WS 多会话 + 首字节分流协议 + xterm 接入 | 程序员想直接看 worker 终端；flockmux 已有 PTY | 中（前端直接可搬，Rust 后端复刻协议） |
| 5 | **长 swarm 历史的上下文压缩（ContextEngine）**：快照增量摘要 + 多视角翻译 + 自适应阈值 | blackboard/message 无限增长，orchestrator 汇总缺压缩 | 中高（算法移植 Rust） |
| 6 | **session 事件日志 + resume 重放**：刷新/重连不丢正在跑的进度 + 队列可视化 | 程序员刷页面/换设备无缝接回 | 中 |
| 7 | **Ctrl+K 命令面板**：搜会话/跳方向/切 workspace，竞态防护 + 键盘环绕 | Linear/Vercel 级专业感，多 workspace/方向场景 | 中 |
| 8 | **代码块复制 + diff 折叠渲染**（`highlight.ts` 纯函数） | flockmux 是 coding agent 工具，展示 diff 是核心场景 | 低（纯函数可移植） |

### P2 — 锦上添花（低成本高观感 / 思路参考）

| # | 项目 | 价值 | 落地成本 |
|---|------|------|---------|
| 9 | **SCSS 变量委托 CSS 自定义属性 + 双轴主题 + RGB 分量** | 设计系统地基，无刷新切主题 + 一致半透明叠加 | 低中 |
| 10 | **SettingRow 原子组件 + Settings tab→子组件+URL query** | 71 行统一所有设置页；tab 可深链 | 低 |
| 11 | **i18n 深度合并 fallback**（纯函数）：漏译自动回退英文 | 把"加文案就进 locale"从纪律变架构保证 | 低 |
| 12 | **Provider URL 探测 + live/fallback 模型目录缓存** | 补强模型配给页/MCP 页，候选自动填充 | 低中 |
| 13 | **滚动 near-bottom 跟随 + 会话级位置记忆** | 聊天/日志流黄金法则 | 低 |
| 14 | **边界感知 @mention 路由 + per-agent 串行锁/去抖** | 补轻量人机交互（chat 里 @某 worker） | 低中 |
| 15 | **`$SHELL -l -c` 抓登录 shell PATH** | Tauri sidecar spawn CLI 找不到 claude/codex/git 的现成解法 | 低 |
| 16 | **Bridge readiness 双信号 + 退避重启 + 日志 summarize** | 直击 codex 启动卡死/503 的坑；只记长度/计数的日志姿势 | 低中 |
| 17 | **路径防穿越双层校验** | 开放文件访问时的安全底线 | 低 |
| 18 | **Cron 运行历史文件化 + synthetic 补洞 + 预设表单** | 定时唤醒 orchestrator 需求（调度核心需自建） | 中 |

---

## 维度一：多 agent 协同 + Chat 引擎

### 上下文压缩 ContextEngine（最值得搬）
- `context-engine/compressor.ts:58-284`：三路径快照增量压缩——无快照则全量摘要保留 tail；有快照则只摘 `lastMessageId` 之后的新消息，旧 summary 作前文喂摘要器；LLM 失败降级为裁剪。per-room 压缩锁（`:59-73`）。
- `mapToHistory`（`compressor.ts:375-408`）：群聊多 agent 历史按 `isOwnAgent` 翻译——本 agent 发言映射 `assistant`，他人映射 `user` + `[发送者]:` 归属前缀。**正是"多 worker 产出汇总给一个 reviewer/orchestrator"所需**。
- `run-chat/compression.ts:163`：`triggerTokens = contextLength × threshold`（默认 0.5），摘要可用更便宜的 `auxiliary.compression.model`。
- 对 flockmux：PTY 模式 CLI 自管上下文，不直接适用；但 blackboard/message 流无限增长，orchestrator 汇总进展时正缺此机制。**移植算法到 Rust**，契合 per-CLI 模型配给（用廉价 aux 模型摘要）。

### context_status 可见进度 + typing 快照
- `group-chat/index.ts:1124-1202`：服务端维护 typing（30s 过期）+ context_status（`compressing/replying/ready`），**新成员 join 时把当前状态塞进 ack**，重连立即可见。agent 回复全程发进度（`agent-clients.ts:436-460`）。
- 对 flockmux：把 worker 生命周期（spawn→建上下文→运行→写 blackboard→done）做成 typed status 广播，join/重连带快照。**直接可搬思路，提升 swarm 可观测性**。

### session 事件日志 resume 重放 + 队列
- `run-chat/index.ts:328-349`：`pushState` 把每个流事件追加进 `state.events`；`resume` 时若 session 仍在跑，把累积事件全部回放给新连上的 socket。所有 emit 走 `session:${id}` room 天然多端同步。
- session 在跑时再发 run → 入 `state.queue` + 广播 `run.queued`（含预览），可 `cancel_queued_run`。

### 边界感知 @mention 路由
- `group-chat/mention-routing.ts:33-80`：检查 `@name` 两侧边界字符（含中英文标点），避免 `@alice` 误中 `@alicent` / 邮箱误触发；`@all` 广播；排除发送者自身；`stripMentionRoutingTokens` 投喂某 agent 时只剥它的 token、保留指向他人的 mention。~100 行纯字符串解析，可移植 Rust `regex`。
- `agent-clients.ts:1108-1156`：per-agent 串行锁 + mention 队列，drain 时只取最后一条（去抖）。可强化 WakeCoordinator——多次 wake 命中同一 worker 时合并为最新意图。

### 流式 tool-call markup 过滤（跨 chunk 边界）
- `run-chat/bridge-delta.ts`：从 delta 流剥离 `[Calling tool: ...]` markup，处理 marker 跨 delta 边界（trailing-prefix 缓冲 + 括号配对 + flush 兜底防吞字符）。Rust 流解析 PTY 输出时同样适用。

### 思路参考（与 Rust+PTY 差异大，不直接搬）
- agent 作为回连 socket 客户端（`agent-clients.ts:144-164`）：agent 与人类共用同一消息总线/事件。
- cursor-based bridge 可恢复流（`agent-bridge/client.ts:462-494`）：双游标（cursor + event_cursor）天然可恢复，文本 delta + 结构化 events 双流。flockmux 未来接 Codex app-server 时值得重提。

---

## 维度二：可观测 / 模型管理 / cron / profile

### Usage/Cost 可观测（flockmux 空白，重点）
- **采集**：`proxy-handler.ts:119-186` 边转发 SSE 边解析，命中 `run.completed` 写 `data.usage`；`usage-store.ts:15-51` 写 SQLite（带 JSON fallback）。字段：input/output/cache_read/cache_write/reasoning + model + profile。
  - flockmux 改造：claude `--output-format stream-json` / codex JSON 事件流逐事件抓 `usage.{input,output,cache_creation,cache_read}_tokens`，写一张 SQLite usage 表（加 `agent_id`/`thread`/`role` 维度）。
- **成本公式**：`sessions-db.ts:1194` `COALESCE(SUM(COALESCE(actual_cost_usd, estimated_cost_usd, 0)))`——实际优先、估算兜底；`cost_status`/`pricing_version` 标可信度。
- **价格表数据源**：`model-context.ts:10` 用 `models_dev_cache.json`（opencode 同款 models.dev 数据集，含每模型 context/input/output/cache 单价）。flockmux 可定期拉 `models.dev/api.json` 或打包静态快照。
- **聚合 SQL**：`sessions-db.ts:1187-1248` totals / by_model(`GROUP BY model`) / by_day(`GROUP BY date(started_at,'unixepoch')`)；`tableHasColumn` 缺列降级；`sessions.ts:720-735` 补齐零值日期保证图表 X 轴连续。
- **纯 CSS 图表（零图表库）**：`DailyTrend.vue:36-75`（垂直堆叠柱）、`ModelBreakdown.vue:40-55`（水平条），`:style="{height/width: percent+'%'}"`，hover tooltip 纯 CSS；model 名 hash 取色（`usage.ts:58-66`）；7/30/90/365d 窗 + `latestRequestId` 防竞态。契合首包瘦身。
- 缓存命中率：`usage.ts:101` `cache_read/(input+cache_read)×100`。

### Model Management
- **Provider URL 探测**：`config-helpers.ts:236` `/\/v\d+\/?$/.test(base) ? base+'/models' : base+'/v1/models'`——已含版本号直接接，否则补 v1。8s 超时，去重排序。Rust + reqwest 几行。
- **凭证驱动发现 + 并发 + live/fallback 缓存**：`model-catalog-cache.ts` 遍历各 profile `.env`（按 `*_API_KEY` 前缀）识别 provider，`runLimited(candidates, 4)` 并发上限 4，结果标 `source: live|fallback`（失败退 preset），缓存 key=`provider|baseUrl|free/all`。
- **模型上下文 7 级 fallback**：`model-context.ts:412-448` DB override→provider config→custom→全局→models_dev→默认 256k，含别名映射（gemini→google）。
- OAuth 设备流（RFC 8628）：`copilot-device-flow.ts` start/poll/status 三段式。**flockmux 不适用**（登录由 CLI 自管）。

### Scheduled Jobs（cron）
- Web UI 不自调度，全转 `hermes cron create/edit/run/...`；状态读 `<profileDir>/cron/jobs.json`，历史读 `<profileDir>/cron/output/<jobId>/<timestamp>.md`（**运行历史=文件而非 DB 大字段**）。
- 巧思：立即触发=`cron run <id>`；create 后 before/after diff 找新 job；**synthetic run entry**（`cron-history.ts:116-183`）——job 跑了没产出时合成记录避免"跑了看不到"。
- 表单 `JobFormModal.vue:45-53`：7 个预设下拉**填充而非锁定**；投递目标未配置凭证自动 disabled；repeat 次数上限。
- flockmux：调度核心需自建（Rust cron + tokio timer）；可搬产物文件化 + synthetic 补洞 + 预设表单。注意融入 flockmux CronCreate 已有的「整点错峰」理念。

### Multi-Profile
- profile = 目录 `~/.hermes/profiles/{name}/`（config/auth/cron/sessions/skills/memory）。隔离：物理目录隔离 + Web UI 共享表加 `profile` 列 WHERE 过滤。
- **智能克隆清理**（`profile-credentials.ts:178` `smartCloneCleanup`）：clone 后自动剥离独占平台凭证（区分"可共享的模型 API key" vs "独占的 bot token"）、平台 `enabled:true→false`、改写前备份 `.bak.<timestamp>`。清单与上游 adapter 1:1 对齐（有依据，不主观扩展）。
- 用户↔profile 权限：`user_profiles` 关联表 + `canAccessProfile` 守卫全面复用 + `RESERVED_PROFILE_NAMES` 防撞 CLI 子命令。runtime status 缓存 + 防抖后台刷新。
- 对 flockmux：印证 per-agent CODEX_HOME / `--mcp-config` 隔离思路；profile 可作比 thread 更上层的「凭证集」隔离。

---

## 维度三：前端 UX / 设计系统

### 工具调用折叠行 + 实时活动面板（TOP，agent 编排标准答案）
- 折叠行 `MessageItem.vue:727-783` + `:1459-1571`：一行 11px 灰字（扳手图标 + 等宽 tool 名 + 单行 preview + 状态），仅 `hasToolDetails` 出现 chevron；展开 `border-left:2px` 缩进，Arguments/Result 两段代码块（`max-height:300px` 内滚）。状态语义化 SVG（running spinner / done 对勾 / error 红叉 + 时长 badge）。
- 实时活动面板 `MessageList.vue:252-414`：`isRunActive` 时消息流底部浮层，倒序列当前轮所有 tool call + spinner/对勾/时长，**内联压缩进度**（`Compressing... 1.2K→0.4K tokens`）和**中止进度**。把"黑盒等待"变可观测。

### 思考块 streaming-aware
- `utils/thinking-parser.ts`（纯函数可移植）：先用占位符保护代码块再分离已闭合/未闭合(pending)/正文；头部 `💭 思考中…·12s·1,234字`，时长 `tabular-nums` 等宽数字。streaming 中标签未闭合不错乱。

### 代码块复制 + diff 折叠（coding agent 核心）
- `highlight.ts`（368 行，纯函数）：复制按钮内联进 `<pre>` header，**事件委托** `handleCodeBlockCopyClick`（v-html 无法绑 Vue 事件的正解），`data-copy-text` 存完整原文。
- 统一 diff 渲染 `renderUnifiedDiffCode:174-212`：识别 diff/patch 渲染行号 +/− 着色（CSS grid `58px+内容`），**自动折叠 >8 行未改上下文**（保留首尾各 3 行，`⋮ 已隐藏 N 行`）。从 tool result JSON 递归抽 `diff/patch/stdout` 字段。
- `clipboard.ts`：HTTP 非安全上下文 fallback（内网部署必备）。

### Ctrl+K 命令面板
- `useKeyboard.ts`（全局 Ctrl/Cmd+K/N/J）+ `useSessionSearch.ts`（共享 ref 单例）+ `SessionSearchModal.vue`：空查询显示最近 8 个；160ms debounce；**竞态防护** `++requestSeq` 丢弃过期响应；键盘 ↑↓ 取模环绕，鼠标 `@mouseenter` 同步 activeIndex；结果项标题+来源 badge+2 行 snippet+匹配 `#id`。

### 滚动策略
- `MessageList.vue:115-216`：会话级 `{scrollTop,scrollHeight,wasNearBottom}` 快照（模块级 Map）切回恢复；streaming 时**仅用户在底部 200px 内才自动跟随**；触顶分页用 `captureScrollPosition`/`restoreScrollPosition` 锚定防跳动；`DynamicScroller` 变高虚拟列表。
- `ChatInput.vue:96-127`：每会话输入草稿持久化 localStorage。

### 设计系统
- `styles/variables.scss`：**SCSS 变量委托 CSS 自定义属性**（`$bg-card: var(--bg-card)`），主题切换只换 `:root`/`.dark` 的 CSS 变量值，无重编译无刷新；**导出 RGB 分量** `--accent-primary-rgb:51,51,51` 让 `rgba(var(--accent-primary-rgb),0.08)` 跟随主题；亮度×风格双轴正交（`useTheme.ts` 分两轴存 localStorage，system 模式监听 `prefers-color-scheme`）；Naive UI 通过 `theme.ts` `GlobalThemeOverrides` 同步。
- `global.scss:78-85`：仅 `html.theme-transitioning` 时给全局加 0.3s 过渡（切换瞬间加完即移除），避免日常交互慢半拍；`prefers-reduced-motion` 关动画。
- 全内联 SVG（`stroke:currentColor stroke-width:1.5`，Feather/Lucide 风格），零图标库依赖。

### i18n 工程
- `i18n/messages.ts`：`mergeMessagesWithFallback(en, locale)` 递归深合并到完整英文基底（纯函数可测）+ `fallbackLocale:'en'` 双保险——漏译自动显英文而非崩。locale 智能协商（`zh-Hant/-TW→zh-TW`）。
- locale 按功能域命名空间（~30 个域）；折叠态用独立短 key（`sidebar.groupXxxShort`）而非 CSS 截断。

### Settings / 表单
- `SettingsView.vue`：`NTabs` + 每 tab 一个独立子组件（10 个），SettingsView 只编排；`activeTab` 双向同步 `route.query.tab`（可深链/前进后退）；按权限过滤 tab。
- `SettingRow.vue`（71 行）：`<SettingRow label hint><控件/></SettingRow>` 原子化"标签+提示+控件"行，移动端自动堆叠。统一所有设置页视觉。

---

## 维度四：基础设施 / 架构 / 工程化

### harness:check 机械化校验（TOP 1 推荐）
- `scripts/harness-check.mjs`（379 行纯 Node 零依赖）把"只能靠人记的规矩"变 CI 断言：
  1. 存在性：必需文档/目录/10 个尺寸桌面图标 `requireFile`。
  2. 内容：`AGENTS.md` ≤120 行且必须链接子文档；`ARCHITECTURE.md` 必须含特定短语；`package.json` 必须有 5 个关键 script。
  3. workflow：build.yml 必须跑 harness:check 且 `fetch-depth:0`；release.yml 必须有三 OS matrix + `*.dmg/*.exe/*.AppImage` glob + `fail_on_unmatched_files:true`；**解析 bash case 块**确认单架构 macOS job 不上传 `latest*.yml`。
  4. **diff-aware（最巧）**：`changedFilesFromGit`（PR 用 `origin/$BASE...HEAD`）算改动文件；改了 chat 链路文件却没改 `docs/cli-chat-sessions.md` 直接 fail 并列出哪些文件。
  6. 失败收集全部一次性打印。
- **flockmux 落地**：把 MEMORY.md 每条坑写成断言——grep `tauri.conf.json` 确认 server+shim+mcp 三 binary 都在 externalBin、grep lib.rs 确认新 model 有 re-export、diff-aware 检查「改了 storage::MessageRecord 是否同步改 protocol::rest::MessageRecord」、检查 codex CODEX_HOME 隔离逻辑存在、「改 chat 链路要更新文档」。Rust xtask 或 .mjs 均可，挂 CI 必跑。

### Web Terminal（单 WS 多会话）
- 后端 `routes/hermes/terminal.ts:97-101`：一个 Connection 持 `sessions: Map<id,PtySession>` + `activeSessionId` + `outputBuffers`。inactive 会话不流给前端而服务端缓冲（截断到最近 5000 chunk）；切换先发 `switched` 控制帧再 flush。
- **协议巧思**：裸字符串=PTY 数据，**首字节 `0x7B`（`{`）=JSON 控制帧**（create/switch/close/resize），解析失败回退当裸输入。零额外 framing。
- 前端 `TerminalPanel.vue`：每会话一个 Terminal 实例缓存（`termMap`），切 tab **DOM 搬移而非重建**（`container.appendChild(term.element)`，xterm 多 tab 关键技巧）；`ResizeObserver`→`FitAddon.fit()`→发 resize 帧，挂载后双 `setTimeout(50/200)` fit 兜竞态；FitAddon+WebLinksAddon；4 套 ITheme localStorage 持久化；移动端手写 touch 滚动换算 `scrollLines`；token 走 query param（WS 无法设 header）；3 次重连（正常关闭码不重连）。
- flockmux：前端纯代码直接可搬；Rust 后端用 `portable-pty`+`tokio-tungstenite` 复刻，协议原样照搬。「点开某 worker 看/接管实时 PTY」是高价值可观测性。

### File Browser 多后端抽象
- `file-provider.ts`：统一 `FileProvider` 接口（read/list/stat/write/delete/rename/mkdir/copy），4 实现（Local 用 fs；Docker/SSH/Singularity 把语义翻译成 `docker exec`/`ssh`/`singularity exec`）。远端列举靠解析 `ls -la --time-style=...` 输出当"文件系统 API"。工厂读 `terminal.backend` 选实现 + 10s TTL 缓存；未实现后端显式抛 `unsupported_backend` 不静默降级。错误码归一化（`not_found`/`backend_timeout`/...）+ HTTP 层统一 statusMap。
- flockmux：多后端是过度设计（local 一种够用），但**统一 trait + 错误码归一化 + provider 工厂 TTL 缓存**值得照抄，为未来"worker 跑远程/容器"留扩展点。

### 路径安全（防穿越，开放文件访问必须）
- `validatePath:78-89` resolve+normalize 检查不含 `..` 且绝对路径；`resolveHermesPath:119-133` 相对路径解析后再 `isPathWithin(resolved, homeDir)` 二次确认（防 normalize 绕过）；`isSensitivePath` 黑名单（`.env`/`auth.json`）HTTP 层拦 403。Rust 用 `canonicalize`+`starts_with(base)`。

### Path-based 下载
- `download.ts`：`GET /api/hermes/download?path=&name=`，`Content-Disposition` 双给 `filename=`+`filename*=UTF-8''` 兼容中文；前端 token 走 query，先 decode 再 `URLSearchParams` 编码防双重编码，**fetch 先探错再转 blob** 让失败能弹精确错误。

### Logs Viewer
- `controllers/hermes/logs.ts`：`parseLine` 按优先级吃 3 种格式（pino JSON / Python 文本 / `[logger][time][LEVEL]`）；`appendPinoContext` 把结构化字段拼成 `key=value` 尾巴人读展示；返回 `.reverse()` 最新在前。
- `LogsView.vue`：后端粗筛（level/lines）+ 前端细筛（searchQuery）；**access log 高亮** `parseAccessLog` 拆 method/path/status 三段，status 按首位数字上色（2xx 绿/3xx 黄/4xx5xx 红）；level 左边框上色。

### Bridge Broker 状态机（与 flockmux 核心同构）
- `agent-bridge/manager.ts`：
  - **startup readiness 双信号**（`:427-504`）：监听子进程 stdout 命中 `{event:'ready'}` + 桌面 TCP 并行连通探测兜底；120s 超时 reject。
  - **指数退避自动重启**（`:523-546`）：`delay=min(30s, base×attempts)`，就绪后清零；**只有「曾就绪过 && 非主动 stop && autoRestart」才重启**（避免启动即崩死循环）。
  - Windows 端口抢占清理（`netstat -ano`→`taskkill /T /F`）；优雅停止 SIGTERM→1.5s→SIGKILL；解释器解析链层层 fallback。
- `agent-bridge/client.ts`：每请求新建 socket 写一行 JSON 读一行响应（newline-delimited JSON）；连接重试对 `ECONNREFUSED` 等在 120s 内每 100ms 重试；可选串行化锁（`serialize:true` 排进 lock Promise 链）；cursor 轮询拉流（`get_output` 不打日志）；**日志 summarize**（`:172-200`）只记 action+id+各字段计数/字符数而非完整内容（避免日志爆量保留可诊断性）。
- flockmux 可搬：readiness 双信号（直击 codex 启动卡死/503）、退避+曾就绪才重启门槛、日志 summarize、可选串行化锁。

### Koa BFF 分层 + WS 共存
- 多 WS/Socket.IO 共享一个 httpServer（`index.ts:226-254`），terminal WS（`noServer:true`+手动 handleUpgrade）/kanban WS/group-chat/chat-run（同 Socket.IO Server 不同 namespace），**最后注册 catch-all upgrade handler 把未知 upgrade `socket.destroy()`**（防野连接挂住）。本地 API 路由必须在 proxy catch-all 前。

### 鉴权 + 登录限流
- `auth.ts`：token 优先 `AUTH_TOKEN` env，否则 `~/.hermes-web-ui/.token`（32 字节 hex，`0o600`）；middleware 从 Bearer 或 `?token=` 取，非 API 路径放行。
- `login-limiter.ts`（设计完整）：**三层防护**——per-IP（10 次失败/15min→锁 1h）+ 全局速率（100 req/min→429）+ 全局熔断（累计 50 次失败→锁全站 30min 防分布式撞库）；状态持久化 `.login-lock.json`（普通失败 debounce 2s 批量落盘，触发锁定立即同步落盘）；IP map 超 10000 条淘汰。
- flockmux：桌面单机用「随机 token 写文件 + 127.0.0.1 only」即可；Web/LAN 模式时三层限流是现成参考。

### 桌面 sidecar 启动（flockmux 踩过的坑的正解对照）
- `webui-server.ts`：
  - **`$SHELL -l -c` 抓登录 shell PATH**（`mergePathEntries`+`getLoginShellPath`+nvm/homebrew 路径）——macOS GUI 应用启动 PATH 残缺导致找不到用户 CLI 的完整解法。Rust：`Command::new(shell).args(["-l","-c","echo $PATH"])`。
  - **health 探测 200/401 都算活**（401=起来了但要鉴权）+ stdout marker 判 bridge 就绪。
  - token 握手随机 32 字节写 `0o600`；端口分配 `getFreeTcpPort`；退出 `killProcessTree`（Windows `taskkill /T /F`）。
  - env 注释即踩坑记录：「ipc unix socket 在 macOS EDR/sandbox 下被静默 SIGKILL→强制 TCP loopback」。
  - node-pty `spawn-helper` 跨文件系统拷贝丢执行位→启动 `chmod 0o755`（Rust portable-pty 无此问题）。

### 可下载运行时 + CI release matrix（多数不适用 flockmux）
- runtime 不打进安装包，首次启动从 CDN 下载按 sha256 校验；electron-builder filter `!node_modules/node-pty/prebuilds/!(${platform}-${arch})/**` 只留当前平台 prebuild。flockmux 三 Rust binary 体积本就小，不适用；仅借鉴「只留当前平台 prebuild」「机械化断言禁止打不该打的」理念。
- `desktop-release.yml`：5 个 matrix target 各只传自己平台 artifact + `fail_on_unmatched_files:true`；单独 job merge 跨架构 `latest-mac.yml`。

---

## 子代理一致结论

- flockmux 因底层是 claude/codex（不吐成本/不带调度/不管 profile），**采集层与调度核心要自建**；但 Hermes 的**消费层/工具层**（聚合 SQL、models.dev 价格源、URL 探测、纯 CSS 图表、文件化历史、机械化校验、xterm 接入、防穿越、readiness 状态机）可大量直搬或借鉴。
- 最高优先级三件套：**① harness:check 把 MEMORY 坑变 CI 断言（投入产出比最高）；② Usage/Cost 可观测补空白；③ 实时活动面板让 worker 运行态可见。**
