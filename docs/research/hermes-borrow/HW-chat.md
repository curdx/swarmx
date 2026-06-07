## HW-chat — Hermes Web UI 聊天与群聊（多 agent）逐组件考古 + flockmux 借鉴清单

> 范围：`/Users/wdx/opc/hermes-web-ui/packages/client/src/components/hermes/chat`（18 文件）+ `group-chat`（6 文件）+ `views/hermes` 里 chat 相关 view。逐组件穷举 UI 元素 / 交互 / 状态来源 / 布局。重点放在 **group-chat 如何呈现多 agent 对话**——这正对位 flockmux 的蜂群聊天（SwarmPanel / MessagesPanel / ChatMarkdown）。
>
> 技术栈：Vue3 `<script setup>` + Pinia + naive-ui + vue-i18n + `vue-virtual-scroller` + `socket.io-client`（群聊走 Socket.IO，单聊走 chat store/SSE）。

---

### 0. 文件总览与对位

| 文件 | 行数 | 一句话 | flockmux 对位 |
|---|---|---|---|
| `group-chat/GroupChatPanel.vue` | 1337 | 群聊主壳：房间侧栏 + 头部 stacked 头像/agent 管理 + 状态条 + 审批条 + 消息列表 + 输入 | SwarmPanel 整体 |
| `group-chat/GroupMessageItem.vue` | 1097 | 单条群消息气泡（user/agent/tool/thinking/附件/error/TTS） | MessagesPanel 单条 + ChatMarkdown |
| `group-chat/GroupMessageList.vue` | 153 | 群消息虚拟列表包装（过滤 tool、首屏置底、上拉翻页） | MessagesPanel 列表 |
| `group-chat/GroupChatInput.vue` | 774 | 群聊输入框：`@mention` agent 下拉 + 附件 + 拖拽高度 + tool-trace 开关 | SwarmPanel 输入 |
| `group-chat/CreateRoomForm.vue` | 161 | 建房表单（昵称/邀请码/压缩配置折叠） | 新建方向/房间表单 |
| `group-chat/mention-options.ts` | 46 | `@all` + agent 列表 mention 候选构建（纯函数） | @mention 候选 |
| `chat/MarkdownRenderer.vue` | 787 | markdown-it + KaTeX + Mermaid + 本地文件卡片/视频/图片预览 | ChatMarkdown 升级版 |
| `chat/highlight.ts` | 369 | 高亮 + **unified diff 渲染（行号/折叠未变更行）** + 复制按钮 HTML | ChatMarkdown 代码块/diff |
| `chat/markdownFenceRepair.ts` | 217 | 修复 LLM 把整篇答案包进 ```md 外层围栏 / 嵌套围栏 | ChatMarkdown 预处理 |
| `chat/mermaidRenderer.ts` | 47 | Mermaid 占位/编码/上限常量 | ChatMarkdown mermaid |
| `chat/VirtualMessageList.vue` | 483 | **核心虚拟滚动引擎**（DynamicScroller 封装：置底/锚点对齐/翻页快照） | MessagesPanel 性能底座 |
| `chat/MessageList.vue` | 813 | 单聊消息列表 + **流式工具调用面板** + **消息队列浮窗** + 压缩/中止指示 | TaskActivity + 蜂群活动 |
| `chat/MessageItem.vue` | 1662 | 单聊单条气泡（含 command/status/系统消息/thinking 计时） | MessagesPanel 单条 |
| `chat/ConversationMonitorPane.vue` | 333 | **只读会话监控**：左会话列表 + 右消息（15s 轮询） | 旁观 agent 会话 |
| `chat/OutlinePanel.vue` | 312 | 从消息正文抽 Q + h1-h3 标题做大纲导航 | 长会话导航 |
| `chat/ChatInput.vue` | ~900 | 单聊输入：`/slash` 命令下拉 + 上下文长度编辑 + 附件 | CommandPalette/输入 |
| `chat/ChatPanel.vue` | ~1900 | 单聊主壳：会话侧栏（批量删/置顶/profile 过滤）+ 模型选择 + 审批/澄清条 + 抽屉 | chat 路由壳 |
| `chat/SessionSearchModal.vue` | ~330 | 全局会话搜索（防抖 + 键盘导航 + 跳转） | 会话/方向搜索 |
| `chat/SessionListItem.vue` / `HistoryMessageList.vue` / `FilesPanel.vue` / `FolderPicker.vue` / `TerminalPanel.vue` | — | 会话项 / 历史消息 / 文件面板 / 文件夹选择 / 内嵌终端 | 侧栏项/文件浏览器/XtermPane |

views：`ChatView.vue`（薄壳：路由 sessionId↔store 同步 + 动态 document.title）、`GroupChatView.vue`（薄壳：`connect()`/`loadRooms()`/`disconnect()` 生命周期 + roomId 路由同步）。其余 view（Kanban/Jobs/Usage/Profiles/Memory/Plugins/Skills…）见其它附录。

---

### 1. group-chat：多 agent 对话如何呈现（重点）

#### 1.1 GroupChatPanel.vue — 群聊主壳（最高优先借鉴）

**布局**（三段式）：
- **左：房间侧栏 `.room-sidebar`**（220px，`window.innerWidth>768` 才默认展开；移动端为 absolute + backdrop 遮罩）。
  - header：标题 + 「+建房」icon-btn。
  - `.room-list`：每个 `.room-item` = 房间图标 + 房间名 + 邀请码（mono 字体）+ token 计数（`formatTokens`，`≥1000` 显示 `x.xk tokens`）+ hover 出现的删除按钮（`NPopconfirm` 二次确认）。
  - **右键菜单**：`@contextmenu` → 手动定位 `NDropdown`（复制房间链接 / 克隆房间）。
- **中：主聊天区 `.chat-main`**：
  - **头部 `.chat-header`**：折叠侧栏按钮 + 房间标题 + **stacked 头像组**（用户头像在最前 z-index 最高，agent 取后 4 个负 margin 叠放，超出显示 `+N`）→ 点击弹 `NPopover` 列出「你 + agents(N)」，每个 agent 行可移除（× 按钮）。再加：+加 agent / ⚙压缩配置 / 🗑清空上下文（Popconfirm）/ 成员数 / **连接状态绿点**（`store.connected`）。
  - **消息区** `<GroupMessageList />`。
  - **状态条 `.status-bar`**：两种——(a) `contextStatuses`（每个 agent 一个 chip：`@name 正在回复/压缩中` + 跳动 typing dots + **红色停止按钮**调 `interruptAgent`）；(b) 普通 `typingText`（"X is typing…"）。
  - **审批条 `.approval-bar`**：盾牌图标 + kicker + `@agentName · 标题` + 描述 + `<code>` 命令块 + 4 个动作按钮（once/session/always/deny，按 `choices` 条件渲染）。
  - **输入** `<GroupChatInput @send>`。
  - 无房间时 `.no-room` 空态。
- **弹窗（Teleport to body）**：建房（`CreateRoomForm`）/ 加 agent（profile 选择 + 名称 + 描述）/ 克隆房间（名+邀请码+刷新生成）/ 压缩配置（triggerTokens / maxHistoryTokens / tailMessageCount + 「立即压缩」按钮）。

**状态来源**：全部来自 `useGroupChatStore`（rooms / currentRoomId / agents / members / contextStatuses / activePendingApproval / connected / userName / currentUserAvatar）+ `useProfilesStore`（profile→avatar 映射）。本地 ref 只管弹窗开关与表单字段。

**关键交互函数**：`handleCreateRoom`（建房失败逐 agent 反馈 `formatAgentFailures`）、`extractApiErrorMessage`（从 err.message 里挖 JSON 取 `PROFILE_AGENT_CONNECT_FAILED` 友好文案）、`copyRoomLink`（拼绝对 URL）、`handleForceCompress`（压缩进行中禁用）、`handleApproval`、`handleInterruptAgent`。

#### 1.2 GroupMessageItem.vue — 单条群消息（核心渲染契约）

一条消息按 `role` 分支渲染，**身份判定全靠数据**：
- `isAgent` = agents 里能匹配 `senderId/senderName`；`isSelf` = `senderId===currentUserId`；据此决定气泡左/右（`.self { flex-direction: row-reverse }`）和配色（agent 用 accent 淡色、self 用 accent 更淡、error 用红框）。
- **头像解析链**：agent → `profilesStore` 里 profile.avatar；user → member.avatar（JSON 解析 `{type:'image',dataUrl}`）→ 回退 `ProfileAvatar`（multiavatar by name seed）。
- **tool 消息**（`role==='tool'`）：折叠行 = 扳手图标 + `toolName`（mono）+ `toolPreview` 单行省略 + running spinner / error badge；展开后分 `Arguments` / `Result` 两段代码块（`v-html` 注入 `renderToolPayload`）。
- **thinking 块**（`hasThinking`）：💭 + 「思考中/思考」+ **字符数 meta**；流式时强制展开，否则可折叠（`thinkingOverride`）。文本来源优先 `reasoning` 字段，回退从 content 解析 `<think>` 标签（`parseThinking`）。
- **附件**：图片缩略图（点开 lightbox overlay）/ 文件卡片（下载链接）。**还能从 content 是 `[...]` 的 JSON contentBlocks 里抽 image/file 块**（`renderedAttachments`）。
- **正文**：`<MarkdownRenderer :content :mention-names>`，mentionNames = `['all', ...agentNames]` 用于高亮 `@xxx`。
- **message-meta**（hover 才显）：TTS 播放按钮（多 provider：openai/custom/edge/mimo/webspeech）+ 复制整条气泡 + 时间。
- **错误气泡**：`finish_reason==='error'` 或 content 以 `Error:` 开头 → 整条红框红字（连 markdown 内联也染红）。

**JSON 安全截断**（`truncateJsonValue`）：tool payload 超 1000 字符时做**深度/节点数/键数/数组项/字符串长度多重上限**的安全截断，防止巨型工具结果卡死渲染——这是 flockmux 工具结果展示直接能抄的防御。

#### 1.3 GroupMessageList.vue — 群消息列表包装

- `displayMessages` = `store.sortedMessages` 过滤掉 tool（除非 `toolTraceVisible` 或正在 running）。
- 复用 `VirtualMessageList`，`estimated-item-height=170`，`@top-reach` 上拉翻页（`loadOlderMessages` + 翻页前 `captureScrollPosition`/翻页后 `restoreScrollPosition` 保持视口）。
- **首屏置底策略**：`pendingInitialBottomRoomId` 标记+多帧 `scrollToBottom({frames:5,keepAliveMs:700})`，流式追加时只在「近底部」才自动跟随（`isNearBottom(200)`）——避免用户上翻历史时被强行拽到底。
- watch 的依赖键把 `id/content.length/reasoning.length/toolStatus` 拼成串，**精确感知流式增量**触发置底判断。

#### 1.4 GroupChatInput.vue — `@mention` 多 agent 输入（重点借鉴）

- **`@mention` 全自定义实现**（不用 NDropdown）：`updateMentionState` 反向扫描光标前最近的 `@`（遇空格/换行停），用**镜像 span 测量**算出下拉的像素 X 坐标，并据视口空间决定上/下弹（`placement`）。下拉项 = `@all`（高亮 accent）+ 匹配的 agent（label + profile 描述）。键盘 ↑↓ 循环、Enter/Tab 选中、Esc 关；`selectMention` 把 `@name ` 插回光标处。
- **输入区**：附件按钮 + **自动播放语音开关**（localStorage 持久化）+ **tool-trace 显隐开关**（`useToolTraceVisibility` 全局组合式）。
- **附件**：点击/拖拽（dragCounter 计数避免子元素闪烁）/ **粘贴位图**（`handlePaste` 过滤 image/* 转 File）。
- **可拖拽改高度**：顶部 `.resize-handle` 鼠标拖动改 textarea 高度（20-400px），未手动拖时按内容自适应（`scrollHeight` capped 100px）。
- **中文输入法保护**：`isComposing` + `e.keyCode===229` 双保险，避免拼字中途回车误发。
- `emitTyping` 节流广播「正在输入」给房间其他人。

#### 1.5 mention-options.ts — 候选构建（纯函数，可直接搬）

`buildMentionOptions(agents, query)`：`@all` 永远置顶（保留字，agent 名若叫 all 被跳过），其余按名字 `includes(query)` 过滤，输出 `{key,type,name,label,description}`。**纯函数无副作用、易单测**——flockmux @mention worker/role 可照搬这个形状。

#### 1.6 群聊数据流（store + Socket.IO，最高架构价值）

`stores/hermes/group-chat.ts`（868 行）+ `api/hermes/group-chat.ts` 揭示**完整多 agent 实时协议**：

- **传输**：`socket.io('/group-chat')`，`reconnectionAttempts: Infinity` + 指数退避 + websocket/polling 降级。
- **流式三段事件**：`message_stream_start`（建占位气泡 + 去重清理同发送者空流气泡）→ `message_stream_delta`（content 追加）/ `message_reasoning_delta`（reasoning 追加）→ `message_stream_end`（落定，空气泡删除）。**正文/思考独立两条增量流**——对位 flockmux 读 JSONL transcript 的 AgentActivity。
- **多 agent 并发态**：`contextStatuses: Map<agentName, {status}>`（compressing/replying/ready），每个 agent 独立状态条；`typingUsers: Map`（含 5s 超时自动清）。
- **工具调用流**：`mapGroupMessages`（store 末尾纯函数）把 `tool_calls` 拆成独立 tool 气泡、把后到的 `role:'tool'` 结果**按 `tool_call_id` 回填**进占位气泡（合并 name/args/preview/result/status）——这套「先占位 running、后合并 done」正是 flockmux 实时进度想要的。
- **审批流**：`approval.requested` / `approval.resolved` 事件 → `pendingApprovals: Map<approvalId>`，`activePendingApproval` 只取当前房间的。
- **token 计费**：`room_updated` 推 `totalTokens`，侧栏实时显示；房间级压缩配置（trigger/maxHistory/tail）+ `forceCompress`。
- **断流自愈**：`needsFinalContentRecovery`（只有 reasoning 没 content）→ 300ms 后 `getRoomDetail` 重拉补 content（`recoverMissingFinalContent`）——网络抖动兜底。
- **乐观发送**：`sendMessage` 先本地 push 用户气泡（带上传后的 url），ack 失败再回滚 filter 掉。
- **加房恢复态**：`join` 的 ack 里带回 `typingUsers` / `contextStatuses` / `agents`，**重连后恢复"谁在打字/谁在跑"**，不丢上下文。

---

### 2. chat（单聊）里值得搬的机制

#### 2.1 VirtualMessageList.vue — 虚拟滚动引擎（P0 性能底座）

封装 `vue-virtual-scroller` 的 `DynamicScroller`，对外暴露一套高层 API：`scrollToBottom({frames,keepAliveMs})`（多帧重试置底，应对动态高度未稳）、`scrollToMessage`/`scrollToAnchor`（先 scrollToItem 再多帧 `alignElement` 精对齐，找真实 DOM 锚点优先）、`captureScrollPosition`/`restoreScrollPosition`（翻页保位：按 scrollHeight 差值反推 scrollTop）、`captureViewportPosition`/`restoreViewportPosition`（切会话保位）、`isNearBottom`。内部用 `requestAnimationFrame` 循环 + `keepBottomUntil` 时间窗 + `ResizeObserver`。**flockmux MessagesPanel/SwarmPanel 长会话直接受益**——蜂群消息可能成百上千条，目前若无虚拟化必然卡。

#### 2.2 MessageList.vue — 流式工具面板 + 消息队列浮窗（P0/P1）

- **流式指示器 `.streaming-indicator`**（`#after` slot）：左边「思考 gif」+ 右边 **tool-calls-panel** 实时列出本轮（最后一条 user 之后）所有工具调用，每行 = 图标 + name + preview + **执行耗时** + 状态（running spinner / ✓done 绿 / ✗error 红）。**还内联压缩指示**（Compressing… N msgs ~X→Y tokens）和**中止指示**（Pausing…/Paused and synced）。这是 flockmux「成员栏活动行 / TaskActivity」的成品级参考。
- **消息队列浮窗 `.queue-float-panel`**（sticky 右下）：跑批时排队的用户消息，带轨道 spinner 动画 + 序号 + 预览 + 删除。对位 flockmux「mailbox/待派任务」可视化。

#### 2.3 highlight.ts — unified diff 渲染（P1，蜂群改代码场景刚需）

不止语法高亮：能识别 unified diff（即便没标 ```diff，靠 `isUnifiedDiffContent` 启发式：文件头≥2 + hunk 头），逐行渲染**带行号（old/new 分列）**，并**折叠超过 8 行的未变更上下文**（`⋮ N unchanged lines`）。还能从工具结果 JSON 里递归挖 `diff/patch/stdout/content` 字段当 diff 渲染（`extractUnifiedDiffPayload`）。flockmux merge-resolver / worker 改文件的 diff 展示可直接用。

#### 2.4 markdownFenceRepair.ts — LLM 围栏修复（P1，渲染正确性）

LLM 常把整篇答案/PR 草稿包进外层 ```md 围栏，导致前端把整篇当代码块、markdown 全不渲染。这里**只剥那层"草稿外壳"**，并把内部嵌套示例围栏加长一格以满足 CommonMark 闭合规则。MEMORY 里 flockmux 已踩过「Tailwind v4 灭 list-style」类渲染坑，这个围栏修复是另一个会咬人的真实坑，值得搬进 ChatMarkdown。

#### 2.5 MarkdownRenderer.vue — 富渲染全家桶（P1）

markdown-it（breaks/linkify/typographer）+ **KaTeX**（行内 `$..$` 与 ```latex/tex/math 围栏）+ **Mermaid**（懒加载 import、`securityLevel:'strict'`、每条最多 4 图、超长降级代码块、5s 超时、渲染期保持滚动位置）+ **本地文件智能化**：把 `src=/abs/path` 改写成 `/api/hermes/download?path=` 后端端点（对位 flockmux 「http 页加载不了 file:// 必走后端」的已知结论）、把指向 `.mp4/.webm/.mov` 的链接渲染成 `<video>`、其它本地文件渲染成可下载/可预览的文件卡片（`SUPPORT_PREVIEW_FILE_TYPES` 白名单走右侧 Drawer 预览）。标题自动加 `id` 供大纲锚点跳转。

#### 2.6 ConversationMonitorPane.vue — 只读会话监控（P1，正中 flockmux 痛点）

左会话列表（live badge / source / 时间 / 预览）+ 右消息流，**15s 轮询** `fetchConversationSummaries`/`fetchConversationDetail`，带 `requestId` 防竞态（旧请求结果丢弃）+ `humanOnly` 过滤 + 静默刷新不闪 loading。这正是 flockmux「旁观某个 worker 当前在干嘛」的只读视图原型——比内嵌完整聊天轻量得多。

#### 2.7 其它

- **OutlinePanel.vue**：纯前端从 user 消息抽问题 + assistant 正文抽 h1-h3 做层级大纲，点击经 `VirtualMessageList.scrollToAnchor` 跳转。长蜂群会话导航直接可用。
- **ChatInput.vue（单聊）**：`/slash` 命令下拉（usage/status/abort/queue/plan/goal·status·pause·resume·done·clear/subgoal/clear --history/title/compress/steer/destroy/reload-mcp）——**完整的会话内命令语言**，键盘导航同 mention；外加在输入框内联编辑模型上下文长度（`fetchContextLength`/`setModelContext`）。对位 flockmux CommandPalette，可把「派 worker / 合并方向 / 压缩」做成 slash 命令。
- **ChatPanel.vue（单聊）**：会话侧栏支持**批量模式**（多选删除 Popconfirm）、置顶分组、profile 过滤、按 provider 分组的模型选择器；头部审批条 + **澄清条 `.clarify-bar`**（agent 反问、用户选项回答）；右侧 Drawer（终端/文件）。
- **SessionSearchModal.vue**：防抖搜索 + 键盘 ↑↓/Enter 导航 + 跳转并把不在列表的会话动态加入。对位方向/会话全局搜索。
- **DrawerPanel.vue**：右侧抽屉，文件/终端两 tab，`min(1180px,88vw)` 宽，移动端全宽，`transition: right 0.3s`。

---

### 3. 与 flockmux 的差距对照（哪些是 flockmux 当前缺的）

- flockmux 蜂群聊天目前更偏「台账/消息流」，**没有 group-chat 这种"一个房间多 agent、@mention 指名、每 agent 独立 typing/replying 状态条、stacked 头像 + agent 增删"的成品形态**——HW group-chat 是最直接的升级蓝本。
- flockmux 实时进度靠读 JSONL→AgentActivity；HW 的 **`mapGroupMessages` 占位/回填 + 流式 delta（content/reasoning 双流）** 是更细粒度的渲染契约，可借鉴其「先 running 占位、后按 tool_call_id 合并 done」。
- flockmux 缺：消息虚拟化（VirtualMessageList）、unified diff 行号/折叠、围栏修复、KaTeX/Mermaid 富渲染、只读会话监控、大纲导航、会话内 slash 命令语言——本附录逐条给了成品参考。
- 注意差异：HW 用 Socket.IO + 房间模型（邀请码/克隆/压缩配置），flockmux 是 WS + workspace/方向/blackboard 模型；借鉴的是**前端呈现与交互**，不是直接搬后端协议。
