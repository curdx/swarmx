# hermes-web-ui 深度借鉴报告（2026-06）

> EKKOLearnAI / hermes-web-ui — 多 agent 工作台前端 (Vue3 + TS) + Koa BFF
>
> 由 8 个专读 agent 穷举产出 · 函数级条目 **445** · 框架思想 **105** · 页面元素 **134** · 借鉴点 **86**
> 
> 本文为机器穷举 + 结构化整理，未做删减；引用格式 `文件:符号`。

**目录**：0 定位与架构哲学 · 1 框架思想/设计模式 · 2 函数级地图（穷举）· 3 页面元素穷举 · 4 swarmx 借鉴小结

## 0. 定位与架构哲学

EKKOLearnAI 的 Agent 控制台前端（Vue3 + TS monorepo：client / server / desktop / skills / website）+ 一层 Koa BFF 把 claude / codex CLI 包成 HTTP。一句话：一个**功能完备的多 agent 工作台参考实现**——22 个页面、80+ 组件，覆盖单聊 / 群聊 / 文件浏览器 / 看板 / 定时任务 / 模型管理 / 用量 / 技能 / 设置。它对 swarmx 是「前端该长什么样、缺哪些页面」的最近邻样板；它的 BFF（codex-proxy / claude-code-proxy / safe-file-store / login-limiter）则与 swarmx 的 Rust 后端职责对位。

## 1. 框架思想 / 设计模式

### chat 组件(单聊) — hermes-web-ui packages/client/src/components/hermes/chat/

- **requestId / requestSeq 版本号防竞态** — `ConversationMonitorPane.vue::loadSessions/loadDetail; SessionSearchModal.vue::runSearch; ChatInput.vue::loadContextLength(用 key+Promise 串联)`
  - 每次异步请求前递增本地计数器，回调时比对当前值，不一致则静默丢弃。比 AbortController 轻量且不需要后端支持取消。swarmx 所有「切方向/切 agent 时刷新」场景都有这个竞态风险。
- **多帧 rAF 循环 + keepAliveMs 时间窗置底** — `VirtualMessageList.vue::scrollToBottom / scheduleScrollToBottom`
  - 动态高度虚拟列表在插入新消息时高度不稳定；单次 scrollTop 赋值可能被后续渲染覆盖。用 N 帧重试 + Ms 级时间窗保障「新消息后始终追底」，不依赖任何事件回调。
- **anchorToken 版本号取消并发对齐** — `VirtualMessageList.vue::cancelAnchorAlignment / scheduleAnchorAlignment`
  - OutlinePanel 点击跳转时若用户快速连续点击，token 递增让旧 rAF 链感知到自己已过期并退出。比 cancelAnimationFrame 单次取消更健壮（链式 rAF 只需检查 token）。
- **captureScrollPosition → 翻页 → restoreScrollPosition 保持视口** — `VirtualMessageList.vue; MessageList.vue::handleTopReach`
  - 上拉加载历史消息会导致 scrollHeight 增大，若不补偿 scrollTop 则内容向下跳。用 nextScrollTop = newScrollHeight - oldScrollHeight + oldScrollTop 精确还原。
- **模块级 Map 跨实例持久化滚动位置** — `MessageList.vue(sessionScrollPositions); HistoryMessageList.vue(historySessionScrollPositions)`
  - Vue 组件销毁时数据会丢失，而 Map 声明在 <script lang='ts'> 的模块作用域（非 setup 内），生命周期同模块。切回已访问会话时能恢复精确滚动位置。
- **Draft localStorage 按 sessionId 分组** — `ChatInput.vue::DRAFT_STORAGE_KEY / saveDraftForActiveSession`
  - 多会话切换时每个会话保留独立草稿，空草稿时删 key 节省存储，键超 0 条时移除整个 storage 项。比全局单草稿体验好得多。
- **slash 命令语言：/name [args] 内联下拉** — `ChatInput.vue::bridgeCommands / updateSlashState`
  - 只在 source==='cli'(bridge session) 时激活，不干扰普通会话。命令有 name+args+insertText(name/args 合并时需要) 三字段区分「显示」和「插入文本」，支持子命令如 'goal status'。
- **MarkdownRenderer 内联 DOM 改写：src/href 本地路径→后端端点** — `MarkdownRenderer.vue::renderedHtml (computed)`
  - http 页无法加载 file:// 资源；LLM 输出本地文件路径作为 markdown 图片/链接时，需要在前端渲染阶段统一替换为 /api/hermes/download。这与 swarmx MEMORY 中已知结论完全吻合。
- **Mermaid 占位 → 异步懒加载 → 超时 fallback** — `MarkdownRenderer.vue::renderMermaidDiagrams; mermaidRenderer.ts`
  - Mermaid 库较大，首次 import() 懒加载；渲染有 5s 超时防止挂起；超 4 个图或超 20k 字符降级为代码块；渲染前后保持滚动位置。三重兜底避免单个 diagram 卡死整个消息渲染。
- **工具结果多维度截断防 OOM** — `MessageItem.vue::truncateJsonValue`
  - 工具调用结果可能是数 MB 的 JSON，直接 v-html 会卡死浏览器。六维限制（深度/节点数/对象键/数组项/字符串长度/总字符数）独立生效，任一触发则截断。swarmx 工具结果展示面临完全相同的问题。
- **TTS 多 provider 策略路由** — `MessageItem.vue::handleSpeechToggle / 自动播放事件 auto-play-speech`
  - 5 个 provider(openai/custom/edge/mimo/webspeech)共用同一按钮，策略分支在 click handler 内；自动播放通过 window 自定义事件解耦（chatStore 触发，MessageItem 订阅）。不需要 provider 组件感知 chatStore 流式状态。
- **批量操作 = isBatchMode ref 控制 UI 模式切换** — `ChatPanel.vue::toggleBatchMode / selectedSessionKeys(Set)`
  - 不新增路由，而是本地 ref 切换「正常/批量」两套 UI（checkbox 出现，普通 click 改为 toggle-select）。Set 存 profile\0id 复合 key 防跨 profile 同 id 冲突。
- **ConversationMonitorPane 只读轮询：silent refresh 不闪 loading** — `ConversationMonitorPane.vue::loadSessions(silent=true)`
  - 15s 定时刷新时 silent=true 跳过 loading 指示器和 error 覆盖，只在请求成功后静默更新数据，不打断用户阅读。首次加载才显示 loading。
- **markdownFenceRepair：只剥最外层 md 围栏** — `markdownFenceRepair.ts::repairNestedMarkdownFences`
  - LLM 常将整篇 markdown 答案包进 ```md 外壳。算法只对「围栏闭合在文档最后一个非空行」的 markdown fence 执行剥离，保守性强。内部嵌套示例围栏长度自动提升以满足 CommonMark 规则。
- **SessionListItem 长按模拟 contextmenu（移动端兼容）** — `SessionListItem.vue::onTouchStart/End/Move`
  - 移动端没有右键。500ms 长按合成 MouseEvent('contextmenu') 含触点坐标，触发和桌面相同的下拉菜单流程。touchmove 清 timer 防滑动误触。

### group-chat + jobs + kanban

- **Room 粒度的 context 压缩配置（triggerTokens/maxHistoryTokens/tailMessageCount）** — `GroupChatPanel.vue + CreateRoomForm.vue`
  - context 压缩不是全局开关，而是 per-room 可配置策略。创建时内嵌在折叠区（NCollapse 默认收起，降低认知负担），运行时可修改并可立即 forceCompress。这意味着不同 room（对应不同任务边界）有独立的 token budget，与 agent 数量/任务复杂度解耦
- **@mention 全自定义（非 NDropdown，自绘 fixed 下拉）** — `GroupChatInput.vue::updateMentionState + mention-options.ts`
  - 用 DOM mirror span 精确计算 @ 符号像素坐标，position:fixed 贴光标显示，根据视口剩余空间自动 flip top/bottom。原因：NDropdown 挂在组件树内会有 z-index 和 IME 干扰；自绘则可完全控制 keydown 截取顺序，Enter/Tab 先 selectMention 再走发送逻辑，不会误触表单提交
- **多 provider TTS 统一调度（openai/custom/edge/webspeech/mimo）** — `GroupMessageItem.vue::playSpeech`
  - 单函数 playSpeech 通过 voiceSettings.provider 分发，各 provider 使用同一套 speech composable 不同方法（openaiPlay/mimoPlay/toggleBrowser）。autoplay 参数区分自动触发（window 事件）和手动点击（toggle 语义）。edge TTS 走自建后端代理避免浏览器跨域
- **Tool payload 分离 full/display 两份数据** — `GroupMessageItem.vue::formatToolPayload + truncateJsonValue`
  - 展示截断版（防止 DOM 爆炸），复制全量版（完整 JSON/diff），两者分开存储。复制按钮用事件委托（data-copy-source 属性）区分 args vs result，避免为每个 payload 注册 listener。truncateJsonValue 用 WeakSet 防循环引用，stringifyLength 动态判断阈值
- **虚拟列表 + 滚动位置还原（分页加载历史消息）** — `GroupMessageList.vue::handleTopReach + VirtualMessageList`
  - 滚到顶端触发 captureScrollPosition→loadOlderMessages→nextTick→restoreScrollPosition，避免新消息插入后视图跳跃。初次进 room 用 frames:5/keepAlive:700 多帧强制滚底（对抗异步渲染延迟），后续流式追加用 frames:1/keepAlive:120 轻量滚
- **Agent 审批权限五档（once/session/always/deny）明确在 UI 层分档** — `GroupChatPanel.vue visibleApproval + handleApproval`
  - 审批条目有 choices 字段（后端返回该 agent 支持哪些档位），前端 v-if 按 choices 渲染按钮，不做前端假设。这样不同 agent（claude/codex/未来其他）可差异化支持审批粒度
- **Job CRUD 中的 diff-only update（buildJobUpdateRequest）** — `jobs/JobFormModal.vue::handleSave`
  - 编辑时调 buildJobUpdateRequest 对比原始 job 和 formData，只发送变化字段，无变化则 no-op 直接返回成功。减少后端无意义 write，避免 cron 调度被误触重置
- **Job 投递渠道（deliver）按配置状态 disable 选项** — `jobs/JobFormModal.vue::isDeliverTargetConfigured + targetOptions`
  - 渠道是否可选取决于 settingsStore.platforms[key] 是否完整配置，各渠道 required 字段不同（token/extra.homeserver 等），在表单构造时预计算，disabled 选项阻止选中但保留可见，用户知道有这个渠道但未配置
- **Cron Run 历史懒加载详情（accordion expand on-demand）** — `jobs/JobRunHistory.vue::handleExpand`
  - 列表只加载 RunEntry（metadata），展开时才 readCronRun 获取完整 content 并渲染 Markdown。expandedContent/loadingContent 两个 Record 做缓存，同 key 不重复请求
- **Kanban 状态机（triage→todo→ready→running→blocked→done→archived）** — `KanbanTaskCard.vue::priorityLabel + KanbanColumn.vue::statusIcon + KanbanTaskDrawer.vue::canMutateTask`
  - 状态枚举驱动 CSS class（border-left 颜色）、按钮可见性（done/archived 不可再操作）、操作逻辑（blocked 显示 unblock，否则显示 block）。done/archived 进入终态，canMutateTask 统一判断，防止前端误操作
- **Kanban 任务详情抽屉内嵌 chat session 关联（search-sessions API）** — `KanbanTaskDrawer.vue::searchTaskSessions + historySession`
  - task 与 AI chat session 通过 task_id+profile+board 三元组关联，懒加载（点击才查询），结果可跳转到 chat 页面或 modal 内嵌 HistoryMessageList。这是任务管理和 AI 对话的双向打通：kanban 任务由 AI 执行，执行过程可溯源到具体对话
- **两步式危险操作确认（complete/block 需先展开输入框再提交）** — `KanbanTaskDrawer.vue::handleComplete + handleBlock`
  - 第一次点击仅展开 input（showCompleteInput/showBlockInput），第二次才真正提交。比 Popconfirm 更轻量，且 complete 可附带 summary，block 必须填 reason。避免误触同时收集必要信息
- **Race condition 防护：watch 回调里用快照比对** — `KanbanTaskDrawer.vue (watch taskId+selectedBoard)`
  - watch 监听两个状态，async 加载后检查 props.taskId===id && selectedBoard===board 再写 detail，避免快速切换任务时旧请求覆盖新状态。这是前端异步防竞态的标准写法
- **dragCounter 计数解决 dragLeave 误判问题** — `GroupChatInput.vue::handleDragEnter/Leave`
  - 子元素 dragenter/dragleave 会冒泡，导致拖拽经过子元素时触发 false leave。用 counter++ / counter-- 解决：counter>0 才算真正在 drop zone 内。是解决这个经典前端问题的标准方案
- **IME composing 兼容（isComposing + keyCode===229）** — `GroupChatInput.vue::handleKeydown`
  - 中日韩输入法组字期间不应触发 Enter 发送。检查 e.isComposing + e.keyCode===229（旧 IE/Edge 遗留），同时用 isComposing ref 跨事件持久化状态，compositionEnd 用 requestAnimationFrame 延迟更新（某些浏览器 compositionEnd 时 isComposing 还未还原）

### models + profiles + mcp + skills 组件

- **OAuth Device Flow 状态机（idle→loading→waiting→approved/expired/error）** — `CodexLoginModal / CopilotLoginModal / NousLoginModal / XaiOAuthLoginModal`
  - 四个 provider 用完全相同的 5 态状态机，差异只在 API 调用和个别额外状态（Copilot 多 denied）。自动启动（startLogin in mounted），setTimeout 递归轮询而非 setInterval，catch 继续轮询吸收抖动。xAI 无 user_code 直接 redirect，但状态机结构相同——说明状态机是协议无关的，只是驱动方式不同。
- **Provider Key 命名空间：builtin vs custom:xxx** — `ProviderCard.vue, ProviderFormModal.vue, models store`
  - 用 provider key 前缀（custom:）区分两类 provider，避免枚举类型。builtin 有预设 base_url/models，custom 需用户填。这使「添加预设 provider」和「添加自定义 provider」共用一套数据结构，扩展新 builtin 只需在服务端增加预设 key，前端无需改。
- **Auxiliary Task 表驱动配置（task_key → per-task model assignment）** — `AuxiliaryModelsPanel.vue, fetchAuxiliaryModels API`
  - 系统有 12 种内部任务（compression/vision/web_extract/skills_hub/approval/mcp/title_generation/triage_specifier/kanban_decomposer/profile_describer/curator/session_search），每种可独立指定 provider/model/timeout/extra_body。task 列表由服务端下发，前端只渲染。这实现了「主模型做对话，辅助模型做专项」的分工，等同于 swarmx 的 per-task role 配给。
- **Provider 可见性控制（include vs all 模式）** — `ProviderCard.vue:handleVisibilitySave, appStore.getProviderVisibility`
  - 用户可以对 provider 的模型列表做「隐藏」，过滤掉不想在下拉中看到的模型。存储为 {mode: 'all'\|'include', models: [...]}，全选时自动升级为 all 节省空间。这解决了大型 provider（如 Alibaba 上百个模型）的 UX 噪声问题。
- **Model Alias（别名层）** — `ProviderCard.vue:modelDisplayName, appStore.setModelAlias`
  - 在 model ID 和用户看到的显示名之间插一层别名映射，别名存储在 appStore（本地）。显示时：别名存在则显示别名+原始 ID（小号字）；不存在则显示原始 ID。此设计让用户可以给 gpt-4o-2024-11-20 起别名「GPT4o Latest」，不污染服务端配置。
- **Profile 作为隔离边界（多套独立配置）** — `ProfilesStore, ProfileCard, ProfileCreateModal`
  - 每个 profile 有独立的 model/provider/.env/SKILL.md/头像，切换 profile 等同于切换完整的 agent 配置快照。clone 时后端自动清理独占平台凭据（如 OAuth token）并把清理清单返回给前端。export/import 走 tar.gz，实现配置迁移和备份。
- **Profile Avatar：生成式 vs 图片双模式** — `ProfileAvatar.vue（multiavatar 库）`
  - 用 @multiavatar/multiavatar 库按 seed 确定性生成 SVG 头像。无用户上传时 seed=profile.name，保证同名 profile 在不同机器上看到一样的头像。用户可上传图片覆盖（type=image + dataUrl）。这避免了「所有用户都是同一个灰色图标」问题，且零存储成本。
- **MCP 卡片纯展示，副作用上浮（事件总线模式）** — `McpServerCard.vue`
  - McpServerCard 所有操作（edit/test/reload/remove/toggle）都 emit 给父级，自身不调 API。这使卡片可以在不同上下文（弹窗内、页面内）复用，状态管理统一在父级。对比 swarmx 的 agent 卡片，这种分离使 e2e 测试更容易 mock。
- **Skill 分层：category→skill→files（3 层树）** — `SkillList.vue, SkillDetail.vue, skills.ts`
  - skill 按 category 分组，每个 skill 包含 SKILL.md（描述文档）+ 附属文件（脚本/配置）。skill 有 4 种 source（builtin/hub/local/external）+ modified 标记。pin/enabled 是两个独立维度：enabled 控制是否被 agent 使用，pin 控制在 UI 中置顶。usage stats（view_count/use_count/patch_count）分别追踪查看/调用/编辑频次。
- **智能 preset 路由（apikey.fun URL 自动识别）** — `ProviderFormModal.vue:routeApiKeyFunCustomProvider`
  - 用户在 custom 表单填入 apikey.fun 的 base_url 并选模型后，系统自动识别并路由到对应内置 preset（fun-codex/fun-claude），无需用户手动切换。这类「智能路由」减少用户配置失误（如 base_url 正确但 provider key 选错导致 alias 不生效）。
- **Profile 切换用全页刷新而非 store 局部更新** — `ProfileCard.vue:performHermesSwitch`
  - profile 切换后直接 window.location.reload()。这看似粗暴，实则是正确的工程决策：profile 影响 models/skills/memory/settings 等大量全局状态，局部更新极易遗漏；全页刷新用浏览器缓存确保一致，代价是短暂白屏，但用户切换 profile 是低频操作，可接受。

### settings 全部面板(15)

- **SettingRow 原子布局组件 + slot 控件** — `SettingRow.vue，所有面板`
  - 把「label左/control右」这个布局提取为原子组件，所有面板只传 label/hint props + slot 控件，响应式逻辑集中一处。好处：所有行的间距、分割线、移动端折叠全部统一，新面板零样式负担。swarmx 设置页各面板行布局目前各自写 flex，应该抽同等原子组件
- **两阶段写入：乐观 updateLocal + 防抖 saveSection** — `AgentSettings / MemorySettings / CompressionSettings / SessionSettings 的 debouncedSave`
  - 数字输入框每次击键先写本地 store（UI 即时响应），300ms 后才发 HTTP。Switch 则直接保存不防抖。区分「输入型」和「选择型」控件使用不同策略，既快速又不暴力发 HTTP。swarmx 设置页目前 onChange 直接 fetch，应采用同等策略
- **PlatformCard 草稿隔离 + unsavedChanges 门控** — `PlatformSettings.vue: configDraft/credentialDraft/touchedConfig/hasUnsavedChanges`
  - 每个平台维护独立草稿，JSON.stringify 对比判断是否有未保存变更，Save 按钮 disabled 当无变更。避免用户手误点 Save 触发无意义 restart。swarmx MCP 管理页和平台连接类配置应搬这套：先写草稿、diff 后才允许提交
- **分层持久化：server-persistent vs client-only localStorage** — `useVoiceSettings.ts（纯 localStorage）vs useSettingsStore（Pinia + API）`
  - 语音设置纯前端体验，不需要同步到服务器，用 localStorage 模块级单例 ref；全局配置用 Pinia + REST。两者在使用层（组件）界面一致（都是 ref），选择哪层持久化由「是否要多端/多用户共享」决定。swarmx 也有类似场景：如 UI 主题偏好可 localStorage，模型配给必须 DB
- **PlatformCard configured computed 的多字段枚举检测** — `PlatformCard.vue:configured`
  - 不要求特定结构，而是枚举「所有可能的凭据字段名」检查任意一个非空即认为已配置。这样新平台加新字段不需要改 PlatformCard。swarmx CLI 连接状态展示可借鉴：check_configured 扫描 token/api_key/path 等多字段
- **GithubPreview 的软错误 vs 硬错误分离** — `GithubPreviewSettings.vue:runAction + applyErrorStatus`
  - API 返回 {success:false, message, code} 是软错误（正常 HTTP 200，业务层失败）；throw 是硬错误（网络/4xx/5xx）。两者分别处理，软错误 warning toast，硬错误 error toast，都尝试从响应 JSON 提取 status 回填 UI。swarmx 工具执行结果也有类似分层，值得统一
- **completionNotificationsReady 门控防初始化虚假通知** — `GithubPreviewSettings.vue:watch(last_action_completed_at)`
  - 组件挂载时先记录当前 completedAt，再置 ready=true，watch 在 ready 前的变化静默跳过。防止用户打开设置页时看到旧操作的完成通知。swarmx 活动日志/通知组件有同样风险
- **微信 QR 登录状态机 (idle→loading→waiting→scaned→confirmed/expired/error)** — `PlatformSettings.vue:startWeixinQrLogin/pollWeixinStatus`
  - OAuth/外部授权流程用显式状态枚举建模，每个状态对应不同 UI（按钮/spinner/提示文本），轮询用 setTimeout 递归而非 setInterval（防超时堆叠）。swarmx 如果将来加外部 OAuth 授权连接（GitHub/Slack/企业微信）可直接搬这个状态机模式
- **useVoiceSettings 模块级单例 ref 替代 Pinia** — `useVoiceSettings.ts`
  - 不用 Pinia store 而用模块级 ref，每次 useVoiceSettings() 返回同一批 ref 引用，watch 自动持久化。适用于「纯前端偏好、不需要跨用户共享、不需要 devtools 调试」的场景。比 Pinia 少一层注册，比 localStorage 读/写更响应式
- **LocalStorage schema 版本迁移（migrateOldKeys + sanitize）** — `useVoiceSettings.ts:migrateOldKeys/sanitize`
  - localStorage key 版本升级时先做数据迁移（重命名 provider/字段），再 sanitize 清除过期字段。防止旧用户升级后数据错乱。swarmx 若引入 localStorage 存 UI 偏好应从一开始带版本后缀和迁移逻辑
- **UserManagement DataTable columns 用 h() render 函数 inline 声明** — `UserManagementSettings.vue:columns`
  - 避免单独写 column 组件，直接在列定义里用 h(NTag/NButton/NPopconfirm) render，条件逻辑内联。快速但难维护；适合列数少、逻辑简单的管理表格。swarmx 的 agent 列表/workspace 列表已有类似模式
- **双节点 API 分离：config section vs credentials** — `PlatformSettings.vue:savePlatform（分别调 saveSection 和 saveCredsApi）`
  - 配置（非敏感，如 require_mention、free_response_chats）和凭据（敏感，如 bot token）走不同 API endpoint，敏感数据不在 store 中保留明文，只在 credentialDraft 中短暂存活。swarmx MCP API key 和 MCP 配置路由也应分离

### files + usage + layout + auth + common 组件

- **Store 作为单一事实源，UI 组件只读 store + 触发 action** — `useFilesStore / useUsageStore`
  - 文件浏览器的 editingFile/previewFile/entries 全部由 store 持有，所有组件（FileTree/FileList/FileEditor/FilePreview）通过同一 store 实例共享状态。避免了跨组件 prop drilling 或 event bus。FileContextMenu 通过 defineExpose({show}) 暴露命令式接口，保持调用处简洁。
- **乐观更新 + 竞态保护（requestId token）** — `useUsageStore.loadSessions / ProfileSelector.loadRuntimeStatuses`
  - 用模块级递增 latestRequestId 做取消：新请求覆盖 token，旧 Promise resolve 时对比 token 决定是否丢弃结果。比 AbortController 轻量，适合不需要真正中断网络请求的场景（结果来得太晚丢掉就好）。
- **派生数据全在 computed 中计算，原始 API 数据仅存 ref(stats)** — `useUsageStore`
  - dailyUsage/modelUsage/cacheHitRate 等展示数据完全是 computed，API 只返回原始 token 计数和 cost。百分比/颜色/排序全在前端派生。好处：图表动画（CSS transition）可以对 computed 变化响应，且数据不冗余。getModelColor 用字符串哈希绑定颜色保证跨图表一致性。
- **文件类型判断集中在 store 层，组件通过调用函数而非自行猜扩展名** — `stores/hermes/files.ts: isTextFile / isImageFile / isMarkdownFile / isPreviewableFile`
  - 所有判断集中在一处，TEXT_BASENAMES 处理无扩展名文件（README/LICENSE/Makefile 等），TEXT_EXTS 用 Set 做 O(1) 查询。组件层只调 isTextFile/isPreviewableFile，变更只需改 store。
- **删除/重命名时主动失效相关 UI 状态（isAffected）** — `useFilesStore.deleteEntry / renameEntry`
  - 不依赖组件自己感知，而是在 action 中统一检测 previewFile 和 editingFile 的 path 是否受影响（支持目录前缀匹配），避免编辑器持有已删文件的内容、保存时 404 的 bug。
- **文件下载绕过 request 封装——token 放 query param** — `api/hermes/download.ts:getDownloadUrl / api/hermes/files.ts:getFileDownloadUrl`
  - <a href> 和 <img src> 无法注入自定义 Header，只能把 Bearer token 作为 query param 传给后端。同时加防双包装保护：检测 filePath 是否已经是 /api/hermes/download? 开头，先解包再重构；并做 decodeURIComponent 防双重编码。这是 AI 回复中路径被二次拼接的真实 bug 防御。
- **分组折叠状态持久化——command-flush 模式（手动 persist，不自动 watch）** — `composables/usePersistentRecord.ts / AppSidebar.vue`
  - usePersistentRecord 返回 reactive record + persist() 函数，调用方在 toggleGroup 末尾手动调 persistCollapsedGroups()，不是每次 reactive 变化都写 localStorage。修改立即生效在内存，持久化是显式操作，避免频繁 IO。
- **主题系统用模块级单例 ref，所有组件共享同一响应式状态** — `composables/useTheme.ts`
  - brightness/style 定义在模块顶层（不在 composable 函数体内），多次调用 useTheme() 得到同一引用。watch 也在模块级注册，只跑一次。比 Pinia store 更轻量，适合不需要 devtools 追踪也不需要 SSR 的全局单例。
- **Auth 通过 CustomEvent 总线（非 WS、非 store）广播 401/403** — `components/auth/AuthEventListener.vue`
  - HTTP 请求拦截器触发 window.dispatchEvent(new CustomEvent('hermes-auth-notice',...))，AuthEventListener 组件监听并 toast 提示。职责分离：API 层只抛事件，UI 层只显示；1200ms 防抖避免批量 401 轰炸用户。
- **RouteLinkItem：RouterLink v-slot custom 模式解耦激活逻辑** — `components/common/RouteLinkItem.vue`
  - active prop 可外部传入（处理多子路由映射同一激活态的场景），fallback 到 Vue Router 的 isActive/isExactActive。一个 wrapper 统一处理 a11y（aria-current='page'）和导航逻辑，消除各页面散布的 :class=isActive 代码。
- **ModelSelector：disabled 双重防护（列表点击 + custom 输入框都检查）** — `components/layout/ModelSelector.vue:handleSelect / handleCustomSubmit`
  - disabled 检查同时在 handleSelect（点击列表项）和 handleCustomSubmit（输入框 Enter）中执行，注释明确写明「防止 custom input 绕过列表里的灰显限制」。这是防御性编程的典型：UI 层的视觉禁用不够，需要逻辑层也验证。
- **指数退避轮询 + 可取消（scheduleRuntimeStatusPoll）** — `components/layout/ProfileSelector.vue:scheduleRuntimeStatusPoll`
  - 首次 700ms（用户打开弹窗后很快看到最新状态），后续 1200ms，最多 12 次自动停止；loadRuntimeStatuses 的 refreshing 返回值决定是否继续轮询；modal 关闭时 showProfileModal=false 使下次 setTimeout 回调提前退出。无需 clearTimeout，通过状态驱动。

### views 全部(22) — hermes-web-ui

- **View-as-thin-shell + store + component 三层分离** — `所有 22 个 view`
  - View 只负责生命周期初始化和路由同步，业务状态全在 Pinia store，UI 拆到专属 component。好处是 view 文件可读性极高，可以把 view 当作「页面入口配置文件」快速理解全貌
- **请求序列号（requestSeq/requestId）防竞态** — `HistoryView.vue::loadHermesSessions, SkillsUsageView.vue::loadStats`
  - 两处都用模块级自增整数+闭包捕获来丢弃旧请求的响应，而非 AbortController，实现简单且零依赖；适用于「频繁切换期间/周期」导致多并发的场景
- **路由-状态双向同步：route.query 作为持久化的 UI 状态** — `KanbanView.vue(board)、SettingsView.vue(tab)、HistoryView.vue(sessionId+profile)`
  - 用 watch(route.query...) + router.replace 使 URL 成为单一真值，刷新/分享链接自动恢复状态；normalizeTab 做防御性校验防止非法 query 值
- **指数退避自动重试(loadServers)** — `McpManagerView.vue`
  - 连接失败不立即重试，2^n 秒递增(2→32s)，最多 5 次，scheduleReload 用 clearTimeout 幂等调用；适合等待 MCP 进程冷启动的场景
- **config dirty 检测 = originalContent vs content** — `CodingAgentsView.vue::hasConfigUnsavedChanges`
  - 不用 Vue reactive deep watch，只在保存成功后更新 originalContent，逻辑简洁且不需要递归比较对象
- **Terminal 实例 DOM 复用(移动元素不重建)** — `TerminalView.vue::mountActiveTerminal`
  - xterm.js Terminal 初始化 open() 后保留 DOM 元素，切换 session 时用 appendChild 移动而不销毁，保留 scrollback buffer；首次才 open()，entry.opened 标记控制
- **多 session 状态完全独立于 chatStore (historySession vs activeSession)** — `HistoryView.vue`
  - History 页有自己的 historySessionId/historySession，不共享 chatStore.activeSession，避免查看历史时干扰当前聊天状态
- **按 source 分组+折叠，状态持久化到 localStorage** — `HistoryView.vue::groupedSessions + collapsedGroups`
  - api_server 置顶、cron 置底的排序规则由 sourceSortKey 数值控制；折叠状态序列化为 JSON 数组存 localStorage，刷新后复原
- **MCP config 接受 JSON/YAML 双格式 + 自动格式化(防抖1500ms)** — `McpManagerView.vue::handleInput/handleModeChange`
  - 用户粘贴任意格式后实时校验，通过后在 1500ms 无操作后自动美化；YAML 用 js-yaml 的 JSON_SCHEMA 确保兼容性；格式切换时相互转换已有内容
- **SkillsUsage 自定义纯 CSS 堆叠条形图(无图表库)** — `SkillsUsageView.vue::chartSegments + skill-bar-fill/segment`
  - flex 布局实现堆叠：.skill-bar-fill 用 flex-direction:column-reverse，segment 用 flex:count(比例) 实现百分比栏；完全零依赖，颜色调色板手工维护
- **Kanban SSE 实时刷新 + 15s 轮询 fallback** — `KanbanView.vue::kanbanStore.startEventStream() + setInterval`
  - 优先 SSE 推送，同时设 15s 轮询双保险；visibilityState 检测标签不可见时跳过轮询，节省资源
- **per-period 结果缓存(statsByPeriod Map)** — `SkillsUsageView.vue::statsByPeriod`
  - 切回已加载周期时立即展示缓存，loading 状态用 isRefreshing 区分「首次加载」和「后台刷新」；防闪烁
- **CodingAgents launch 准备与执行分离** — `CodingAgentsView.vue::launchBuiltInTerminal vs launchNativeTerminal`
  - prepareCodingAgentLaunch 服务端生成 shellCommand(注入 env/auth)，前端拿到命令后可选两条路：嵌入式 xterm 展示或唤起宿主原生终端；解耦准备和执行
- **batchDelete 用 NUL 字节复合 key 解决跨 profile 同 ID 冲突** — `HistoryView.vue::sessionSelectionKey`
  - profile 和 id 都是任意字符串，用  作分隔符可100%避免拼接歧义，Set 结构自动去重

### 前端架构(状态/路由/api/i18n/composables)

- **模块级单例 ref 替代 store（composable singleton pattern）** — `useTheme / useSessionSearch / useToolTraceVisibility / useVoiceSettings / useGlobalSpeech`
  - 轻量全局状态不值得开一个 Pinia store，在 .ts 文件模块作用域内声明 ref 即为单例（ESM 模块只执行一次），所有 useXxx() 调用共享同一 ref，避免 store 样板代码。代价是无法 devtools inspect，适合 UI 偏好类轻量状态
- **requestId / generation 竞态保护** — `useUsageStore(latestRequestId), useKanbanStore(boardGeneration+多个requestSeq)`
  - async 请求返回顺序不确定，用自增 requestId 在 finally 阶段丢弃陈旧响应。kanban 额外引入 boardGeneration（board 切换时递增）使整个 board 范围内所有飞行请求失效，是两维度竞态保护
- **Socket.IO 全局事件分发 + sessionEventHandlers Map** — `api/hermes/chat.ts`
  - 单一 Socket.IO 连接服务所有 session，通过 sessionEventHandlers Map<sessionId, handlers> 按 session_id tag 路由事件，避免多 socket 连接的资源浪费。run.completed 时自动 delete map entry 防内存泄漏
- **先乐观更新后 server 确认 + 竞态安全的 profile-scoped storage** — `stores/hermes/app.ts(switchModel), stores/hermes/session-browser-prefs.ts`
  - UI 立即反映用户操作，server 请求在后台进行；storage key 带 profile 前缀，切 profile 时 watch + reload 自动隔离不同用户的偏好
- **JWT 客户端解码用于路由守卫（不依赖 server round-trip）** — `api/client.ts:getStoredUserRole, router/index.ts:isStoredSuperAdmin`
  - super_admin 页面鉴权在路由守卫客户端完成，JWT payload 无需验签（server 已在 HTTP 层验证）。用于 UX（隐藏菜单项/拒绝进入路由），不作为安全边界，真正鉴权在 API 层
- **Merge-with-fallback i18n（递归 deep-merge，非整体 fallback）** — `i18n/messages.ts:mergeMessagesWithFallback`
  - vue-i18n fallbackLocale 在 key 缺失时回退到整个 fallback locale，但 nested object 下某个子 key 未翻译会导致整个父对象 fallback 为 undefined。自定义 merge 逐 key 递归，只回退缺失的叶节点，保证部分翻译完成的语言不丢失已翻译的其他 key
- **LocalStorage 写入 QuotaExceededError 自愈** — `stores/hermes/chat.ts:setItemBestEffort / recoverStorageQuota`
  - LocalStorage 5MB 限制在大量会话缓存下可能超限。setItemBestEffort 捕获 QuotaExceededError，先清理已知废弃 prefix 的旧 key，再重试一次，仍失败则静默降级为纯内存（cache is best-effort 语义）
- **playbackToken 防 TTS 竞态 + 多引擎优先级 fallback** — `composables/useSpeech.ts`
  - 每次新播放自增 playbackToken（整数），所有异步回调（onended/onerror）先检查 token 是否仍是当前值再执行状态更新，防止前一次播放的回调污染后一次。引擎链：server TTS → browser WebSpeech，server 失败时在同一 token 上 fallback，不发起新 token
- **hash 路由（createWebHashHistory）+ URL token 在 router 初始化前读取** — `router/index.ts, main.ts`
  - hash 路由不需要 server 配置（适合作为 sidecar 静态部署），但 hash router 会在初始化时剥离 search params。main.ts 在 app.use(router) 之前就读取 URL token 并存入 window.__LOGIN_TOKEN__，解决 token 被 hash router 截断的问题
- **FOUC 防护：主题类在 createApp 之前写入 documentElement** — `main.ts`
  - Vue 应用 mount 有延迟，期间浏览器用默认样式渲染，暗色模式会闪白。在任何 JS 逻辑之前（module 顶层）读 localStorage 并写 class，与 CSS 'html.dark {...}' 配合，页面一出现就是正确主题
- **tab 可见性重同步 + document.visibilitychange** — `stores/hermes/chat.ts（visibilitychange listener）`
  - 后台 tab 不接收实时事件，切回前台时重新 resume session，对齐 server 上的最新消息和 isWorking 状态。这是 SPA 多 tab 场景的常见但易忽视的必要处理
- **ability/capability 发现式 API（Kanban）** — `stores/hermes/kanban.ts:isCapabilitySupported / assertCapability`
  - 后端不同版本支持的功能不同（bulk/links/taskLog/events 等），frontend 在使用前先查 capabilities 响应，不支持的功能在 UI 层隐藏/禁用，调用前 assertCapability 抛出明确错误。这比假设 server 版本更健壮
- **WeakSet 去重事件处理** — `stores/hermes/chat.ts:seenSessionCommandEvents WeakSet`
  - Socket.IO 重连或事件回放可能导致同一 RunEvent 对象被 handler 处理两次。用 WeakSet 记录已处理的事件对象（不阻止 GC），第二次收到同一对象引用时直接跳过

### Koa BFF server + other packages (hermes-web-ui)

- **API 格式中间层代理（Protocol Adapter Proxy）** — `services/claude-code-proxy.ts + services/codex-proxy.ts`
  - claude CLI 只接受 Anthropic Messages 格式，codex CLI 只接受 OpenAI Responses API 格式。服务端在本地注册 routeKey→target 映射，对外暴露统一本地 URL，内部做双向协议转换（含 SSE 流式转换）。token 是随机生成的 Bearer 令牌，防止本地代理被外部访问。每种 apiMode 是独立的适配分支，扩展新 API 格式只加新分支不改主路由。
- **Context Window 双路径压缩（Incremental vs Full）** — `services/hermes/context-engine/compressor.ts:ContextEngine`
  - Path A（有快照）：只压缩快照后的新消息，增量更新摘要；Path B（无快照）：全量压缩，保留末尾 tailMessageCount 条原文。两条路径都有降级逻辑：压缩失败时 trimToBudget 截断。per-room Promise 锁防并发快照覆写。设计允许接入外部 contextTokenEstimator 精确计算（比字符估算准确），但自有 CJK-aware 字符估算作为 fallback。
- **Promise 链 per-key 互斥锁（无 Mutex 库）** — `services/safe-file-store.ts:SafeFileStore.withLock`
  - 用 Map<string, Promise> 记录每个文件路径的进行中操作，新操作 .then 链接到当前末尾，任务完成后从 Map 删除。无外部依赖，完全利用 Promise 语义。同样模式用于 ContextEngine._compressLocks。
- **登录限流双层状态机（IP级 + 全局）持久化** — `services/login-limiter.ts`
  - 两个独立 IP map（password/token）交叉检查（password 失败也锁 token 路径），全局窗口计数防分布式攻击。脏标记 + 2s 防抖异步持久化，锁定时同步写文件（确保重启后不丢失）。IP map 上限 10000，超出按 lockedUntil 清理。
- **Koa 中间件数组注入（auth 作为参数而非全局中间件）** — `index.ts:bootstrap → registerRoutes(app, [requireUserJwt, resolveUserProfile])`
  - auth 中间件以参数形式传入路由注册函数，而非全局 app.use。这使特定路由可以选择性跳过或替换 auth 逻辑，测试也更容易 mock。
- **CLI-as-数据库（Kanban 完全委托 CLI）** — `services/hermes/hermes-kanban.ts`
  - 所有 kanban 操作（CRUD/状态机/dispatch/watch）全部 exec `hermes kanban ...` 子进程，服务端不维护 kanban 状态，只做参数验证和 JSON 解析。这让 kanban 数据保持与 CLI 工具单一真相，但代价是每次操作都有子进程开销。getCapabilities 静态声明 supported/partial/missing 矩阵，是少见的「功能清单自文档化」模式。
- **群聊消息 ID 相位排序（phase-based ordering）** — `services/hermes/group-chat/index.ts:sortGroupMessages + groupRunOrder`
  - 消息 ID 编码语义：baseId_part_N_toolcall_X / _toolresult_X 等。排序时提取 baseId+phase（0=toolcall, 1=toolresult, 2=assistant），保证同一 agent run 的 tool 调用链按正确顺序显示。并发多 agent 回复时按最早同 baseId 消息的时间戳分组排序。
- **两阶段网关启动（顺序分配端口 + 并行启动进程）** — `services/hermes/gateway-manager.ts:GatewayManager.startAll`
  - Phase 1 串行：检测现有进程状态、清理僵尸、为每个 profile 分配端口（避免竞争）；Phase 2 串行启动（注释说并行但实现是 for...of）：每个 gateway run --replace。端口分配维护 allocatedPorts Set 防同次启动中不同 profile 抢到同端口。
- **Agent Bridge 子进程就绪感知（stdout JSON line protocol）** — `services/hermes/agent-bridge/manager.ts:AgentBridgeManager.startProcess`
  - Python 桥接进程启动后读取 stdout 逐行解析 JSON，遇到 {event:'ready'} 才认为就绪。desktop 模式额外加 TCP 轮询（因为某些场景 stdout 可能不及时 flush）。自动重启使用指数退避（delay × attempts，上限 30s）。
- **用量可观测（Usage Store with cache/reasoning token 分解）** — `db/hermes/usage-store.ts`
  - 每次 LLM 调用写一条 usage 记录（input/output/cache_read/cache_write/reasoning tokens + model + profile），支持按 session、model、天聚合查询。SQLite 不可用时 fallback 到 JSON 文件（同一接口）。这是非侵入式成本监控的标准模式。
- **手写 HS256 JWT（无外部库）** — `middleware/user-auth.ts:signUserJwt / verifyUserJwt`
  - 只依赖 Node.js 内置 crypto（createHmac + timingSafeEqual），避免引入 jsonwebtoken 等依赖。payload 包含 aud='hermes-web-ui' 防止跨服务 token 复用，exp=30天。resolveUserProfile 从 header/query/body 多处读取 profile，集中处理避免各 route 重复。
- **Skills 目录在启动时强制覆盖同步** — `services/hermes/skill-injector.ts:HermesSkillInjector.injectMissingSkills`
  - 每次启动删除并重建所有 bundled skill 目录（非增量），保证内置技能版本与应用版本一致。对所有 profile 目录都同步，解决多 profile 场景下技能不一致问题。
- **群聊 mentionDepth 控制 agent 链式调用深度** — `services/hermes/group-chat/index.ts:handleMessage`
  - 每条消息携带 mentionDepth，agent 回复消息的 mentionDepth+1 传递，超过上限（默认4，最大10）后不再触发 @mention 路由，防止 agent 间无限循环。人类消息始终从 depth=0 开始。

## 2. 函数级地图（穷举）

### chat 组件(单聊) — hermes-web-ui packages/client/src/components/hermes/chat/  ·  60 项

- `chat/ChatInput.vue::startResize` — 顶部 resize-handle 鼠标拖拽改 textarea 高度(20-400px)；记录 startHeight/startY，document 级 mousemove/mouseup 监听，body cursor/userSelect 临时覆盖
  - 💡 height=null 表示「自适应」；手动拖过后改变 textareaHeight ref，不再被 scrollHeight 覆盖
- `chat/ChatInput.vue::updateSlashState` — 扫描 textarea 光标前文本，检测 / 开头且无空格/换行时激活 slash 命令下拉；只在 isBridgeSession(source==='cli') 时生效
  - 💡 slashQuery=光标前 /XXX 的 XXX 部分，filteredBridgeCommands 实时过滤
- `chat/ChatInput.vue::selectBridgeCommand` — 选中 slash 命令条目，将 /name 插入 textarea 并移动光标到末尾
- `chat/ChatInput.vue::loadContextLength` — 按 profile/provider/model 三元组做 key 缓存，防重复请求(同 key 串联 Promise)；切会话或模型变化自动刷新
- `chat/ChatInput.vue::saveContextLimit / handleEditContextLimit` — 点击 context-limit 数字弹 modal，调 setModelContext API 持久化上下文长度限制
- `chat/ChatInput.vue::handlePaste` — 拦截 ClipboardEvent，过滤 image/* item，转成 File 对象加入 attachments；阻止默认粘贴以避免文字乱入
- `chat/ChatInput.vue::handleDragEnter/Leave/Drop` — dragCounter 计数防子元素闪烁；drop 后重置 counter，addFile 批量处理；只响应 Files 类型 dataTransfer
- `chat/ChatInput.vue::saveDraftForActiveSession / loadDraftForActiveSession` — 按 sessionId 为 key 将输入草稿持久化到 localStorage(hermes_chat_input_drafts_v1)；切 session 时自动恢复
- `chat/ChatInput.vue::isImeEnter` — isComposing \|\| e.isComposing \|\| e.keyCode===229 三重保护，防 IME 拼字中途回车误发
- `chat/ChatInput.vue::formatTokens` — 数字格式化：>=1M 显示 xM，>=1K 显示 xK，否则原数；用于 context-info 显示 token 用量
- `chat/ChatPanel.vue::sortSessionsWithActiveFirst` — 按 updatedAt 降序排列 session 列表
- `chat/ChatPanel.vue::handleContextMenu` — 右键 session 项时手动定位 NDropdown(contextMenuX/Y)，触发 pin/rename/setWorkspace/setModel/export/copy-link/open-link 等操作
- `chat/ChatPanel.vue::handleBatchDelete` — 批量删除选中 session：收集 {id, profile} 数组，调 batchDeleteSessions API，成功后本地刷新+清理 pinned 记录
- `chat/ChatPanel.vue::openSessionModelModal / selectSessionModel` — 每个 session 独立切换模型；按 profile 聚合 modelGroups，支持搜索过滤、自定义 model 输入、preview/disabled badge
- `chat/ChatPanel.vue::handleClarify` — 处理 agent 澄清请求：有 choices 则选项按钮、无 choices 则文本输入；调 chatStore.respondToClarify
- `chat/ChatPanel.vue::syncNewChatModelSelection` — 新建会话时按 profile 的 default_provider/default_model 字段自动预选供应商+模型
- `chat/ChatPanel.vue::parseExportKey` — 将 'export-full-json' 等 dropdown key 解析成 {mode, ext}，支持 full/compressed × json/txt 四种导出
- `chat/VirtualMessageList.vue::scrollToBottom` — 调用 scheduleScrollToBottom(frames, keepAliveMs)；keepBottomUntil 时间窗内强制保持在底部，多帧重试应对动态高度渲染延迟
- `chat/VirtualMessageList.vue::scrollToAnchor` — 先 scrollToItem(index, start) 粗定位，再 scheduleAnchorAlignment(token, 10帧) 精确找真实 DOM 锚点并 alignElement；token 机制防旧对齐任务干扰新请求
- `chat/VirtualMessageList.vue::captureScrollPosition / restoreScrollPosition` — 上拉翻页前快照 {scrollTop, scrollHeight}，翻页后用 scrollHeight 差值反推新 scrollTop，保持视口内容不跳动
- `chat/VirtualMessageList.vue::captureViewportPosition / restoreViewportPosition` — 切会话时保存/恢复用户上次滚动位置；记录 wasNearBottom，回来时若原本在底部则直接 scrollToBottom
- `chat/VirtualMessageList.vue::handleResize` — ResizeObserver 回调：若 keepAliveMs 窗口内或已接近底部则追底，若有活动锚点对齐则重新对齐
- `chat/VirtualMessageList.vue::cancelAnchorAlignment` — 递增 anchorToken，取消当前 rAF 链；防止并发的 scrollToAnchor 互相干扰
- `chat/MessageList.vue::currentToolCalls` — 计算当前轮次（最后一条 user 消息之后）的所有 tool 消息，倒序排列用于 tool-calls-panel 展示
- `chat/MessageList.vue::displayMessages` — 过滤掉：(1) 当前轮次正在运行的 tool 消息；(2) 流式时只有 reasoning 无 content 的空 assistant 气泡
- `chat/MessageList.vue::handleTopReach` — 滚到顶时触发 loadOlderMessages，先 captureScrollPosition，翻页后 restoreScrollPosition 保持视口
- `chat/MessageList.vue::applyInitialSessionScroll` — 切 session 初始化滚动策略：focusMessageId 优先→上次 viewport 快照→默认 scrollToBottom；pendingInitialScrollSessionId 防竞态
- `chat/MessageList.vue::formatToolDuration` — 工具耗时格式化：<1s 显 Nms，<60s 显 N.Ns，>=60s 显 Nm Ns
- `chat/MessageList.vue::queuedPreview` — 将排队消息内容规范化(多空白压缩)后截取前48字符+省略号用于队列浮窗预览
- `chat/MessageItem.vue::truncateJsonValue` — 深度/节点数/键数/数组项/字符串长度多重上限安全截断 JSON，防止巨型工具结果卡死渲染；含循环引用检测(WeakSet)
  - 💡 六个维度独立限制：JSON_MAX_DEPTH=6, JSON_MAX_NODES=1000, JSON_MAX_KEYS=50, JSON_MAX_ITEMS=50, JSON_STRING_LIMIT=200, TOOL_PAYLOAD_DISPLAY_LIMIT=1000
- `chat/MessageItem.vue::formatToolPayload` — 工具 args/result 格式化：先 JSON.parse+stringify pretty，尝试 extractUnifiedDiffPayload 挖 diff；超限则调 truncateJsonValue；非 JSON 走 isUnifiedDiffContent 启发式判断
- `chat/MessageItem.vue::parseContentBlocks` — 解析 ContentBlock[] 格式的 message content；兼容 Python str(list) 格式（替换 None/True/False/'→"）
- `chat/MessageItem.vue::handleSpeechToggle` — 多 TTS provider 路由：openai/custom/edge/mimo/webspeech 各走不同 speech 方法；按 voiceSettings.provider 分支
- `chat/MessageItem.vue::ensureTick` — watchEffect 驱动：thinking 流式进行时每秒 setInterval 更新 nowTick，停止后 clearInterval；避免全局 timer 泄漏
- `chat/MessageItem.vue::getFilePathFromContent` — 从 message content 中提取上传文件路径：先解析 ContentBlock JSON，fallback 到正则匹配 [File: name](path) markdown 格式
- `chat/ConversationMonitorPane.vue::loadSessions / loadDetail` — requestId 计数器防竞态：若新请求在旧请求返回前完成，旧请求结果静默丢弃；silent 模式刷新不触发 loading 闪烁
- `chat/ConversationMonitorPane.vue::invalidateRequests` — sessionsRequestId/detailRequestId 双双递增，同时废弃 sessions 和 detail 的进行中请求
- `chat/OutlinePanel.vue::extractAllHeadings` — 从 assistant 消息正文提取 h1-h3（先剥 <think> 块），生成带层级的锚点 ID 列表；headingIndex 全局递增对应 MarkdownRenderer 的 heading-N 命名
- `chat/OutlinePanel.vue::outlineItems` — computed：遍历 user/assistant 消息对，每个 user 消息抽 Q 标签，后续 assistant 消息抽标题树
- `chat/SessionListItem.vue::onTouchStart/End/Move` — 500ms 长按模拟 contextmenu（合成 MouseEvent 含 touch 坐标），touchmove/touchend 清 timer 防误触
- `chat/SessionSearchModal.vue::runSearch` — 160ms 防抖；requestSeq 计数器防竞态；同时支持 profileFilter 过滤
- `chat/SessionSearchModal.vue::openItem` — 打开搜索结果：若 session 不在 store 中则 addOrUpdateSession 注入再 switchSession；支持 matched_message_id 跳转到具体消息
- `chat/MarkdownRenderer.vue::renderedHtml (computed)` — markdown-it render → 标题自动加 id → 本地 img src 改写为 /api/hermes/download 端点 → 本地链接转 video player 或 file card → @mention 高亮
- `chat/MarkdownRenderer.vue::renderMermaidDiagrams` — 渲染 data-mermaid-pending 占位元素：按 MERMAID_MAX_DIAGRAMS_PER_MESSAGE=4 限制，超出 fallback 代码块；每个图有 5s 超时；渲染前保持滚动位置
- `chat/MarkdownRenderer.vue::renderLatexFence / isLatexFence` — 识别 latex/tex/math/katex 围栏，调 KaTeX.renderToString 渲染为 displayMode HTML；不抛错(throwOnError:false)
- `chat/MarkdownRenderer.vue::withTimeout` — Promise.race 实现超时：N ms 后 reject；清理 timer 用 finally
- `chat/MarkdownRenderer.vue::getScrollParent / isNearScrollBottom` — 渲染 Mermaid 前/后保持滚动位置：找最近可滚动祖先，渲染后若近底部则恢复到底
- `chat/highlight.ts::renderUnifiedDiffCode` — unified diff 按行渲染：行号分 old/new/context 三列，高亮 added/removed/hunk-header；上下文超 8 行折叠为「⋮ N unchanged lines」
- `chat/highlight.ts::collapseFoldableContextRows` — 将连续 foldableContext 行超 DIFF_CONTEXT_FOLD_THRESHOLD=8 时折叠，保留头尾各 DIFF_CONTEXT_FOLD_EDGE_LINES=3 行
- `chat/highlight.ts::extractUnifiedDiffPayload` — 从工具结果 JSON 对象递归查找 diff/patch/stdout/output/content 字段，找到有效 unified diff 内容则提取出来以 diff 语言渲染
- `chat/highlight.ts::isUnifiedDiffContent` — 启发式判断：含 >=2 行文件头(diff --git/---/+++) 且有 @@ hunk 头
- `chat/highlight.ts::renderHighlightedCodeBlock` — 主入口：先判断是否 diff(语言标记或内容启发式)→走 renderUnifiedDiffCode；否则走 hljs.highlight 语法高亮；输出带 copy-btn 的 pre 包装
- `chat/highlight.ts::handleCodeBlockCopyClick` — 事件委托：找 [data-copy-code] button 的最近 [data-copy-text] 祖先取全量内容，fallback 到 code.hljs innerText
- `chat/markdownFenceRepair.ts::repairNestedMarkdownFences` — 剥去 LLM 把整篇答案包进的 ```md 外层围栏，并自动加长内部嵌套围栏满足 CommonMark 闭合规则
  - 💡 只剥「最外层且闭合在文档最后」的 markdown fence；不动其他 fence
- `chat/markdownFenceRepair.ts::promoteMarkdownExampleFences` — 扫描内容中的 markdown 示例围栏，若内部出现等长围栏则外层长度+1 提升
- `chat/mermaidRenderer.ts::renderMermaidPlaceholder` — 生成 data-mermaid-pending=true + data-mermaid-source=encoded HTML 占位 div；source 用 encodeURIComponent 编码避免 HTML 注入
- `chat/FolderPicker.vue::toggleExpand` — 懒加载子目录：首次展开时请求 /api/hermes/workspace/folders?path=XXX，结果缓存到 childrenCache Map
- `chat/FolderPicker.vue::flatNodes (computed)` — 将树形目录展开为扁平 FlatNode[] 数组(含 depth/isExpanded)，避免递归组件渲染
- `chat/TerminalPanel.vue::TERMINAL_THEMES` — 内置4套 xterm 主题(default/solarized-dark/tokyo-night/github-dark)，存 localStorage，NSelect 切换
- `chat/HistoryMessageList.vue` — 历史消息列表：复用 VirtualMessageList，接受 props.session 覆盖 chatStore.activeSession，支持独立滚动位置快照(historySessionScrollPositions Map)

### group-chat + jobs + kanban  ·  61 项

- `group-chat/GroupChatPanel.vue::profileAvatarFor` — 根据 profileName 从 profilesStore 里找 profile 的 avatar 对象，找不到返回 null
  - 💡 避免直接遍历，用 find+optional chaining，返回类型可为 null，调用处直接传给 ProfileAvatar
- `group-chat/GroupChatPanel.vue::agentAvatarName` — 从 RoomAgent 取显示名优先级：profile > name > agentId
  - 💡 fallback 链很清晰，三合一没有嵌套 if
- `group-chat/GroupChatPanel.vue::userMemberAvatar` — computed：从 members 找当前用户，解析 avatar JSON（类型为 image+dataUrl），安全 fallback 到 currentUserAvatar，malformed JSON 吞掉错误
  - 💡 两次 try/catch 合并，parse 走 typeof raw===string 先判断避免双重 JSON.parse
- `group-chat/GroupChatPanel.vue::formatTokens` — 把 token 数格式化成「1.2k tokens」或「900 tokens」
- `group-chat/GroupChatPanel.vue::generateCode` — 生成 6 位大写邀请码（排除混淆字母 I/O/1/0），用于 Room 邀请码自动生成和刷新
- `group-chat/GroupChatPanel.vue::formatAgentFailures` — 把 agentResults 数组里失败项的 reason/error/profile 合并成一条 warning 文案，全成功返回 null
- `group-chat/GroupChatPanel.vue::extractApiErrorMessage` — 从 API 错误的 message 字符串里截取 JSON 部分解析，针对 PROFILE_AGENT_CONNECT_FAILED code 拼接 error:reason，否则回退到原始 message
- `group-chat/GroupChatPanel.vue::handleCreateRoom` — 调用 store.createNewRoom，成功后 push 路由到新 room，同时处理 agent 部分失败（warning 非 error）
- `group-chat/GroupChatPanel.vue::handleDeleteRoom` — 删房间后若当前在该房间则 replace 到 groupChat 根路由
- `group-chat/GroupChatPanel.vue::buildRoomUrl` — 用 router.resolve 拼出带 origin+pathname 的完整房间分享链接
- `group-chat/GroupChatPanel.vue::handleRoomContextMenu` — 右键菜单定位：记录 event.clientX/Y 并 show NDropdown，contextRoomId 暂存目标房间
- `group-chat/GroupChatPanel.vue::handleOpenCloneRoom` — 打开克隆 modal 时预填 name+'Copy' 和随机邀请码
- `group-chat/GroupChatPanel.vue::handleClearRoomContext` — 若 contextStatuses 非空（有压缩进行中）阻止清空并报 warning，否则调用 store.clearCurrentRoomContext
- `group-chat/GroupChatPanel.vue::handleOpenCompressionConfig` — 打开压缩配置 modal 时从当前 room 对象读取三参数：triggerTokens/maxHistoryTokens/tailMessageCount，带 nullish coalescing 默认值
- `group-chat/GroupChatPanel.vue::handleForceCompress` — 立即触发压缩，检查 contextStatuses 避免并发，isCompressing 防重复提交
- `group-chat/GroupChatPanel.vue::handleApproval` — 转发 once/session/always/deny 四档权限审批响应给 store.respondApproval
- `group-chat/GroupChatInput.vue::startResize` — 自定义 textarea 高度拖拽：mousedown 起点记录，mousemove 算 deltaY 并 clamp 到 [20,400]px，mouseup 时移除 listener，同时锁定 body cursor/userSelect
- `group-chat/GroupChatInput.vue::updateMentionState` — 在 textarea 当前光标向左扫描，找 @ 符号并提取查询词，通过 DOM mirror span 计算下拉框 X 坐标，根据视口剩余空间决定 placement top/bottom
- `group-chat/GroupChatInput.vue::selectMention` — 选中 mention 后把 @name + 空格插入文本正确位置，更新光标，若未自定义高度则触发 auto-resize
- `group-chat/GroupChatInput.vue::handleKeydown` — mention 激活时接管 ArrowUp/Down/Enter/Tab/Escape；不在 mention 时 Enter（非 Shift）触发发送，兼容 IME composing（e.isComposing + keyCode===229）
- `group-chat/GroupChatInput.vue::handleInput` — textarea auto-resize（不覆盖拖拽高度），调用 store.emitTyping，非 composing 时调 updateMentionState
- `group-chat/GroupChatInput.vue::handlePaste` — 拦截剪贴板图片粘贴（clipboardData.items filter image/*），构造 File 对象添加到 attachments，阻止默认行为
- `group-chat/GroupChatInput.vue::handleDragEnter/Leave/Drop` — 用 dragCounter 计数（避免子元素触发 dragleave 误判），Files 类型才激活 isDragging，drop 后收集文件
- `group-chat/GroupChatInput.vue::addFile` — 按 name 去重，生成唯一 id（Date.now().toString(36)+random），创建 objectURL 追加到 attachments
- `group-chat/GroupChatInput.vue::removeAttachment` — 移除附件同时 revokeObjectURL 防内存泄漏
- `group-chat/GroupMessageList.vue::scrollToBottom` — 暴露给父组件，代理调用 VirtualMessageList 的 scrollToBottom，支持 frames+keepAliveMs 选项
- `group-chat/GroupMessageList.vue::handleTopReach` — 虚拟列表滚到顶端时：先 captureScrollPosition，await loadOlderMessages，再 restoreScrollPosition 保持视觉位置不跳
- `group-chat/GroupMessageList.vue (watch displayMessages)` — 监听消息内容变化（复合 key 字符串），自动判断是否滚底：初次进 room 强制 frames:5/keepAlive:700，已在底部则 frames:1/keepAlive:120
  - 💡 用 map+join 把多字段拍平成一个 string 做 watcher key，避免深度监听
- `group-chat/GroupMessageItem.vue::truncateJsonValue` — 递归截断 JSON 结构用于工具调用展示：控制最大深度/节点数/每对象键数/每数组项数/字符串长度，WeakSet 检测循环引用，超限时注入 __truncated__ 占位
  - 💡 stringifyLength 内联用于实时判断截断阈值，不预先序列化整个对象
- `group-chat/GroupMessageItem.vue::formatToolPayload` — 工具调用 payload 格式化：JSON 解析→抽取 unified diff→截断显示版，返回 {full, display, language}，保留 full 用于全量复制
- `group-chat/GroupMessageItem.vue::renderToolPayload` — 调用 renderHighlightedCodeBlock 把 tool payload 渲染成带高亮和复制按钮的 HTML 字符串
- `group-chat/GroupMessageItem.vue::handleToolDetailClick` — 事件委托：点击 tool detail 区域时检测 data-copy-code 属性，根据 data-copy-source 区分复制 full tool-args 还是 full tool-result
- `group-chat/GroupMessageItem.vue::playSpeech` — 多 provider TTS 分发：openai/custom/edge/mimo/webspeech，autoplay 参数控制 play vs toggle，edge 走后端代理 /api/tts/proxy
- `group-chat/GroupMessageItem.vue::handleSpeechToggle` — 点击播放/暂停按钮时调用 playSpeech(assistantBody)
- `group-chat/GroupMessageItem.vue::normalizeLocalFilePath` — Windows 绝对路径（C:\）替换反斜杠为正斜杠，用于附件 URL 构造
- `group-chat/GroupMessageItem.vue (onMounted auto-play handler)` — 监听 window 级 auto-play-speech 自定义事件，匹配 messageId 后自动播放，onBeforeUnmount 移除 listener
- `group-chat/CreateRoomForm.vue::handleCreate` — 创建房间表单提交：inviteCode 为空时自动 generateCode，emit submit 携带 name/code/user/description/compression 五参数
- `group-chat/mention-options.ts::isReservedMentionName` — 判断 agent 名称是否与保留关键词 'all' 冲突（大小写不敏感），防止 @all 被普通 agent 占用
- `group-chat/mention-options.ts::buildMentionOptions` — 构建 mention 下拉选项：先加 @all（query 为空或匹配时），再遍历 agents 过滤 name 包含 query，跳过保留名，返回 MentionOption[]
- `jobs/JobCard.vue::statusLabel / statusType (computed)` — 把 job.state(running/paused) + job.enabled 映射到 i18n 文案和 naïve-ui 状态类型(info/warning/error/success)
- `jobs/JobCard.vue::handlePause / handleResume / handleRun / handleDelete` — 四个操作各自调 jobsStore 对应方法，统一 try/catch + message 反馈
- `jobs/JobCard.vue::handleCardClick` — 点击卡片 body 选中 job，点击 .card-actions 区域不触发 select（用 closest 判断）
- `jobs/JobFormModal.vue::isDeliverTargetConfigured` — 按 platform key 检测该渠道是否已配置（token/enabled/extra 字段各不同），决定 targetOptions 中该选项是否 disabled
- `jobs/JobFormModal.vue::handleSave` — 创建/编辑双模：edit 时调 buildJobUpdateRequest 只 diff 变化字段，无变化则提前返回成功；create 时构造 CreateJobRequest，统一 loading 控制
- `jobs/JobFormModal.vue::handleClose` — 先设 showModal=false 触发 modal 退出动画，setTimeout 200ms 后 emit close，避免内容瞬间消失
- `jobs/JobRunHistory.vue::fetchRuns` — 调 listCronRuns 拉取运行历史，selectedJobId 为 null 则拉全量
- `jobs/JobRunHistory.vue::handleExpand` — accordion 展开时懒加载运行详情：key = jobId/fileName，检查 expandedContent/loadingContent 去重，调 readCronRun 并渲染 MarkdownRenderer
- `jobs/JobsPanel.vue::handleSelect` — 实现 toggle select：再次点击已选中的 job 则 emit null 取消选中
- `kanban/KanbanColumn.vue::statusIcon (computed)` — 把 status 枚举映射到 Unicode 符号：todo→○, ready→◎, running→●, blocked→⊘, done→✓
- `kanban/KanbanColumn.vue::headerTitle (computed)` — 拼接 icon+label+(count) 作为 NCollapseItem 标题，列可折叠
- `kanban/KanbanTaskCard.vue::timeAgo (computed)` — 计算 created_at 到 now 的相对时间，四档：刚刚/N分钟/N小时/N天，全 i18n
- `kanban/KanbanTaskCard.vue::priorityLabel (computed)` — priority 数值 1/2/3+ 映射到 low/medium/high，用于 CSS class 和文案
- `kanban/KanbanCreateForm.vue::handleSubmit` — 调 kanbanStore.createTask，emit created+close，saving 状态防重复提交
- `kanban/KanbanCreateForm.vue::assigneeOptions (computed)` — 调 withDefaultAssignee 工具函数，结合 kanbanStore.assignees 和 stats.by_assignee 排序/补默认
- `kanban/KanbanTaskDrawer.vue::searchTaskSessions` — toggle 展示相关 chat session：按 task_id+profile+board 调后端 /kanban/search-sessions，懒加载
- `kanban/KanbanTaskDrawer.vue::historySession (computed)` — 把 detail.session 的 messages 过滤 user/assistant，映射为标准 Session 格式供 HistoryMessageList 渲染
- `kanban/KanbanTaskDrawer.vue::handleComplete` — 两步式完成：第一次点击展开 summary 输入框，第二次确认才调 completeTasks，summary 可选
- `kanban/KanbanTaskDrawer.vue::handleBlock / handleUnblock` — block 需填 blockReason 才能提交，unblock 直接调；均 emit updated+close
- `kanban/KanbanTaskDrawer.vue::handleAssign` — 选 profile 后调 kanbanStore.assignTask，成功后重新 getTask 刷新 detail（不重刷全看板），emit updated
- `kanban/KanbanTaskDrawer.vue::handleNavigateTask` — 抽屉内点击 parent/child task 链接时 emit navigate(taskId)，让父层切换 taskId
- `kanban/KanbanTaskDrawer.vue (watch taskId+selectedBoard)` — 双维度 watch：taskId 或 board 变化都重新 getTask，用 id/board 快照比对防止 race condition 写入过期数据

### models + profiles + mcp + skills 组件  ·  45 项

- `models/ProviderCard.vue:isCustom` — computed: 判断 provider 是否为 custom:前缀，驱动卡片 badge 和删除路径分叉
  - 💡 custom 和 builtin 共用同一卡片结构，仅靠 provider key 前缀区分
- `models/ProviderCard.vue:modelDisplayName` — alias 优先，fallback 到原始 model ID，统一模型显示名
  - 💡 alias 存在时显示别名 + 在下方用小号字体显示原始 ID，人工友好
- `models/ProviderCard.vue:openAliasEditor` — 在卡片内直接弹 alias 编辑 modal，无需跳页
  - 💡 模型名别名功能独立于 provider 配置，是 appStore 的 per-model 本地映射
- `models/ProviderCard.vue:handleVisibilitySave` — 把勾选的 visible models 存为 include 规则；若全选则重置为 all 模式
  - 💡 all vs include 两种模式，避免每次全选后写入巨大列表；清空不允许（至少选一个）
- `models/ProviderCard.vue:handleDelete` — 删除 provider；Copilot 特殊路径：先 checkCopilotToken 查 token 来源，再提示影响范围，再 disableCopilot
  - 💡 Copilot 不真正删账号，只是 disable；且根据 token source 给出不同警告文案（env/gh-cli/apps-json）
- `models/ProviderFormModal.vue:switchToApiKeyFunPreset` — 当用户在 custom 表单里填了 apikey.fun 的 base_url 并选了模型，自动切换到对应 preset provider（fun-codex/fun-claude）
  - 💡 智能路由：custom form → preset，对用户透明，避免 custom:fun-codex 和 fun-codex 两份配置
- `models/ProviderFormModal.vue:routeApiKeyFunCustomProvider` — watch model 变化时触发，检测 base_url 是否为 apikey.fun 并推断 preset，驱动 switchToApiKeyFunPreset
  - 💡 在用户填完 base_url + 选定模型时自动完成 preset 路由，无需用户手动重选
- `models/ProviderFormModal.vue:autoGenerateName` — 从 base_url 提取 hostname 自动生成 provider 名，localhost 特判
  - 💡 去掉 https:// 前缀和 /v1/ 后缀，只留 hostname 首字母大写，减少用户输入摩擦
- `models/ProviderFormModal.vue:triggerCopilotAdd` — 添加 Copilot 时先探测已有 token（checkCopilotToken），有则弹确认 dialog；无则进 device flow（showCopilotLogin）
  - 💡 渐进式 OAuth：检测已有 token 优先，避免已有 gh CLI 登录的用户重走 device flow
- `models/ProviderFormModal.vue:fetchModels` — 调用 fetchProviderModels API 动态拉取该 base_url 支持的 model 列表，填充 Select 下拉
  - 💡 update_cache 仅在 provider key 已知时传 true，避免污染未知 provider 缓存
- `models/AuxiliaryModelsPanel.vue:taskLabel` — 将内部 task key（compression/vision/web_extract/skills_hub 等）映射为 i18n 显示名
  - 💡 穷举 12 种辅助任务 key，覆盖 kanban_decomposer/profile_describer/curator 等高级任务
- `models/AuxiliaryModelsPanel.vue:buildSettings` — 从表单构建 AuxiliaryModelSettings，provider=auto 时不写 model；provider 有值时要求 model 不空
  - 💡 extra_body 以 JSON textarea 输入，需 parse + 类型校验（必须是 object 非 array）
- `models/AuxiliaryModelsPanel.vue:clearTask` — 将指定 task 的配置重置为 {provider: auto, default_timeout}
  - 💡 重置不是删除 key，而是写入 auto 配置，保持结构完整
- `models/CodexLoginModal.vue:startPolling` — 3s 间隔轮询 pollCodexLogin，处理 pending/approved/expired/error 四态
  - 💡 pending 时递归调自身（而非 setInterval）；catch 时静默继续轮询，不暴露网络抖动
- `models/CodexLoginModal.vue:startLogin` — 启动 Codex device flow：调 startCodexLogin 获取 user_code + verification_url + session_id
  - 💡 modal 挂载时自动触发（无需用户点按钮），降低步骤感
- `models/CopilotLoginModal.vue:startPolling` — 与 CodexLoginModal 同结构；多处理 denied 状态（Copilot 有拒绝流）
  - 💡 Copilot 比 Codex 多一个 denied 态，映射为 error 态+提示
- `models/XaiOAuthLoginModal.vue:startLogin` — xAI OAuth 不用 user_code，直接弹新窗口到 authorization_url，无 device code 步骤
  - 💡 xAI 走标准 OAuth redirect，与 Codex/Copilot/Nous 的 device flow 不同；modal 提供「复制链接」兜底
- `models/ProvidersPanel.vue` — grid 布局渲染所有 ProviderCard；空态显示 SVG + 提示文案
  - 💡 auto-fill minmax(420px,1fr) 自适应列数，不硬编码列数
- `profiles/ProfileAvatar.vue:generatedSvg` — 用 multiavatar 库根据 seed（或 profile name fallback）生成确定性 SVG 头像
  - 💡 type=image 时展示 dataUrl 图片，否则 v-html 注入 SVG；两种类型共享同一组件
- `profiles/ProfileCard.vue:toggleDetail` — 展开时懒加载 profile 详情（path/provider/skills/hasEnv/hasSoulMd），折叠时不重新拉取
  - 💡 按需加载 detail，初始只显示 name+model，展开才请求
- `profiles/ProfileCard.vue:performHermesSwitch` — 切换 active profile 后 window.location.reload()，刷新所有 profile 依赖数据
  - 💡 选择全页刷新而非局部 store 更新，简单彻底，避免遗漏依赖
- `profiles/ProfileCard.vue:handleExport` — 触发 exportProfile，后端返回 tar.gz blob，前端创建临时 <a> 下载
  - 💡 导出文件名固定为 hermes-profile-{name}.tar.gz
- `profiles/ProfileCreateModal.vue:handleNameInput` — 输入时实时过滤非法字符（只保留 a-z0-9_-），同时显示校验提示
  - 💡 不是 block 而是 transform：过滤后写回，让用户感受到限制但不阻断输入流
- `profiles/ProfileCreateModal.vue:handleSave` — 创建 profile（可选 clone=true 从当前克隆）；clone 时解析并展示被清理的凭据/平台列表
  - 💡 clone 时 strippedCredentials/disabledPlatforms 要单独拼接提示，duration 6000ms 留阅读时间
- `profiles/ProfileImportModal.vue:beforeUpload` — 文件选择前校验扩展名（.tar.gz/.tgz/.gz/.zip），不合法则拦截并提示
  - 💡 扩展名 endsWith 匹配，兼容 .tar.gz 这种双扩展
- `profiles/ProfileImportModal.vue:handleSave` — 读取 fileList[0].file，调 importProfile（FormData POST 上传），成功后 emit saved
  - 💡 profilesStore.importProfile 内部走原生 fetch（非封装 request），以便处理 multipart
- `profiles/ProfilesStore.ts:fetchProfiles` — 拉取 profile 列表，与 localStorage 中的 activeProfileName 对齐，activeProfile 持久化
  - 💡 初始化时从 localStorage 同步 activeProfileName，确保其他 store 在启动时已有 profile 上下文
- `profiles/ProfilesStore.ts:updateAvatar` — 更新头像后同步更新 profiles/detailMap/activeProfile 三处引用
  - 💡 三处同步避免缓存不一致，但要注意原子性——如果 API 失败中途会出现不一致
- `profiles/ProfilesStore.ts:clearAllSessionCaches` — 预留的会话缓存清理钩子，当前实现为空（已去掉 localStorage 会话缓存）
  - 💡 空实现是有意为之：注释说明已全部改为服务端实时拉取，不再需要清缓存
- `mcp/McpServerCard.vue:statusClass` — 根据 enabled 和 connected 两个维度返回 CSS class（disabled/connected/disconnected）
  - 💡 disabled 优先级高于 connected，避免「禁用但显示已连接」的歧义
- `mcp/McpServerCard.vue (emit pattern)` — 卡片只 emit 事件（edit/test/reload/remove/toggleEnabled/manageTools），不直接调 API
  - 💡 纯展示组件模式，所有副作用由父级处理，便于测试和复用
- `mcp/mcp.ts:fetchMcpServers` — GET /api/hermes/mcp/servers 返回 McpServerInfo[]，含 tool_details（name+description）
  - 💡 tool_details 与 tool_names 分离，tool_details 含描述，tool_names 只含名称（节省带宽选项）
- `mcp/mcp.ts:mcpServerTest` — POST /api/hermes/mcp/servers/{name}/test，返回 tools[] 或 error，用于连接探测
  - 💡 独立的 test 端点，不影响 server 状态，用于 UI 「测试」按钮
- `mcp/mcp.ts:mcpReload` — POST /api/hermes/mcp/reload?server=xxx，可选 server 参数，单 server 或全量重载
  - 💡 不传 name 则全量重载；设计上允许局部热更新 MCP 配置
- `skills/SkillList.vue:filteredCategories` — 双维过滤：sourceFilter（builtin/hub/local/external/modified）+ searchQuery（名称/描述全文）
  - 💡 modified 是虚拟 source，匹配 skill.modified=true；search 时保留 category name 也匹配的分组
- `skills/SkillList.vue:handleToggle` — 就地 toggle skill enabled，乐观更新本地 skill.enabled，失败时 message.error 但不回滚
  - 💡 用 togglingSkills Set 做 per-skill loading 状态，避免多个 skill 同时 toggle 时状态串扰
- `skills/SkillDetail.vue:loadSkill` — 并发加载 SKILL.md 内容和附属文件列表（Promise.all），过滤掉 isDir 和 SKILL.md 本身
  - 💡 fetch 前清空所有状态，避免切换 skill 时短暂显示上一个 skill 的内容
- `skills/SkillDetail.vue:viewFile` — 处理绝对路径和相对路径的 normalization：从绝对路径中提取 skills/ 后的相对部分
  - 💡 兼容 Windows 路径（反斜杠）和 Unix 路径；从 .hermes/skills/ 或 hermes/skills/ 切割
- `skills/SkillDetail.vue:handlePinToggle` — pin/unpin skill，emit pinToggled 给父级更新状态，而非直接改 prop
  - 💡 pinLoading 防止重复点击；pin 状态不在本组件维护，只 emit 让父级决定如何更新
- `skills/skills.ts:fetchSkillUsageStats` — GET /api/hermes/skills/usage/stats?days=N，返回按天+按 skill 的使用统计
  - 💡 统计粒度：总次数/view 次数/edit 次数/最后使用时间/distinct skills
- `skills/skills.ts:toggleSkill` — PUT /api/hermes/skills/toggle {name, enabled} 启用/禁用 skill
  - 💡 toggle 和 pin 是两个独立端点（/toggle vs /pin），enabled 是运行时状态，pinned 是 UI 置顶状态
- `models/models.ts (store):fetchProviders` — 拉取当前 active profile 的 provider 列表（包含 allProviders 完整目录），分离 providers（已配置）和 allProviders（预设目录）
  - 💡 providers = 用户已添加的；allProviders = 包含未添加预设的完整目录，用于 ProviderFormModal 下拉
- `models/models.ts (store):refreshModelCache` — 强制刷新服务端 model 缓存，再 fetchProviders，再 reloadModels
  - 💡 三步级联刷新确保一致性：服务端缓存→store→appStore
- `profiles/profiles.ts:exportProfile` — POST /api/hermes/profiles/{name}/export，拿到 blob 后构造临时 <a> 触发下载，URL.revokeObjectURL 清理
  - 💡 在 API 层处理文件下载，不经过 request() 封装（原生 fetch），因为需要 blob 响应
- `profiles/profiles.ts:createProfile` — POST body 含 clone 参数，返回 strippedCredentials/disabledPlatforms/strippedConfigCredentials 给前端展示清理详情
  - 💡 后端在 clone 时自动清理独占凭据，并把清理清单返回给前端；前端负责告知用户

### settings 全部面板(15)  ·  43 项

- `AccountSettings.vue:compressImage` — 客户端纯 Canvas 对上传图片做迭代质量压缩：quality 从 0.8 递减 0.1，直到 base64 长度 ≤ maxBytes 或 quality≤0.3 为止，返回 data-URL
  - 💡 不调服务端，完全浏览器侧；质量下限硬编码 0.3 防过度损失
- `AccountSettings.vue:handleAvatarUpload` — 文件格式校验(png/jpeg/webp) + 大小 1MB 限制 + 超 500KB 时调 compressImage → POST updateMyAvatar
  - 💡 先用原生 FileReader 读取，再按阈值决定是否压缩，两路共享同一 POST 路径
- `AccountSettings.vue:handleRandomAvatar` — 用 crypto.randomUUID + username + Date.now 生成种子，调 multiavatar 库生成 SVG，转 base64 data-URL 后上传
  - 💡 依赖 @multiavatar/multiavatar 纯前端 SVG 生成，seed 含时间戳保证每次不同
- `AccountSettings.vue:handleResetAvatar` — 调 resetMyAvatar API，本地恢复 {type:'default', seed: username}
- `AccountSettings.vue:handleChangePassword` — 二次密码一致性校验 + 最短 6 位校验 → changePassword API，成功后清空表单并关闭 Modal
- `AccountSettings.vue:handleChangeUsername` — 用户名 ≥2 位校验 → changeUsername API，需要输入当前密码做二次鉴权
- `AccountSettings.vue:loadLockedIps` — 加载被锁定的 IP 列表（登录失败次数超限锁定），显示 IP、锁定类型、剩余解锁时间
- `AccountSettings.vue:handleUnlockIp / handleUnlockAll` — 单条/全量解锁被锁 IP，全量带 NPopconfirm 二次确认
- `AccountSettings.vue:formatTime` — 把时间戳换算成剩余分钟数字符串，过期返回 i18n 'expired'
- `AgentSettings.vue:debouncedSave` — 每个字段独立 debounceTimers[key]，300ms 防抖后发 HTTP；先立即 updateLocal 保证 UI 不等待
  - 💡 两阶段写入：立即写本地 store + 延迟发 HTTP，是整个设置系统的通用范式，在 Agent/Memory/Compression/Session 面板反复出现
- `AgentSettings.vue:save` — NSelect 等单次操作直接 saveSection，不防抖
- `CompressionSettings.vue:(setup)` — context 压缩策略参数面板：enabled/threshold/target_ratio/protect_last_n/protect_first_n 五项，含 defaults 兜底
- `DisplaySettings.vue:handleThemeChange` — 主题切换同时调 setBrightness(composable 即时改 DOM class) + saveSection 持久化
  - 💡 即时生效 + 异步持久化解耦，避免等待网络
- `GithubPreviewSettings.vue:runAction` — 统一包装异步操作：设 actionLoading、调 fn、处理 success=false 的软错误和 throw 的硬错误、错误时 applyErrorStatus 把 JSON body 回填到 status 展示
  - 💡 applyErrorStatus 从异常 message 字符串里找 '{' 再 JSON.parse，用于服务端错误时附带详情
- `GithubPreviewSettings.vue:startPolling / stopPolling` — watch active_action：有动作时每 2s 轮询状态，动作完成后停止，防止无操作时持续轮询
- `GithubPreviewSettings.vue:watch(last_action_completed_at)` — 通过 completionNotificationsReady 标志避免组件初始化时触发虚假完成通知；每次完成 show success/error toast
  - 💡 completionNotificationsReady 布尔门控防止 onMounted 拿到旧 completedAt 误报
- `GithubPreviewSettings.vue:applyErrorStatus / parseErrorPayload / errorCodeMessage` — 从异常消息体提取结构化错误 payload，做 error code → i18n key 映射，fallback 到原始消息
- `MemorySettings.vue:(setup)` — 记忆功能控制：memory_enabled/user_profile_enabled 两个开关 + memory_char_limit/user_char_limit 两个字数上限输入
- `ModelSettings.vue:isCustom` — 判断 provider 是否为自定义：!builtin && key 以 'custom:' 开头
- `ModelSettings.vue:getEditKey` — lazy 初始化 editKeys[provider]，首次访问时从 store 读取已保存的 api_key
- `ModelSettings.vue:handleSaveApiKey / handleSaveCustom` — 内置/自定义 provider 分别走同一 updateProvider API，区别是自定义不强制 key 非空
- `PlatformSettings.vue:configDraft / credentialDraft` — Lazy clone 当前 store 值作为本地草稿，编辑时写草稿不写 store，直到显式 Save 才提交
- `PlatformSettings.vue:mergeDeep` — 递归深合并对象，支持 credential.extra.* 嵌套字段的局部更新
- `PlatformSettings.vue:hasConfigChanges / hasCredentialChanges / hasUnsavedChanges` — JSON.stringify 对比草稿与 store 快照，决定 Save 按钮 disabled 状态
- `PlatformSettings.vue:savePlatform` — 配置与凭据分两个 API 提交；只变了其中一方时跳过另一方调用；配置保存时 restart 参数由是否同时变凭据决定（双改时由凭据保存触发 restart，避免重启两次）
- `PlatformSettings.vue:startWeixinQrLogin / pollWeixinStatus / stopWeixinPoll` — 微信扫码登录状态机：idle→loading→waiting→scaned→confirmed/expired/error，3s 轮询 pollWeixinQrStatus，confirmed 时保存 token+account_id
- `PlatformSettings.vue:watch(platforms)` — deep immediate watch 平台列表，未 touch 的平台 draft 自动跟 store 同步，touched 的不覆盖
- `PlatformCard.vue:configured` — computed：检查 credentials 顶层和 extra.* 层任意一个「有意义 key」非空即为 configured，驱动绿色 border 和状态 Tag
  - 💡 key 列表固定枚举 token/api_key/app_id/client_id/secret/app_secret/client_secret/access_token/bot_id/account_id/enabled，覆盖所有平台
- `PlatformCard.vue:(template)` — 可折叠平台卡片：header 点击 toggle expanded；exclusive 平台显示 NAlert 互斥警告；slot 插槽放平台特定字段
- `PrivacySettings.vue:(setup)` — 最简单面板：单个 redact_pii 开关，直接保存无防抖
- `SessionSettings.vue:toggleRequireAuth` — 把 approvals.mode 在 'manual'/'off' 之间切换，注意这是 approvals section 不是 session_reset section
- `SessionSettings.vue:(setup)` — 会话重置策略：requireAuth 开关 + mode select(both/idle/daily/none) + idle_minutes + at_hour + liveMonitorHumanOnly(纯本地 browser prefs)
- `SettingRow.vue:(template)` — 原子布局组件：label+hint 左侧 flex:1，control slot 右侧 flex-shrink:0；响应式 mobile 时堆叠为竖向
  - 💡 全系统 SettingRow 统一行间距(padding 10px 0 + border-bottom)，只需传 label/hint props
- `UserManagementSettings.vue:columns` — 用 computed + NaiveUI DataTable columns DSL 定义含 render h() 函数的列：role Tag(warning/default) + status Tag(success/error) + profiles Tag 列表 + 操作列(编辑/禁用/删除)
- `UserManagementSettings.vue:submit` — create/edit 共用一个 submit：根据 editingUser 判断调 createManagedUser 或 updateManagedUser；super_admin 强制 profiles=[]
- `UserManagementSettings.vue:setStatus / removeUser` — 行内快速切换 active/disabled + 带 NPopconfirm 二次确认的删除
- `VoiceSettings.vue:handleTest` — 按当前 provider 分支调用不同播放路径：webspeech→browser API / openai→openaiPlay / custom→openaiPlay(不同 baseUrl) / edge→proxy endpoint / mimo→mimoPlay
  - 💡 5 个 provider 全用同一 handleTest 函数通过分支路由，让测试 UX 统一
- `useVoiceSettings.ts:(module-level)` — 模块级单例 ref 而非 store：所有 ref 在 import 时即初始化，useVoiceSettings() 每次返回同一批 ref 引用；watch 数组监听全部字段，变化时 localStorage.setItem
  - 💡 故意不用 Pinia 因为语音设置是纯 client-local，不需要服务器持久化
- `useVoiceSettings.ts:migrateOldKeys` — 从旧存储 key 迁移到新格式：provider 'gptsovits' 改名为 'custom'，gptsovitsUrl 复制到 customUrl，删除旧 key
- `useVoiceSettings.ts:sanitize` — 加载时清除旧 edgeUrl（现在改用内部 node-edge-tts，旧适配器 URL 无效）
- `stores/hermes/settings.ts:useSettingsStore` — Pinia store，每个 section 独立 ref；fetchSettings 拉全量 config；updateLocal 做乐观更新；saveSection 先调 API 再按 switch-case 更新对应 ref
- `stores/hermes/settings.ts:updateLocal` — switch-case 按 section 名称分发 shallow merge，是 debouncedSave 的乐观写入入口
- `stores/hermes/settings.ts:saveSection` — 调 configApi.updateConfigSection(section, values, options)，options.restart 控制后端是否重启服务

### files + usage + layout + auth + common 组件  ·  52 项

- `stores/hermes/files.ts:useFilesStore` — Pinia store，管理文件浏览器全部状态：currentPath/entries/loading/sortBy/sortOrder/editingFile/previewFile，暴露 navigateTo/navigateUp/openEditor/saveEditor/closeEditor/openPreview/closePreview/createDir/createFile/deleteEntry/renameEntry/copyEntry/uploadFiles/setSort
  - 💡 editingFile 同时保存 content 与 originalContent，hasUnsavedChanges 用 computed 对比二者，关闭前触发弹窗确认；deleteEntry/renameEntry 同步用 isAffected 检查 previewFile/editingFile 是否受影响，避免编辑器持有失效句柄
- `stores/hermes/files.ts:isAffected` — 判断 targetPath 是否等于 changedPath，或者 changedIsDir=true 时属于其子路径，用于删除/重命名后失效 previewFile/editingFile
- `stores/hermes/files.ts:getLanguageFromPath` — 通过扩展名和特殊文件名（Dockerfile/Makefile/.gitignore 等）映射 Monaco 语言 ID，fallback 为 plaintext
  - 💡 SPECIAL_FILE_LANG_MAP 覆盖无扩展名文件；EXT_LANG_MAP 映射常见语言
- `stores/hermes/files.ts:isTextFile` — 纯函数：TEXT_BASENAMES Set（无扩展名文件）+ TEXT_EXTS Set（O(1) 查询）+ .env.* 前缀判断，确定文件是否可文本编辑
- `stores/hermes/files.ts:isImageFile / isMarkdownFile / isPreviewableFile` — 按扩展名 Set 判断图片/Markdown；isPreviewableFile = isImageFile \|\| isMarkdownFile \|\| isTextFile，组合不重叠
- `api/hermes/files.ts:listFiles / readFile / writeFile / deleteFile / renameFile / mkDir / copyFile / uploadFiles / getFileDownloadUrl` — 文件系统 REST API 层，共 9 个函数。uploadFiles 单独用 fetch+FormData（绕过 request 封装）支持 multipart；getFileDownloadUrl 把 token 放 query param（<a> 标签无法注入 Header）
  - 💡 uploadFiles 通过 Headers 注入 Authorization 和 X-Hermes-Profile；getFileDownloadUrl 亦追加 profile 和 token 到 URL query
- `api/hermes/download.ts:getDownloadUrl` — 构造下载 URL，带防双包装保护（检测 filePath 是否已经是 /api/hermes/download?path= 开头，先解包再重构），先 decodeURIComponent 再交给 URLSearchParams 编码，防双重编码
- `api/hermes/download.ts:downloadFile` — fetch→blob→createObjectURL→<a>.click()→revokeObjectURL，完整前端下载流，错误时解析 JSON body
- `api/hermes/download.ts:fetchFileText` — 预览文件内容，返回 res.text()；与 downloadFile 共用 getDownloadUrl
- `components/hermes/files/FileTree.vue:loadChildren` — 异步懒加载：对指定路径调用 listFiles，只返回 isDir=true 的条目并按名排序，作为 NTree 的 onLoad 回调参数
- `components/hermes/files/FileTree.vue:handleLoad / handleSelect / handleRootClick` — NTree 三个事件处理：handleLoad 注入子节点；handleSelect 同步 filesStore.navigateTo；handleRootClick 清空 selectedKeys 并导航到根
- `components/hermes/files/FileBreadcrumb.vue:handleClick` — 面包屑点击：index=-1 导航到根，否则截取 pathSegments[0..index] join('/') 导航到对应层级
- `components/hermes/files/FileContextMenu.vue:getOptions` — 按文件类型动态生成右键菜单项：目录只有 open；文件按 isTextFile/isPreviewableFile 条件加 edit/preview；通用 download/copyPath/rename/delete 加分隔线
- `components/hermes/files/FileContextMenu.vue:show` — 在 nextTick 中翻转 showMenu=false→true 触发 NDropdown 重定位（防止旧坐标残留），记录鼠标坐标和目标条目；通过 defineExpose 暴露给父组件
- `components/hermes/files/FileContextMenu.vue:handleSelect` — 统一 switch 处理所有菜单动作：copyPath 调 getClipboardPathForEntry；delete 弹 dialog.warning 二次确认；rename 转发 emit
- `components/hermes/files/FileEditor.vue:onMounted Monaco 初始化` — 用 import.meta.url 构造 Monaco worker URL（ESM 方式）、创建编辑器实例、绑定 onDidChangeModelContent 同步 store、注册 Ctrl/Cmd+S 快捷键触发保存
  - 💡 theme 跟随 document.documentElement.classList.contains('dark')；minimap 关闭；automaticLayout=true 响应容器尺寸变化
- `components/hermes/files/FileEditor.vue:handleClose` — 关闭时检查 hasUnsavedChanges，有未保存改动则弹 dialog.warning 二次确认后再 closeEditor
- `components/hermes/files/FilePreview.vue:highlightedPreview` — computed：对 text 类型调用 renderHighlightedCodeBlock 生成带语法高亮+复制按钮的 HTML，maxHighlightLength=200000 防止大文件卡住主线程；image/markdown 走各自分支
- `components/hermes/files/FileList.vue:formatSize / formatDate / getFileIcon` — formatSize 以 1024 为基数四级换算（B/KB/MB/GB）；getFileIcon 按扩展名返回 emoji 图标；formatDate 调 toLocaleString
- `components/hermes/files/FileList.vue:handleDoubleClick` — 双击分发：目录→navigateTo；文本文件→openEditor；可预览文件→openPreview
- `components/hermes/files/FileList.vue:setSort（通过 filesStore）` — 点击列头切换排序：同列名二次点击反转 asc/desc，不同列重置为 asc；sortedEntries computed 目录优先再按字段排序
- `components/hermes/files/FileRenameModal.vue:handleSubmit` — 三合一模态：mode=newFile/newFolder/rename 共用同一 modal+input，统一走 store 的对应 action，Enter 键提交，watch show 变化时重置或填充 inputValue
- `components/hermes/files/FileUploadModal.vue:handleUpload` — 收集 NUpload fileList（directory-dnd 支持目录拖放），批量传给 filesStore.uploadFiles，完成后清空 fileList 并关闭模态
- `stores/hermes/usage.ts:useUsageStore` — Pinia store：loadSessions 用 requestId 做竞态保护；computed 导出 totalInputTokens/totalOutputTokens/totalTokens/totalSessions/totalCacheTokens/cacheHitRate/estimatedCost/modelUsage/dailyUsage/avgSessionsPerDay
- `stores/hermes/usage.ts:getModelColor` — 字符串哈希映射到 10 色调色板，同一 model 名始终得到同一颜色，跨图表颜色一致
- `stores/hermes/usage.ts:modelUsage (computed)` — 从 API 原始数据派生 visualTokens(=input+output+cacheRead)、inputPercent/outputPercent/cachePercent，按 visualTokens 降序排列
- `stores/hermes/usage.ts:dailyUsage (computed)` — 在每天的原始数据上追加 visualTokens 和三份百分比，供堆叠柱状图按高度比例渲染
- `components/hermes/usage/StatCards.vue:formatTokens / formatCost` — formatTokens：>=1M→'xM'，>=1K→'xK'，否则纯数字；formatCost：0→'$0.00'，<0.01→'<$0.01'，否则两位小数
- `components/hermes/usage/DailyTrend.vue:cacheHitRate (local function)` — 局部函数：(cacheRead/(input+cacheRead))*100，用于 tooltip 和表格行；与 store 里全局 cacheHitRate 独立计算
- `components/hermes/usage/DailyTrend.vue:maxTokens (computed)` — 取所有天 visualTokens 的最大值，把柱子高度归一化到 140px CSS height 百分比
- `components/hermes/usage/ModelBreakdown.vue:maxModelTokens (computed)` — 取所有 model 的 visualTokens 最大值，把水平进度条宽度归一化
- `components/layout/AppSidebar.vue:toggleGroup / isGroupCollapsed / groupLabel` — 分组折叠状态用 usePersistentRecord('hermes.sidebar.collapsedGroups') 持久化到 localStorage；groupLabel 在 sidebarCollapsed=true 时返回缩写版
- `components/layout/AppSidebar.vue:isNavActive / hasRoute` — isNavActive 统一处理子路由名映射（session→chat、historySession→history）；hasRoute 用 router.hasRoute 做条件渲染，防止访问未注册路由
- `components/layout/AppSidebar.vue:handleUpdate / handleReloadClient / handleLogout` — 自更新：handleUpdate 调 appStore.doUpdate()；客户端版本落后时 handleReloadClient 刷页；handleLogout localStorage.clear() + router.replace('login')
- `components/layout/AppSidebar.vue:openChangelog` — 版本号文本点击触发 NModal 展示内置 changelog 数据（本地数组），无需外链
- `components/layout/ModelSelector.vue:filteredGroups (computed)` — 搜索过滤：对 modelGroupsWithCustom 每组 models 过滤 displayName/model id 包含 query（safeLower 防 null 崩溃），组名本身也参与搜索
- `components/layout/ModelSelector.vue:handleSelect` — 选中时检查 model_meta[model].disabled，阻止通过点击绕过禁用状态
- `components/layout/ModelSelector.vue:handleCustomSubmit` — 自定义模型输入：同样检查 model_meta disabled，防止 custom input 绕过列表里的灰显限制（注释明确写明此意图），Enter 提交
- `components/layout/ModelSelector.vue:modelGroupsWithCustom (computed)` — 把 appStore.customModels[provider] 里不在官方列表的条目合并进各 group.models，切换 profile 后自定义模型不丢失
- `components/layout/ProfileSelector.vue:loadRuntimeStatuses` — 带 cancellation token（runtimeRefreshToken）的异步请求：token 不匹配时丢弃结果，防止并发请求竞态覆盖最新数据
- `components/layout/ProfileSelector.vue:scheduleRuntimeStatusPoll` — 指数退避轮询：首次 700ms，后续 1200ms，最多 12 次；仅在 showProfileModal=true 时继续；refreshing 返回值决定是否继续
- `components/layout/ProfileSelector.vue:handleAvatarFileChange` — 前端头像上传：校验 mime type（png/jpeg/webp）和大小（<=1MB）后 FileReader.readAsDataURL 转 base64，再调 profilesStore.updateAvatar({type:'image', dataUrl})
- `components/layout/ProfileSelector.vue:handleRandomAvatar / handleResetAvatar` — 随机头像生成传 {type:'generated', seed:'profile-时间戳-随机串'}；重置调 profilesStore.deleteAvatar
- `components/layout/ProfileSelector.vue:gatewayStatusText / bridgeStatusText` — 运行时状态文字：running=null→'checking'，true→'running'/'active'，false→'stopped'/'idle'
- `components/layout/ThemeSwitch.vue` — 双按钮：toggleBrightness（light/dark）+ toggleStyle（ink/comic），图标随当前状态切换 SVG
- `composables/useTheme.ts:useTheme` — 模块级单例 ref（brightness/style/isDark/isComic），监听 prefers-color-scheme 媒体查询变化；watch 分别持久化到 localStorage 并 applyClasses 同步 document.documentElement.classList
  - 💡 ref 定义在模块顶层而非 composable 函数体内，多次调用 useTheme() 共享同一实例，无需 Pinia
- `composables/usePersistentRecord.ts:usePersistentRecord` — 轻量 localStorage 绑定：返回 reactive record 对象和 persist 函数，调用方决定何时持久化（手动 persist()），避免频繁写 localStorage
  - 💡 command-flush 模式：修改立即生效在内存，持久化是显式操作
- `components/layout/LanguageSwitch.vue:handleChange` — 切换语言：调 switchLocale(val) + localStorage.setItem('hermes_locale', val)，10 种语言选项
- `components/auth/AuthEventListener.vue:onAuthNotice` — 监听全局 CustomEvent 'hermes-auth-notice'，1200ms 防抖去重（lastNoticeAt 时间戳对比），区分 forbidden/expired 两种错误类型显示不同 toast；组件本身 display:none 无视觉输出
- `components/auth/DefaultCredentialPrompt.vue:checkDefaultCredentials` — route 变化时触发：跳过 desktop shell（window.hermesDesktop.isDesktop）和 login 页；token 无变化时跳过；拉取 currentUser，若 requiresCredentialChange=true 且本次 session 未 dismiss，弹提醒 modal
- `components/auth/DefaultCredentialPrompt.vue:dismissalKey / remindLater` — per-user sessionStorage key 做「本次会话不再提醒」：key 格式 hermes_default_credentials_prompt_dismissed_{userId}，关闭浏览器后恢复提醒
- `components/common/RouteLinkItem.vue` — 包装 RouterLink：用 v-slot custom 模式自行渲染 <a>，支持 active prop 覆盖默认激活判断（exact/isActive/isExactActive），注入 aria-current='page' 供无障碍使用
  - 💡 active prop 可外部传入（处理多子路由映射单个激活态的场景），fallback 到 Vue Router 的 isActive/isExactActive

### views 全部(22) — hermes-web-ui  ·  70 项

- `LoginView.vue::handleLogin` — 触发密码登录流程入口，防空校验后调用 handlePasswordLogin
  - 💡 onMounted 静默预检 authStatus，connect 错误不阻断页面渲染
- `LoginView.vue::handlePasswordLogin` — 调用 loginWithPassword 获取 sessionToken，存入 setApiKey 后 router.replace('/hermes/chat')；429/503 触发 showLockResetHint 显示 CLI 解锁命令
- `ChannelsView.vue::loadSettingsForProfile` — 先确保 profiles 已加载，再拉取 settings；职责是 PlatformSettings 组件的数据前置
- `ChatView.vue::loadRouteSession` — 按路由 sessionId 加载会话列表；若 sessionId 不存在则 replace 到 chat 根路由
- `ChatView.vue::(watch tabTitle)` — 活跃会话标题变化时同步 document.title，unmounted 时重置为产品名
- `CodingAgentsView.vue::defaultLaunchApiMode` — 根据 provider key 和 base_url 自动推断 API 协议(anthropic_messages/chat_completions/codex_responses)；优先匹配 anthropic，本地地址降级为 chat_completions
- `CodingAgentsView.vue::loadConfigFile` — 按 agentId+fileKey 读取配置文件内容，回填 configEditorStates 并记录 originalContent 用于 dirty 检测
- `CodingAgentsView.vue::saveConfigFile` — 写回配置文件，成功后更新 originalContent 和 absolutePath/exists 状态
- `CodingAgentsView.vue::hasConfigUnsavedChanges` — 对比 content 与 originalContent 检测未保存变更，控制 Save 按钮 disabled 状态
- `CodingAgentsView.vue::openLaunchModal` — 获取当前 profile 可用模型列表，初始化 launch 弹窗的 provider/model/apiMode 选择
- `CodingAgentsView.vue::launchBuiltInTerminal` — 调用 prepareCodingAgentLaunch 获取 shellCommand，注入 TerminalPanel 嵌入式终端
- `CodingAgentsView.vue::launchNativeTerminal` — 调用后端在宿主系统打开原生终端窗口启动 coding agent
- `CodingAgentsView.vue::parseErrorPayload` — 从 err.message 字符串中提取 JSON payload(node_environment_missing 等 code)，用于精确错误展示
- `FilesView.vue::handleContextMenu` — 转发右键事件到 FileContextMenu ref 的 show 方法
- `FilesView.vue::handleShowNewFile/handleShowNewFolder/handleRename` — 设置 renameMode 和 renameEntry 后展示 FileRenameModal
- `FilesView.vue::loadRoot` — 确保 profiles 加载后调用 filesStore.fetchEntries('') 拉取根目录
- `GroupChatView.vue::syncRouteRoom` — 路由 roomId 存在但不在 rooms 列表中时 replace 到 groupChat 根路由；已加入则跳过
- `HistoryView.vue::loadHermesSessions` — 带请求序列号防竞态地拉取所有历史会话列表
- `HistoryView.vue::loadHistorySession` — 优先用 fetchSessionMessagesPage 分页加载，不支持时 fallback 到 fetchHermesSession 全量加载
- `HistoryView.vue::loadOlderHistoryMessages` — 追加更早消息(offset 累加)，去重后前插 messages 列表，更新 hasMoreBefore 状态
- `HistoryView.vue::mapHistoryMessages` — 将后端 HermesMessage 格式规范化为前端 Session['messages'] 格式，tool 消息提取 toolCallId/toolArgs/toolResult
- `HistoryView.vue::sessionFromSummary` — 从 SessionSummary 构建完整 Session 对象，时间戳统一转为毫秒
- `HistoryView.vue::sessionSelectionKey` — 用 NUL 字节拼接 profile+id 生成批量操作的唯一 key，避免不同 profile 下同 id 冲突
- `HistoryView.vue::groupedSessions` — 按 source 分组会话，api_server 组排首位，cron 排末位，其余按字母序；pinned 会话独立前置
- `HistoryView.vue::toggleGroup` — 折叠时记录状态到 localStorage；展开时自动加载并选中该组第一个会话
- `HistoryView.vue::handleBatchDelete` — 批量删除选中会话，调用 batchDeleteSessions，成功后重新加载列表并显示部分失败警告
- `HistoryView.vue::buildHistorySessionUrl` — 构建包含 origin+pathname 的完整分享链接，支持带 profile query param
- `HistoryView.vue::handleImportToWebUi` — 将 Hermes CLI 原生会话导入 WebUI 数据库，结果已存在时提示不重复导入
- `JobsView.vue::reloadJobsForProfile` — 切换 profile 时清空旧 jobs 再重新加载，确保数据隔离
- `JobsView.vue::handleSelectJob` — 点选已选 job 时取消选中(toggle)，用于关联 JobRunHistory 过滤
- `KanbanView.vue::applyBoardSelection` — 调用 recoverSelectedBoard 容错，更新路由 query，有切换时触发 refreshAll
- `KanbanView.vue::handleStatusChipClick` — 点击状态统计芯片时过滤任务并收起其他列，只展开目标状态列
- `KanbanView.vue::handleCreateBoard` — 创建新看板并 replace 路由到新看板
- `KanbanView.vue::handleArchiveSelectedBoard` — window.confirm 确认后归档当前看板，自动切回 DEFAULT 看板
- `LogsView.vue::parseAccessLog` — 用正则从 access log 消息提取 method/path/status，用于专用格式渲染
- `LogsView.vue::levelClass` — ERROR/WARNING/DEBUG/INFO 映射到 CSS class，控制颜色和左边框颜色
- `McpManagerView.vue::parseConfig` — 按 inputMode(json/yaml) 解析配置文本，统一返回 {data, error}
- `McpManagerView.vue::extractServers` — 自动展开 mcpServers/mcp_servers 包装层，兼容两种常见配置格式
- `McpManagerView.vue::validateServerConfig` — 每个 server entry 必须有 command 或 url 字段，否则返回错误信息
- `McpManagerView.vue::handleModeChange` — JSON/YAML 切换时在两种格式间互转当前文本内容
- `McpManagerView.vue::handleInput` — 输入时即时校验，通过后设 1500ms 防抖定时器自动格式化文本
- `McpManagerView.vue::scheduleReload` — 延迟重新拉取 servers 列表，用于等待后端重连更新状态
- `McpManagerView.vue::loadServers (auto-retry)` — 有 enabled 但未连接的 server 时，指数退避(2s→4s→8s→16s→32s)最多重试5次
- `McpManagerView.vue::openToolsModal` — 读取 server.raw_config.tools 恢复 include/exclude 模式和已选 tools，打开工具可见性管理弹窗
- `McpManagerView.vue::saveToolsVisibility` — all 模式删除 tools 字段，include/exclude 模式写入对应数组，保存后触发 scheduleReload
- `MemoryView.vue::startEdit/cancelEdit/handleSave` — 内联编辑状态机：idle→editing→saving→idle，保存后重新加载并清空 editingSection
- `MemoryView.vue::formatTime` — 时间戳格式化为 'MMM DD HH:mm' 短格式用于显示各节的修改时间
- `ModelsView.vue::loadProvidersForProfile` — 先调用 checkCopilotToken 使后端 Copilot 缓存失效，再拉取 providers，错误静默
- `PerformanceView.vue::setAutoRefresh` — 切换自动刷新时清除旧 timer 再按 5s 间隔重建，onBeforeUnmount 清理
- `PerformanceView.vue::formatBytes/formatPercent/formatDuration` — 三个纯格式化函数，formatBytes 自动选单位(B/KB/MB/GB/TB)，formatDuration 输出天/小时/分
- `PluginsView.vue::statusTagType` — 将 effectiveStatus 映射到 Naive UI NTag 的 type('success'/'error'/'info'/'warning'/'default')
- `PluginsView.vue::pluginCommand` — 根据 effectiveStatus 生成启用/禁用的 hermes CLI 命令字符串；provider-managed 不生成
- `ProfilesView.vue::(3 modal handlers)` — handleCreated/handleRenamed/handleImported 关闭对应弹窗，利用 ProfilesPanel 内部状态更新列表
- `SettingsView.vue::normalizeTab` — 将路由 query.tab 规范化为已知 tab，非法值回退到 'account'
- `SettingsView.vue::handleTabUpdate` — 切换 tab 时同步 router.replace(query.tab)，默认 tab 不写入 query
- `SkillsUsageView.vue::chartSegments` — 将日数据拆分为 N 个 top skill 和 other 两类 segment，分别着色用于堆叠条形图
- `SkillsUsageView.vue::colorForSkill` — 按 top skill 列表索引取调色板颜色，不在 top 的统一用 otherSkillColor
- `SkillsUsageView.vue::tooltipAlignment` — 悬停列索引>一半时向左对齐 tooltip，避免溢出右侧
- `SkillsUsageView.vue::loadStats` — 带请求序列号防竞态，结果按 period 缓存在 statsByPeriod，切换周期可瞬间复用缓存
- `SkillsView.vue::loadRecommendations` — 按当前 locale 加载对应语言的 Markdown 推荐文档，检测返回 HTML(404 fallback)时清空
- `SkillsView.vue::handleSelect` — 同一 skill 再次点击时取消选中；移动端选中后自动收起 sidebar
- `TerminalView.vue::buildWsUrl` — 优先用 base URL 配置，否则取 location.host；开发环境支持 VITE_HERMES_DIRECT_WS_PORT 绕过代理
- `TerminalView.vue::getOrCreateTerm` — 懒创建 xterm Terminal 实例，注册 FitAddon/WebLinksAddon，onData 直接写 WS
- `TerminalView.vue::switchSession` — 切换 activeSessionId，调用 mountActiveTerminal 在 DOM 容器中换入对应 terminal 元素
- `TerminalView.vue::mountActiveTerminal` — 首次 open()；已 opened 时用 appendChild 移动已有 DOM 元素(不重建，保留 scrollback buffer)
- `TerminalView.vue::handleTerminalTouchMove` — 手动实现触摸滚动：累积 deltaY 除以 TOUCH_SCROLL_LINE_PX 换算行数调用 scrollLines，防止系统滚动冲突
- `TerminalView.vue::applyTheme` — 运行时修改所有存活 terminal 实例的 options.theme，持久化到 localStorage
- `TerminalView.vue::handleControl` — WS 控制消息路由：created 追加 session 并 switch；exited 标记退出状态并写提示文本；error 弹 message
- `UsageView.vue::loadUsage` — 设置 selectedPeriod 后确保 profile 已加载，调用 usageStore.loadSessions(days)
- `VersionPreviewView.vue::(view)` — 纯壳 view，直接渲染 GithubPreviewSettings 组件

### 前端架构(状态/路由/api/i18n/composables)  ·  45 项

- `main.ts:(bootstrap)` — 应用启动前同步读 localStorage 里的 brightness/style 并写 documentElement.classList，消除 FOUC；读 URL token 存入 window.__LOGIN_TOKEN__；按序 use(Pinia) → use(i18n) → use(router) 后 mount
  - 💡 在 createApp 之前就操作 DOM 类，保证暗色模式不闪，这是业界标准但执行点早于 Vue 生命周期
- `api/client.ts:getBaseUrl` — 按优先级返回 baseUrl：VITE_HERMES_PREVIEW=1 → 空串（同源），isDesktopShell() → 空串，否则读 localStorage hermes_server_url
  - 💡 三种运行模式(preview/desktop/remote)用同一函数完全透明
- `api/client.ts:request` — 通用 fetch 包装，自动注入 Authorization Bearer JWT、X-Hermes-Profile header（跳过 profile-wide session 集合端点），处理 401/403 全局重定向到 login 并 dispatch 自定义 hermes-auth-notice 事件
  - 💡 区分 isLocalBff（本地 BFF）和代理网关请求，代理请求 401 不触发登出，避免第三方 API 鉴权错误强制退登
- `api/client.ts:getStoredUserRole` — 客户端解析 JWT payload（base64url decode）读取 role 字段，无需 server round-trip
- `api/client.ts:shouldAttachProfileHeader` — 精细控制哪些请求应附加 X-Hermes-Profile 头；排除条件：URL 已有 profile 参数、profile-wide session 集合路径、body 已含 profile 字段
- `api/client.ts:emitAuthNotice` — window.dispatchEvent CustomEvent('hermes-auth-notice') 传递 kind=expired\|forbidden；AuthEventListener 组件监听并弹通知，解耦鉴权层与 UI 通知层
- `router/index.ts:(beforeEach guard)` — 三段路由守卫：public 页面若已有 key 直接跳 chat；无 key 的非 public 页面跳 login；requiresSuperAdmin 页面检查 isStoredSuperAdmin()
- `i18n/index.ts:resolveLocale` — 按 navigator.languages 数组依次 normalize：zh 系区分繁简（HANT/TW/HK/MO），非 zh 取前两位匹配 supportedLocales，都不中则 fallback 'en'
- `i18n/index.ts:switchLocale` — 运行时切换 i18n locale ref 并同步 document.documentElement.lang，用于 AppSidebar 里的 LanguageSwitch 组件
- `i18n/messages.ts:mergeMessagesWithFallback` — 递归 deep-merge：locale 消息 key 覆盖 en fallback，只覆盖已翻译的 key，未翻译 key 自动继承英文。比 vue-i18n 原生 fallback 更精细（原生 fallback 是整个 key，这里是 nested key 粒度）
- `composables/useTheme.ts:useTheme` — 模块级单例 ref（brightness/style/isDark/isComic），监听 window.matchMedia prefers-color-scheme change，watch 变化写 localStorage 并 toggle documentElement.classList。返回 setBrightness/setStyle/toggleBrightness/toggleStyle
- `composables/useKeyboard.ts:useKeyboard` — 全局键盘快捷键：Ctrl/Meta+N 新建会话、Ctrl+J 跳 Jobs、Ctrl+K 开 session search modal、Esc 关 search 或 naive-ui modal。onMounted/onUnmounted 绑定/解绑
- `composables/useSessionSearch.ts:useSessionSearch` — 模块级单例 ref sessionSearchOpen，暴露 openSessionSearch/closeSessionSearch。多个组件（键盘处理器+modal）共享同一状态不重复创建
- `composables/usePersistentRecord.ts:usePersistentRecord` — 通用 localStorage 持久化 reactive Record<string,boolean>，返回 {record, persist}，由调用方决定何时 persist，不自动 watch
- `composables/useToolTraceVisibility.ts:useToolTraceVisibility` — 模块级单例 toolTraceVisible ref（默认 true），持久化到 hermes_show_tool_calls；toggleToolTraceVisible 控制是否显示工具调用详情
- `composables/useSpeech.ts:useSpeech` — 三引擎 TTS composable：server-side Edge/Google TTS（generateSpeech API）、Browser WebSpeech API、OpenAI-compatible HTTP TTS、MiMo TTS（chat/completions audio 格式）。内置播放队列、playbackToken 防竞态、server TTS 失败自动 fallback 到 browser
  - 💡 playbackToken 自增机制是防止竞态的标准做法；extractReadableText 过滤 <thinking> 标签和代码块
- `composables/useSpeech.ts:useGlobalSpeech` — 懒初始化全局单例 useSpeech 实例，防止多个组件各自创建独立的语音状态
- `composables/useVoiceSettings.ts:useVoiceSettings` — 模块级多 ref 持久化 TTS 设置（provider/webspeech/openai/custom/edge/mimo 各自参数），watch 数组一次性监听所有字段写 localStorage，包含 migrateOldKeys 迁移旧 hermes-tts-settings
- `stores/hermes/app.ts:useAppStore` — 全局应用 store：sidebar 状态（mobile open + desktop collapsed）、连接检查（health poll 30s）、model 列表（30s TTL 缓存、dedup promise）、model 选择/切换/自定义/alias/visibility、版本更新检测
- `stores/hermes/app.ts:applyAvailableModelsResponse` — 根据 activeProfile 解析 server 返回的 model groups，优先匹配 (defaultProvider+defaultModel)，fallback 顺序：explicitGroup → inferredGroup → unlistedDefault → fallbackGroup
  - 💡 处理 profile 特定模型、custom model 注入、visibility 过滤三重优先级逻辑
- `stores/hermes/app.ts:waitForModelsForRun` — 发送消息前等待 model 列表加载完成（最多等 15s），避免 race condition：用户快速发消息时 model 还未就绪
- `stores/hermes/chat.ts:useChatStore` — 核心聊天 store：session CRUD、Socket.IO 流式事件处理（20+事件类型）、消息队列管理（queuedUserMessages/dequeuedQueueIds）、pending approval/clarify 状态机、abort 控制、<think> 边界检测（thinkingObservation Map）、tab 可见性重同步
- `stores/hermes/chat.ts:sendMessage` — 发送消息完整流程：文件上传 → buildContentBlocks → 等待 model 就绪 → startRunViaSocket → 处理 20+种流式事件 → 竞态检测 swallowedError → 自动语音播放。shouldQueue 判断是否进入排队模式
- `stores/hermes/chat.ts:resumeServerWorkingRun` — 页面刷新后恢复进行中的 run：注册 sessionEventHandlers map，重新 join socket room，streamStates 设置 abort 函数。不重发 resume（switchSession 已做），只补 listener
- `stores/hermes/chat.ts:mapHermesMessages` — 把后端 HermesMessage[] 转为前端 Message[]：过滤无内容 assistant 消息、展开 tool_calls 为 tool 消息、对齐 toolName/toolArgs、提取 JSON preview
- `stores/hermes/chat.ts:handleSubagentEvent` — 处理 subagent.start/tool/progress/complete 事件，以 toolCallId='subagent:{run_id}:{subagent_id}' 为 key upsert tool 消息，记录进度/状态/duration
- `stores/hermes/chat.ts:switchSession` — 切换 session：清旧状态 → Socket.IO resume（等待 15s 超时）→ 处理回放 events（compression/abort/approval 等）→ resumeServerWorkingRun。两路都可触发（route change + 侧边栏点击）
- `stores/hermes/chat.ts:setItemBestEffort` — LocalStorage 写入：QuotaExceededError 时先 recoverStorageQuota（清 prefix 旧缓存），再重试一次，仍失败静默忽略
- `stores/hermes/usage.ts:useUsageStore` — Usage 统计 store：requestId 竞态保护（latestRequestId 自增），computed 派生 totalTokens/cacheHitRate/estimatedCost/modelUsage（按 hash 分配固定颜色）/dailyUsage
- `stores/hermes/kanban.ts:useKanbanStore` — Kanban store：多 requestSeq + boardGeneration 双重防竞态（board 切换时 generation++ 使旧请求失效），WebSocket event stream（有 events capability 时开启），reconnect 3s 自动重试，capability 断言机制
- `stores/hermes/kanban.ts:connectEventStream` — WebSocket 连接 kanban 事件流，收到事件后 debounce 100ms 刷新数据（scheduleEventRefresh），连接断开后 3s 重连
- `stores/hermes/settings.ts:useSettingsStore` — 设置 store：fetchSettings 从 server 拉全量配置到各 section ref；saveSection 写 server + 乐观更新本地 ref；updateLocal 不写 server（用于即时 UI 响应）
- `stores/hermes/session-browser-prefs.ts:useSessionBrowserPrefsStore` — 会话浏览器偏好（pin/human-only 过滤）：profile 感知 key（PIN_KEY_PREFIX + profile），watch profilesStore.activeProfileName 自动 reload，pruneMissingSessions 清理已删除会话的 pin
- `stores/hermes/profiles.ts:useProfilesStore` — profile CRUD + 切换（switchProfile 同时 reloadModels）+ avatar 管理 + 导入/导出，activeProfileName 启动时同步读 localStorage
- `stores/hermes/models.ts:useModelsStore` — 专用 models store（与 appStore.modelGroups 互补）：fetchProviders 按 activeProfile 拉取、refreshModelCache 触发 server 刷新缓存、addProvider/removeProvider 管理自定义 provider
- `api/hermes/chat.ts:startRunViaSocket` — Socket.IO emit start_run，注册 sessionEventHandlers，返回 {abort} controller
- `api/hermes/chat.ts:registerSessionHandlers / unregisterSessionHandlers` — sessionEventHandlers Map 管理每个 session 的事件回调（20+ 类型），全局 socket 事件按 session_id 路由到对应 handler
- `api/hermes/chat.ts:(Socket reconnect logic)` — TRANSIENT_DISCONNECT_REASONS set 识别瞬断，重连时重新 register 所有 global listeners，避免重复注册用 globalListenersRegistered 标志
- `api/hermes/jobs.ts:scheduleToEditableInput` — 将 JobSchedule（string\|interval\|cron\|once union）统一转为可编辑字符串，用于表单回显；scheduleToDisplayText 则转为展示文本
- `utils/thinking-parser.ts:parseThinking` — 解析 <think>/<thinking>/<reasoning> 标签，先 protectCodeBlocks（占位符替换）防止代码块内的标签被误匹配，再用 TAG_RE 提取 segments，streaming 模式保留未闭合的 pending
- `utils/thinking-parser.ts:detectThinkingBoundary` — 检测流式增量中 <think> open/close 边界的首次出现（prev→next），用于 chat store 中 thinkingObservation 计时
- `utils/completion-sound.ts:playCompletionSound` — 用 AudioContext 合成一个 0.16s 880→660Hz 正弦波完成提示音（无外部资源依赖），primeCompletionSound 在用户手势时 resume AudioContext 规避自动播放策略
- `utils/clipboard.ts:copyToClipboard` — 优先 navigator.clipboard（secure context），fallback textarea+execCommand('copy')（HTTP intranet 部署兼容）
- `shared/session-display.ts:getSourceLabel / formatTimestampMs` — getSourceLabel 把 source 字段（telegram/cli/cron 等）映射为显示名；formatTimestampMs 今天显示时间，其他显示 month day
- `styles/theme.ts:getThemeOverrides` — 返回 naive-ui GlobalThemeOverrides：light/dark 两套 + comic 模式（替换 fontFamily 为 Comic Neue/手写字体），作为 NConfigProvider themeOverrides 注入

### Koa BFF server + other packages (hermes-web-ui)  ·  69 项

- `packages/server/src/index.ts:bootstrap` — 应用启动入口：创建目录、初始化登录限流器、注入技能、启动 profile 网关、初始化 SQLite、注册 Koa 中间件/路由、绑定 WebSocket/Socket.IO、启动 SessionDeleter、注册 catch-all WS 升级拦截
  - 💡 desktop runtime 时延迟启动网关（先监听端口再启动），避免 race；两次 setTimeout 1000ms 等待 SQLite ready（脆弱）
- `packages/server/src/index.ts:listenWithFallback` — 在指定 host:port 上监听，返回 {primary, servers} 以便后续多个 WS 实例 attach
- `packages/server/src/index.ts:safeNetworkInterfaces` — 安全读取网络接口（Termux/proot 兼容，捕获 errno 13）
- `packages/server/src/config.ts:getWebUiHome` — 读取 HERMES_WEB_UI_HOME / HERMES_WEBUI_STATE_DIR 或回退 ~/.hermes-web-ui
- `packages/server/src/services/login-limiter.ts:initLoginLimiter` — 加载持久化锁文件并清理过期 IP 锁，返回 Promise
- `packages/server/src/services/login-limiter.ts:checkPassword` — 检查 password 登录是否被限流（全局 + 双 IP map）
- `packages/server/src/services/login-limiter.ts:checkToken` — 检查 token 登录是否被限流
- `packages/server/src/services/login-limiter.ts:recordPasswordFailure` — 记录密码失败，超阈值立即持久化锁（同步写文件），触发全局锁或 IP 锁
- `packages/server/src/services/login-limiter.ts:recordTokenFailure` — 记录 token 失败，逻辑同 recordPasswordFailure
- `packages/server/src/services/login-limiter.ts:pruneIpMap` — IP map 超 10000 时删除已过期锁，仍超则 LRU 删除最老
- `packages/server/src/services/login-limiter.ts:getLockedIps / unlockIp / unlockAll` — 管理员 API：列出/解锁单 IP/清除所有锁
- `packages/server/src/services/safe-file-store.ts:SafeFileStore` — 线程安全（per-path promise 队列）的文件读写封装，支持 Text/YAML，原子写（tmp→rename），可选备份
- `packages/server/src/services/safe-file-store.ts:SafeFileStore.withLock` — 基于 Promise 链的 per-filePath 互斥锁，无需真正的 Mutex
- `packages/server/src/services/safe-file-store.ts:SafeFileStore.updateText / updateYaml` — read-modify-write 原子更新，updater 函数可返回 {content, result} 携带额外返回值
- `packages/server/src/services/credentials.ts:hashPassword` — scrypt(N=16384, r=8, p=1) 哈希密码，64 字节输出
- `packages/server/src/services/credentials.ts:setCredentials / verifyCredentials` — 文件系统（~/.hermes-web-ui/.credentials, mode 0o600）存储用户名+密码哈希+salt
- `packages/server/src/services/claude-code-proxy.ts:registerClaudeCodeProxyTarget` — 注册一个 Anthropic Messages 格式代理目标（provider+model+baseUrl），返回本地 proxy URL + 单次 token，支持 chat_completions/codex_responses/anthropic_messages/bedrock_converse 四种 apiMode
- `packages/server/src/services/claude-code-proxy.ts:anthropicToOpenAiChat` — Anthropic Messages 请求体 → OpenAI Chat Completions 请求体转换，含 tool_calls/thinking/reasoning 字段
- `packages/server/src/services/claude-code-proxy.ts:anthropicToOpenAiResponses` — Anthropic Messages → OpenAI Responses API 格式转换
- `packages/server/src/services/claude-code-proxy.ts:openAiChatToAnthropicSseStream` — 从 OpenAI SSE 流转换为 Anthropic SSE 流（generator），处理 text/tool/thinking delta，状态机管理 block indices
- `packages/server/src/services/claude-code-proxy.ts:anthropicMessagesSseStream` — 直接透传 Anthropic Messages 上游 SSE 流
- `packages/server/src/services/claude-code-proxy.ts:openAiResponsesToAnthropicSseStream` — OpenAI Responses API SSE → Anthropic SSE 格式转换（generator），处理 response.output_text.delta/function_call 事件
- `packages/server/src/services/claude-code-proxy.ts:claudeProxyMessages` — Koa handler：分发 stream/non-stream 请求到对应适配器，统一 Anthropic 格式出口
- `packages/server/src/services/codex-proxy.ts:registerCodexProxyTarget` — 注册 OpenAI Responses API 格式代理目标（含 profile 维度），返回本地 proxy URL+token
- `packages/server/src/services/codex-proxy.ts:responsesToOpenAiChat` — OpenAI Responses API 请求体（含 function_call/function_call_output）→ Chat Completions 转换
- `packages/server/src/services/codex-proxy.ts:openAiChatToResponsesSseStream` — Chat Completions SSE → Responses API SSE 格式转换（generator），包含 response.created/output_item.added/output_text.delta/completed 事件
- `packages/server/src/services/codex-proxy.ts:anthropicMessagesToResponsesSseStream` — Anthropic Messages SSE → Responses API SSE 转换，通过 Anthropic event 标签路由
- `packages/server/src/services/codex-proxy.ts:codexProxyResponses` — Koa handler：codex 使用 Responses API 格式进来，出口统一为 Responses API 格式
- `packages/server/src/services/hermes/context-engine/compressor.ts:ContextEngine` — 群聊 context 压缩引擎：Path A（有快照→增量压缩）/ Path B（无快照→全量压缩），per-room Promise 锁防并发快照覆写
- `packages/server/src/services/hermes/context-engine/compressor.ts:ContextEngine.buildContext` — 公开入口：序列化锁 + 委托 _buildContextImpl，threshold 判断后选择 verbatim/压缩路径
- `packages/server/src/services/hermes/context-engine/compressor.ts:ContextEngine._buildContextImpl` — 核心逻辑：token 估算（支持外部 contextTokenEstimator 精确估算）→ 阈值判断 → 调用 summarize → 保存快照 → 返回 history
- `packages/server/src/services/hermes/context-engine/compressor.ts:ContextEngine.countTokens` — 区分 CJK（1.5 tok/char）和 Latin（config.charsPerToken）的 token 估算
- `packages/server/src/services/hermes/context-engine/compressor.ts:ContextEngine.trimToBudget` — 压缩失败降级：从 history 末尾弹出消息直到满足 maxHistoryTokens
- `packages/server/src/services/hermes/run-chat/index.ts:ChatRunSocket` — Socket.IO /chat-run namespace：单聊 run 调度器，管理 sessionMap(SessionState)、run 队列、abort、approval/clarify 响应
- `packages/server/src/services/hermes/run-chat/index.ts:ChatRunSocket.onConnection` — 连接 handler：监听 run/cancel_queued_run/resume/abort/approval.respond/clarify.respond 事件
- `packages/server/src/services/hermes/run-chat/index.ts:ChatRunSocket.handleRun` — run 分发：source='cli' → handleBridgeRun，否则 → handleApiRun
- `packages/server/src/services/hermes/run-chat/index.ts:ChatRunSocket.dequeueNextQueuedRun` — 从队列取下一个等待 run，emit run.queued 更新前端队列状态
- `packages/server/src/services/hermes/run-chat/handle-api-run.ts:handleApiRun` — API 模式 run：构建压缩 history → 调用上游 /v1/responses SSE → 解析 → 写 DB → 发 socket 事件
- `packages/server/src/services/hermes/run-chat/handle-api-run.ts:loadSessionStateFromDb` — 从 SQLite 恢复 SessionState（含 snapshot-aware context tokens 计算）
- `packages/server/src/services/hermes/group-chat/index.ts:GroupChatServer` — Socket.IO /group-chat namespace：多用户/多 agent 房间，持久化到 SQLite（ChatStorage），ContextEngine 集成，@mention 路由
- `packages/server/src/services/hermes/group-chat/index.ts:ChatStorage` — 群聊 SQLite 数据访问层：rooms/messages/members/agents/snapshots/pending_session_deletes 全 CRUD
- `packages/server/src/services/hermes/group-chat/index.ts:ChatStorage.saveMessageAndRefreshRoom` — BEGIN IMMEDIATE 事务：upsert 消息 + 剪枝 + 重算 totalTokens + UPDATE room，原子性
- `packages/server/src/services/hermes/group-chat/index.ts:sortGroupMessages` — 群聊消息排序：按 baseId+phase（tool_call/result/assistant 三态）稳定排序，解决并发消息乱序
- `packages/server/src/services/hermes/group-chat/index.ts:GroupChatServer.handleMessage` — 群聊消息广播 + @mention 路由触发，限制 mentionDepth 防止 agent-to-agent 无限循环
- `packages/server/src/services/hermes/group-chat/index.ts:GroupChatServer.handleTyping / handleStopTyping` — typing 状态跟踪（30s 超时自动清除），rejoin 时恢复
- `packages/server/src/services/hermes/group-chat/index.ts:GroupChatServer.restoreAgents` — 服务启动后重新连接所有持久化 agent，恢复群聊成员
- `packages/server/src/services/hermes/agent-bridge/manager.ts:AgentBridgeManager` — 管理 Python hermes_bridge.py 子进程：启动/停止/自动重启（指数退避）、等待 stdout JSON ready 事件
- `packages/server/src/services/hermes/agent-bridge/manager.ts:AgentBridgeManager.startProcess` — spawn Python 桥接进程，解析 stdout 中 {event:'ready'} JSON 行判断就绪；desktop 模式额外轮询 TCP 端口
- `packages/server/src/services/hermes/agent-bridge/manager.ts:resolveAgentBridgeCommand` — 多策略 Python 可执行文件探测：venv/uv/shebang/系统 python，跨平台（Win/Unix）
- `packages/server/src/services/hermes/agent-bridge/manager.ts:buildAgentBridgeProcessEnv` — 构建桥接进程环境变量，注入 HERMES_AGENT_BRIDGE_ENDPOINT + OpenRouter attribution headers
- `packages/server/src/services/hermes/gateway-manager.ts:GatewayManager` — 多 profile 网关进程生命周期管理：detectStatus（只读）/resolvePort（空闲端口分配）/start/stop/startAll（两阶段：顺序分配端口+并行启动）
- `packages/server/src/services/hermes/gateway-manager.ts:GatewayManager.detectStatus` — 只读检测：PID 文件存活 + health check 双重确认，不认领未知端口上的进程
- `packages/server/src/services/hermes/gateway-manager.ts:GatewayManager.resolvePort` — 端口分配：优先复用健康运行的已配置端口，否则从 8642 起递增探测空闲端口（避开 web UI 端口）
- `packages/server/src/services/hermes/gateway-manager.ts:GatewayManager.stop` — 多层级停止：hermes CLI stop → 内存子进程 SIGTERM → PID kill → lsof/netstat 找监听 PID force kill，等待 health 失败确认
- `packages/server/src/services/hermes/skill-injector.ts:HermesSkillInjector` — 启动时将 bundled skills 目录同步（覆盖更新）到所有 profile 的 skills 目录，支持多 profile
- `packages/server/src/services/hermes/skill-injector.ts:HermesSkillInjector.injectMissingSkills` — 遍历 sourceDir 技能目录，对每个 targetDir（default + 所有 profile）执行删除重写（非增量）
- `packages/server/src/middleware/user-auth.ts:requireUserJwt` — Koa 中间件：验证 Bearer JWT，HMAC-SHA256 自签发（无第三方库），写 ctx.state.user
- `packages/server/src/middleware/user-auth.ts:signUserJwt / verifyUserJwt` — 手写 JWT（HS256）：base64url header.payload.signature，含 aud/exp/type 字段，timing-safe compare
- `packages/server/src/middleware/user-auth.ts:resolveUserProfile` — 从 x-hermes-profile header / query / body 读取 profile，鉴权后写 ctx.state.profile
- `packages/server/src/db/hermes/usage-store.ts:updateUsage / getLocalUsageStats` — Token 用量持久化到 SQLite（session 级），getLocalUsageStats 聚合按 model/按天统计（含 cache_read/cache_write/reasoning tokens）
- `packages/server/src/services/hermes/ops-monitor.ts:getOpsRuntimeSnapshot` — 系统+进程运行快照：ping agent bridge → 获取所有 worker PID → 跨平台收集 CPU/内存（Linux /proc, macOS vm_stat, Windows PowerShell），返回结构化 OpsRuntimeSnapshot
- `packages/server/src/services/hermes/ops-monitor.ts:parseMacVmStatMemory` — 解析 vm_stat 输出（active+wired+compressed pages × pageSize）计算真实内存用量，比 os.freemem 更准确
- `packages/server/src/services/hermes/hermes-kanban.ts:watchEvents` — spawn `hermes kanban watch` 子进程，返回 ChildProcess 供 SSE/WS 桥接
- `packages/server/src/services/hermes/hermes-kanban.ts:bulkUpdateTasks` — 逐 task 串行调用 complete/block/unblock/archive/assign CLI，返回 per-task 成功/失败结果
- `packages/server/src/services/hermes/hermes-kanban.ts:getCapabilities` — 静态声明 kanban 功能支持矩阵（supported/partial/missing），供前端特性检测
- `packages/server/src/services/hermes/hermes-kanban.ts:listTasks / createTask / getTask / completeTasks / blockTask / assignTask` — Hermes CLI kanban 命令包装，统一 exec 调用模式，JSON 输出解析
- `packages/server/src/db/hermes/usage-store.ts:getUsageBatch` — 批量查询多 session 最新用量（MAX(id) GROUP BY session_id 子查询），避免 N+1
- `packages/desktop/src/main/index.ts:bootstrap (Electron main)` — Electron 主进程：Tray 托盘菜单、BrowserWindow、IPC、runtime 下载管理（Node/Python/Git 打包运行时）、自动更新
- `packages/desktop/src/main/runtime-manager.ts:ensureDesktopRuntime` — 桌面运行时管理：检测/下载/验证 Node+Python 打包运行时，支持增量更新

## 3. 页面元素穷举

### chat 组件(单聊) — hermes-web-ui packages/client/src/components/hermes/chat/

#### ChatInput — 输入区顶部工具栏(input-top-bar)
- **元素**：附件按钮(📎 icon，trigger fileInput.click)；自动播放语音开关(▶ 图标 + NSwitch，localStorage持久化)；工具调用显隐切换(扳手图标 NButton，active 状态高亮)；context-info 文本(「Xk / Yk · 剩余 Zk」，>80% 变橙色警告)；可点击的 context-limit 数字(虚线下划线，hover 出背景，click 弹 modal 编辑)；60px 宽 4px 高进度条(normal/warn/danger 三色)
- **交互**：附件按钮→隐藏 input[file] 点击；语音开关变化→同步 chatStore.setAutoPlaySpeech；工具调用按钮→useToolTraceVisibility().toggleToolTraceVisible；context-limit 点击→showContextEditModal=true
- **UX 细节**：context-limit 数字用虚线下划线+hover 背景暗示可编辑(非传统 input 样式)，减少 UI 噪音；进度条颜色只在 >60% 时变黄、>80% 变红，正常使用时完全中性

#### ChatInput — slash 命令下拉(slash-command-dropdown)
- **元素**：Transition fade 入场；每行 grid(auto auto 1fr)：/name(accent色 monospace) + args(muted monospace) + 描述(secondary截断)；active 行 accent 背景
- **交互**：↑↓ 循环导航；Enter/Tab 选中插入；Esc 关闭；mouseenter 跟随高亮；mousedown.prevent 选中(阻止 blur)；document mousedown 点外关闭
- **UX 细节**：只在 source==='cli' 的 bridge session 中激活；grid-template-columns: auto auto 1fr 保证 /name 和 args 不压缩，描述自适应宽度并截断

#### ChatInput — 输入框区域(input-wrapper)
- **元素**：顶部 8px resize-handle(cursor:row-resize，hover 出 accent 半透明背景)；textarea(自适应高度 scrollHeight capped 100px，手动拖后固定)；拖拽进入时 dashed border + info 色背景；发送按钮(primary)；流式时显示停止按钮(error, disabled during aborting)
- **交互**：Enter 发送(Shift+Enter 换行)；IME 保护；拖拽文件/粘贴图片；resize-handle 拖动；文件遮罩 click 后 textarea.focus
- **UX 细节**：resize-handle 位于 wrapper 顶部 absolute -4px，8px 高可交互区；textareaHeight null 表示自适应，一旦用户手动拖则改为固定高度，不再被 scrollHeight 覆盖

#### ChatInput — 附件预览区(attachment-previews)
- **元素**：图片：64×64 thumb(object-fit:cover)；文件：图标+文件名+大小 垂直排列；右上角 18px × 按钮(hover 出现，50%圆形黑底白字)
- **交互**：× 按钮 removeAttachment(revoke ObjectURL)；图片 click 暂无 lightbox(不同于 MessageItem 有 previewUrl)
- **UX 细节**：× 按钮 opacity:0 默认隐藏，.attachment-preview:hover 才 opacity:1，减少视觉干扰

#### ChatInput — 上下文长度编辑 Modal
- **元素**：NModal preset=card 400px；描述文案；NInputNumber(min:1000 max:10M step:1000 无按钮 suffix:tokens)；提示文案；取消/保存按钮
- **交互**：保存调 setModelContext API；失败 message.error 带具体错误；保存中 loading+disabled；按 provider+model+profile 三元组持久化
- **UX 细节**：NInputNumber 去掉 show-button 减少视觉复杂度；hint 文案告知重启不失效

#### ChatPanel — 会话列表侧栏(session-list)
- **元素**：标题「Sessions」+ 操作栏(关闭×、批量模式checkbox图标、全选/删除、+ 新建)；profile 过滤 NSelect；置顶分组 header(PINNED N) + SessionListItem 列表；空态/loading 文案
- **交互**：移动端 <768px 时变 absolute + backdrop 遮罩；swipe/click 外部关闭；MediaQueryList 监听 resize
- **UX 细节**：showSessions 初始化从 matchMedia 同步读取，避免首屏 flash（组件默认 true 然后 onMounted 改 false 会导致侧栏覆盖内容后消失）

#### ChatPanel — 会话列表批量操作模式
- **元素**：进入批量模式：checkbox 图标变全选图标 + 红色删除按钮(NPopconfirm)；每个 SessionListItem 显示 NCheckbox；
- **交互**：toggleBatchMode 切换；selectAllSessions 排除活跃 session；selectedSessionKeys Set 按 profile\0id 复合 key；Popconfirm 二次确认批量删除
- **UX 细节**：批量删除中(isBatchDeleting) 所有操作 disabled；活跃 session 不能被批量选中(排除逻辑)

#### ChatPanel — 右键 Session 上下文菜单
- **元素**：手动定位 NDropdown(x/y 像素)；选项：置顶/取消置顶、重命名、设置工作区、切换模型(仅 cli source)、导出(full/compressed × json/txt 四种)、新标签打开、复制链接、复制 ID
- **交互**：contextmenu 事件设 x/y 后 show=true；clickoutside 关闭；各选项弹对应 Modal
- **UX 细节**：导出选项用嵌套 children 实现 mode → ext 两级选择；compressed 导出时显示 message.loading toast

#### ChatPanel — 审批条(approval-bar)
- **元素**：盾牌图标(32px accent 色圆角方块)；kicker 大写小字 + 标题 + 描述 + command 代码块(max-height 96px scrollable)；动作按钮(once primary / session secondary / always secondary / deny error)
- **交互**：按 choices 数组条件渲染按钮；点击调 chatStore.respondApproval(choice)；移动端 grid 两列布局
- **UX 细节**：command 代码块 monospace + overflow-auto，长命令可滚动查看

#### ChatPanel — 澄清条(clarify-bar)
- **元素**：? 圆圈图标；kicker + 标题 + 问题描述；有 choices 时：选项按钮组 + 「忽略」按钮；无 choices 时：文本输入行 + 提交按钮
- **交互**：有 choices→handleClarify(choice)；无 choices→handleClarify()取输入值；Dismiss 传 '' 清除
- **UX 细节**：clarify-input-row 用 flex：NInput flex:1 NButton flex:0，移动端<420px 转 column+stretch

#### ChatPanel — 新建会话 Modal
- **元素**：NModal card 440px；Profile NSelect；Provider NSelect；Model NSelect(filterable)；取消+新建按钮
- **交互**：Profile 变化→syncNewChatModelSelection 自动预选 provider+model；Provider 变化→重置 model；Model 可过滤搜索；三者都选才启用新建
- **UX 细节**：getDefaultModelForProfile 按 profileModelGroups 的 default_provider/default_model 字段自动预填，减少用户操作

#### ChatPanel — 会话模型切换 Modal(session-model-modal)
- **元素**：搜索框；可折叠 provider 分组列表(group header 点击折叠/展开，计数)；每个 model 行：display name + canonical id(alias 场景) + preview/disabled/custom badge + active ✓；自定义 model 输入区(provider NSelect + model NInput, Enter 提交)
- **交互**：搜索过滤 model 名和 display name；disabled model 不可点击 cursor:not-allowed；custom model 输入 Enter 提交；点击立即切换并关闭 modal
- **UX 细节**：session-model-badge-preview/custom/disabled 三种徽章用不同背景色(amber/accent/transparent+border)；max-height:50vh 限制弹窗高度

#### ChatPanel — header 区
- **元素**：折叠侧栏按钮(四宫格图标)；session 标题(截断)；workspace badge(📁 显示最后一段路径)；大纲按钮(三横线)；复制 session ID 按钮；新建会话按钮(移动端只显示图标)
- **交互**：大纲按钮 toggle showOutline；workspace badge hover title 显示完整路径
- **UX 细节**：header 左侧 flex+overflow:hidden 保证标题截断不撑宽；workspace badge max-width:160px text-overflow:ellipsis

#### ChatPanel — 浮动抽屉按钮(drawer-button-wrapper)
- **元素**：绝对定位右侧垂直居中 40px 圆形按钮；彩虹流光边框(rainbow-glow 动画 8s 循环 6色)；hover 暂停动画+加强 glow
- **交互**：click → showDrawer=true
- **UX 细节**：彩虹动画 6步关键帧覆盖 red→yellow→cyan→pink→blue→purple；hover 暂停 animation-play-state 让用户看清按钮

#### VirtualMessageList — 虚拟滚动容器
- **元素**：DynamicScroller(vue-virtual-scroller)；#before slot(历史加载 spinner)；#item slot(MessageItem)；#after slot(streaming-indicator/queue-float-panel)；#empty slot
- **交互**：@scroll.passive 同步 scrollTop/viewportHeight；@resize ResizeObserver 回调；@top-reach 触发翻页；@visible 同步 viewport
- **UX 细节**：fade-in 动画 1.5s 减少首次加载视觉跳动；prefers-reduced-motion 禁用动画；CSS 变量 --virtual-row-gap/--virtual-list-padding 父组件注入

#### MessageList — 流式指示器(streaming-indicator, #after slot)
- **元素**：思考 GIF(120×213px)；tool-calls-panel：中止指示(Pausing.../Paused, 对应图标)；压缩指示(Compressing.../Compressed Nmsg ~Xk→Yk tokens，含进度/完成图标)；工具调用列表(逆序，每行：扳手图标+name mono+preview截断+耗时+running spinner/done绿✓/error红×)
- **交互**：Transition fade；tool-calls-panel max-height 213px scrollable(隐藏 scrollbar)；tool-call-item click 无交互(只读)
- **UX 细节**：GIF 和 panel 并排 flex-start align，GIF 固定宽；panel overflow hidden scrollbar，减少视觉混乱；tool 耗时单位自动选 ms/s/min

#### MessageList — 消息队列浮窗(queue-float-panel, sticky)
- **元素**：sticky right:16px bottom:16px，min(340px,calc(100%-16px))；header：orbit 旋转动画 + 「消息队列」+ 数量 badge；items list：序号圆角方块+预览文字(48字截断)+× 删除按钮
- **交互**：Transition queue-float(opacity+translateY+scale)；× 按钮 removeQueuedMessage；移动端 <640px 缩小至 260px
- **UX 细节**：queue-orbit: 18px 圆圈内有 6px 点做轨道旋转动画，glowing box-shadow；数量 badge 自动居右 margin-left:auto

#### MessageItem — 工具消息行(tool-line)
- **元素**：可展开时：右箭头 chevron(旋转展开)；否则：扳手图标；工具名(mono flex-shrink)；预览文字(flex:1 截断，折叠时才显)；running spinner；error badge
- **交互**：click 切换 toolExpanded；展开时显示 tool-details(border-left)；Arguments/Result 各自代码块含 diff 渲染
- **UX 细节**：tool-preview max-width:min(400px,100%) 防超长；error badge 9px font 极小，不抢注意力

#### MessageItem — 思考块(thinking-block)
- **元素**：header：箭头 chevron + 💭 + 「思考中/思考」label + 耗时(秒) + 字符数；body：MarkdownRenderer italic 0.85 opacity border-left
- **交互**：click header 切换 thinkingOverride；流式时强制展开；settingsStore.display.show_reasoning 默认值
- **UX 细节**：thinkingDurationMs 用 setInterval+nowTick 实时计时(流式中)；停止后用 endedAt 静态值；字符数从 reasoning 字段+parsed <think> 段合并计

#### MessageItem — 气泡 message-meta(hover 才显)
- **元素**：TTS 播放/暂停按钮(条件渲染)；复制整条气泡按钮；时间戳 HH:MM
- **交互**：TTS 按钮切换 playing 态(pulse 动画)、paused 态(无动画 opacity:0.6)；复制调 copyToClipboard；移动端 <768px 常驻显示
- **UX 细节**：message-meta opacity:0 默认隐藏，.message:hover 改 1；mobile 打破此规则常驻以便触摸操作

#### MessageItem — 流式中 streaming-dots
- **元素**：3个 6px 圆点 pulse 动画(0/0.2/0.4s 延迟)；仅在 isStreaming && !content 时显示
- **交互**：无，纯动画
- **UX 细节**：只在「assistant 消息流式中但正文还是空」时显示；内容出现后消失；避免空白气泡无反馈

#### MessageItem — 图片 lightbox(Teleport to body)
- **元素**：fixed inset:0 rgba(0,0,0,0.85) overlay；图片 max 90vw/90vh object-contain；click 关闭
- **交互**：msg-attachment-thumb click → previewUrl=att.url；overlay click.self 或 img click 关闭
- **UX 细节**：Teleport to body 避免 z-index 层叠问题

#### ConversationMonitorPane — 只读会话监控
- **元素**：左栏 260px session list：每项 = 标题 + live badge(accent色「近期」) + source·时间 + 预览；右侧 detail：header(title+source+model+linked sessions 数) + 消息列表(user/assistant 不同 border 色)
- **交互**：session 点击切换 selectedSessionId；humanOnly prop 控制过滤；15s 定时 silent 刷新；移动端 column layout 侧栏 max-height:220px
- **UX 细节**：silent refresh 不显示 loading；requestId 防竞态；session list 右侧 scrollbar gutter:stable 防内容偏移

#### OutlinePanel — 大纲导航
- **元素**：固定 280px 右侧面板 border-left；header「大纲」；user 消息：「Q: 内容」圆角方块(bg-secondary)；assistant h1-h3：level-1/2/3 缩进(0/12/24px)，字体大小渐小渐淡
- **交互**：click item → emit('navigate', {messageId, anchorId}) → ChatPanel→MessageList.scrollToAnchor
- **UX 细节**：移动端 absolute right:0 shadow，不占文档流；level-3 字体 12px muted 色，视觉层级明确

#### SessionListItem — 会话列表项
- **元素**：批量模式 NCheckbox；title row：pin 图标(11px) + 流式 spinner(旋转) + 标题(截断) + 缺模型警告按钮(! 圆形红)；meta row：model badge(accent bg 截断100px) + 时间；profile row：16px ProfileAvatar + profile name；删除按钮(NPopconfirm, hover 才显)
- **交互**：普通模式 → <a href> 路由跳转；批量模式 → <button> toggle-select；移动端长按(500ms) 触发 contextmenu
- **UX 细节**：component :is='a'/'button' 动态切换语义元素；to prop 配合 <a> 实现 ctrl+click 新标签打开

#### SessionSearchModal — 全局会话搜索
- **元素**：NModal card 760px；标题行+说明+搜索范围提示；NInput large size clearable；结果列表/最近会话列表：每项 = title+source(右) + snippet(2行clamp) + 时间+message_id(mono)；footer：快捷键提示 + 取消按钮
- **交互**：160ms 防抖搜索；↑↓/Enter/Esc 键盘导航；mouseenter 跟随高亮；Enter 打开 + 路由跳转 + 若不在 store 则 addOrUpdateSession 注入
- **UX 细节**：无搜索词时显示最近8条 session 作为默认列表，避免空搜索框空状态

#### DrawerPanel — 右侧抽屉(Files/Terminal)
- **元素**：Teleport to body；overlay(click 关闭)；drawer-panel min(1180px,88vw) slide-in(transition:right 0.3s)；tab 按钮(Files/Terminal)；× 关闭按钮；内容区 v-show 保活
- **交互**：tab 切换 v-show(TerminalPanel 保持连接)；overlay/close 按钮 update:show；移动端 100% 全宽
- **UX 细节**：v-show 而非 v-if 保持 xterm 连接不断；transition right 比 transform 更语义化，但效果相同

#### TerminalPanel — 内嵌 xterm 终端
- **元素**：xterm.js Terminal + FitAddon + WebLinksAddon；4套主题(default/solarized-dark/tokyo-night/github-dark) NSelect；多 session tab 切换；新建/关闭 session 按钮；连接错误提示
- **交互**：WS 连接 /api/hermes/terminal；FitAddon resize 自适应；theme 变化调 term.options.theme；visible prop 控制 resize 触发
- **UX 细节**：主题存 localStorage；termMap 按 sessionId 管理多个 Terminal 实例

#### FilesPanel — 文件浏览器
- **元素**：左树(FileTree) + 右主区(FileBreadcrumb + FileToolbar + FileList/FileEditor/FilePreview)；移动端树用 sidebar overlay；右键菜单(FileContextMenu)；上传 Modal；重命名/新建 Modal
- **交互**：FileList @contextmenu-entry → contextMenuRef.show；工具栏 new-file/new-folder/upload；filesStore.fetchEntries('')初始化
- **UX 细节**：编辑/预览/列表三态互斥：filesStore.editingFile > filesStore.previewFile > FileList

#### FolderPicker — 工作区目录选择器
- **元素**：懒加载树(扁平 FlatNode[] 渲染)；每节点：depth×16px 缩进 + ▶/▼ 展开图标 + 目录名；NSpin 加载中；选中高亮(accent bg)
- **交互**：toggleExpand：已缓存直接展开，未缓存先请求 /api/hermes/workspace/folders?path=；click 选中调 emit('update:modelValue')
- **UX 细节**：hasChildren:null 表示未知(未加载子目录前)，展开后才知道是否有子节点；扁平数组+depth 渲染避免递归组件

#### MarkdownRenderer — 富渲染内容区
- **元素**：markdown-it 渲染：标题带 id、代码块带语言标签+copy按钮、diff 行号/折叠、KaTeX 公式块、Mermaid 图(占位→异步渲染)、本地文件卡片(下载+预览按钮)、视频 player、@mention span.mention-highlight；图片 lightbox overlay；文件预览 NDrawer
- **交互**：代码块 copy 按钮 handleCodeBlockCopyClick 事件委托；file card click 下载或右侧 Drawer 预览；Mermaid 图渲染后 scroll 位置恢复
- **UX 细节**：SUPPORT_PREVIEW_FILE_TYPES 白名单决定文件是否可预览；mentionNames prop 按长度降序排序正则(防短名匹配到长名的子串)

### group-chat + jobs + kanban

#### GroupChat 主面板
- **元素**：左侧 room 列表侧边栏（220px）：房间名/邀请码/token 用量/删除按钮；移动端侧边栏 position:absolute + backdrop 遮罩；主区域 header：侧边栏 toggle 按钮、房间名、头像叠放栈（最多 4 个+N）、添加 agent 按钮、压缩配置按钮、清空 context 按钮、成员数、WS 连接状态点
- **交互**：右键 room 列表项弹出 NDropdown（copy link / clone room）；点击头像叠放栈弹出 popover（显示 user+agents 列表，agents 可移除）；房间列表 toggle select；移动端 sidebar 点击 backdrop 关闭
- **UX 细节**：连接状态点：connected=绿+glow shadow，disconnected=红；房间列表 hover 时 action 按钮才显示（opacity 0→1）；头像叠放 margin-left:-12px 压叠，hover 时 translateY(-2px)+z-index:100 浮起

#### GroupChat 状态/审批栏
- **元素**：状态栏：多 agent 同时 typing/compressing 时并排展示 chip（@agentName + typing dots + stop 按钮）；审批栏：shield icon、kicker（PERMISSION REQUEST）、title（@agentName · 需要权限）、desc、command 代码块、操作按钮组（once/session/always/deny）
- **交互**：stop 按钮 interruptAgent；审批按钮 respondApproval；移动端审批按钮 2 列网格，超小屏 1 列
- **UX 细节**：审批 command 用等宽字体 scrollable 代码块（max-height:96px）；审批栏 border+card 背景区分于聊天内容；状态 chip 横向 flex 超出横滚不换行

#### GroupChatInput 输入框
- **元素**：顶部工具栏：附件按钮、自动播放语音开关（NSwitch）、tool trace 可见性切换；附件预览区：图片 64x64 缩略图/文件名+大小；拖拽区：drag-over 时 border 变 accent 色+半透明 bg；resize-handle（顶部 8px 横条）；textarea（auto-resize，可拖拽自定义高度）；发送按钮；mention 下拉（position:fixed，fade+scale 动画，top/bottom flip）
- **交互**：mention 下拉 ArrowUp/Down/Enter/Tab/Escape；拖拽 resize（mousedown 起点计算 deltaY）；拖拽文件 drop；粘贴图片；文件 input 多选；附件 hover 显示 remove 按钮（opacity 0→1）
- **UX 细节**：auto-resize textarea rows=1 起步 max-height:100px，拖拽后固定高度（不再 auto）；drag-over 视觉反馈明确；mention all 条目 accent 色加粗区分普通 agent；IME composing 期间不触发发送

#### GroupMessageItem 消息气泡
- **元素**：avatar（36px 圆角 8px）；sender name + agent description（斜体）；消息气泡（自己右对齐+accent bg，agent 浅 accent bg，error 红色边框+文字）；thinking block（折叠/展开，显示字符数）；markdown 渲染区；流式占位 dots；attachment 区（图片 96px/文件带下载链接）；tool message：单行 chip（chevron + tool icon + name + preview + spinner/error badge）+ 可展开 args/result 代码块；message meta（hover 才显示）：TTS 播放/暂停按钮、复制按钮、时间戳
- **交互**：thinking block 点击 toggle 展开；tool chip 点击展开 detail；tool detail 代码块复制（事件委托）；attachment 图片点击 lightbox（全屏遮罩）；TTS 播放 rainbow-glow 边框动画；message meta hover fade in
- **UX 细节**：TTS 播放中气泡边框 rainbow-glow（7色循环 CSS animation）；agent error 消息 error 色贯穿 markdown 内所有子元素（deep 覆盖）；tool spinner 旋转动画；tool-preview 单行溢出省略（max-width min(400px,100%)）

#### GroupChat 创建房间弹窗（CreateRoomForm）
- **元素**：你的昵称/描述输入；房间名输入；邀请码输入（带刷新按钮自动生成）；折叠区（NCollapse）内嵌压缩参数（triggerTokens/maxHistoryTokens/tailMessageCount），三个 NInputNumber
- **交互**：Enter 在 yourName 跳到 roomName；roomName Enter 提交；邀请码刷新按钮；折叠区默认收起
- **UX 细节**：压缩配置隐藏在折叠里降低首次创建复杂度；用户名存 localStorage（下次预填）

#### JobCard 卡片
- **元素**：header：job name + status badge（success/info/warning/error）；body：info 行（schedule/model/lastRun+状态/nextRun/deliver/repeat）；footer actions：pause/resume + runNow + edit + delete（NTooltip 说明）
- **交互**：点击 card body 选中（selected 样式）；点击 action 区不触发选中；pause/resume/run/delete 各自操作
- **UX 细节**：selected 状态 border 变 accent+轻 accent bg；action 按钮在 border-top 分割线下排列；lastRun 后紧跟 ok/err 小 badge；repeat 支持 string 和 {completed/times} 对象两种格式展示

#### JobFormModal 创建/编辑弹窗
- **元素**：name（带 count）、schedule（cron 表达式）、quickPresets（NSelect 预设选项）、prompt（textarea 5000字 带 count）、deliverTarget（下拉，未配置渠道 disabled）、repeatCount（NInputNumber 可 clearable）
- **交互**：选 preset 直接覆盖 schedule 输入框；deliverTarget 未配置渠道 disabled 可见但不可选；编辑模式 diff-only 更新
- **UX 细节**：mask-closable 在 loading 时为 false 防止误关；deliverTarget disabled 让用户知道有该渠道能力但需配置；渠道检测按不同平台校验不同字段

#### JobRunHistory 历史面板
- **元素**：header：title + 运行次数；accordion 列表：每项标题=job名—运行时间，右侧 size；展开后 MarkdownRenderer 渲染内容
- **交互**：accordion 展开时懒加载详情内容（NSpin 占位）；selectedJobId 过滤展示特定 job 的历史
- **UX 细节**：accordion 模式（同时只展开一个）；size 用等宽字体；内容走 MarkdownRenderer 支持 AI 输出格式

#### KanbanColumn 看板列
- **元素**：NCollapse 可折叠列，标题 = Unicode 状态图标 + 状态名 + (count)；任务卡列表；空状态提示
- **交互**：NCollapse 默认展开（default-expanded-names=[status]），display-directive=show（DOM 不销毁）；点击卡片 emit taskClick
- **UX 细节**：display-directive=show 保证折叠后 DOM 保留，滚动位置不丢；flex 布局 flex:1 1 calc(20%-12px) 五列均分，min-width:200px 小屏换行

#### KanbanTaskCard 任务卡
- **元素**：title；meta 区：assignee（头像+名字 tag）、priority badge（high/medium/low，低优先级不显示）、相对时间（justNow/N分钟/N小时/N天）；body 预览（前 80 字符+省略）
- **交互**：点击 emit click(taskId)
- **UX 细节**：border-left 3px 用 CSS 变量 --kanban-card-status-color 按状态着色，hover 时 border-color 变为状态色（从 border-color 变具体色），微妙动画；assignee avatar 带 accent 色 border-ring

#### KanbanTaskDrawer 任务详情抽屉
- **元素**：NDrawer right placement 420px；元数据区（status/ID/parent/children/assignee/priority/tenant/created/started/completed）；body 区；result summary（可点击展开 HistoryMessageList modal）；操作区：complete（两步，可附 summary）/ block（输入 reason）/ unblock / assign（select+按钮）；相关 sessions 区（懒加载）；runs 历史；comments；events（最近 10 条）
- **交互**：parent/children task link 点击 emit navigate；result summary 点击打开 messages modal；sessions 标题点击 toggle+懒加载；assign 选 profile 后点按钮执行；block/complete 均两步确认；关闭 emit close
- **UX 细节**：canMutateTask 控制操作区可见（done/archived 隐藏）；race condition 防护（watch snapshot 比对）；session-messages modal 可在抽屉内弹出（独立于路由）；task ID 可直接 user-select:all 复制；run status badge 与 kanban status badge 颜色体系一致

### models + profiles + mcp + skills 组件

#### ProvidersPanel（供应商面板）
- **元素**：响应式 grid 卡片列表（auto-fill minmax 420px）；空态 SVG 图标+文案
- **交互**：无直接交互，由父页面提供「添加供应商」按钮触发 ProviderFormModal
- **UX 细节**：空态用线框 SVG（opacity 0.3）而非彩色插图，低噪声

#### ProviderCard（供应商卡片）
- **元素**：标题行（provider name + builtin/custom/default 三种 badge）；信息行（provider key / base_url / model 数量）；模型标签列表（可滚动 100px 高度区域，最多显示 20 个 + more 标签）；操作栏（别名管理/可见模型/删除三个 tiny 按钮）；内嵌 alias list modal；内嵌 alias edit modal；内嵌 visibility modal（checkbox list）
- **交互**：模型标签 click → 弹 alias 编辑 modal；「别名管理」→ 弹 alias list modal；「可见模型」→ 弹 visibility checkbox modal；「删除」→ dialog 二次确认（Copilot 特殊文案）
- **UX 细节**：模型标签有 alias 时：别名文字大+原始 ID 小号灰色一行展示；有别名的模型才显示原始 ID，干净；默认模型标签黄色底色高亮

#### ProviderFormModal（添加供应商弹窗）
- **元素**：preset/custom 切换按钮（非 radio，视觉上是 toggle button）；preset 时：provider 下拉（filterable）+ 可选 apikey.fun 链接；custom 时：name 输入+自动生成；base_url 输入（preset 有些禁用）；api_key 密码输入（show-password-on-click）；model 下拉（tag 模式可输入）+ 「获取」按钮；context_length（custom 专有）；阿里巴巴 region 单选（preset 专有）
- **交互**：选 Codex → 触发 CodexLoginModal；选 Copilot → 先检测 token → 有则确认 dialog；选 xAI → 触发 XaiOAuthLoginModal；选 Nous → 触发 NousLoginModal；model 下拉变化 → 自动识别 apikey.fun 路由；base_url 变化 → 自动填充 name
- **UX 细节**：「获取」按钮只在可拉取目录的 provider 下显示（排除 Codex/Nous/Copilot/xAI 等非 OpenAI-compatible）；tag 模式允许手动输入 model ID 不在列表时也能用

#### CodexLoginModal / CopilotLoginModal / NousLoginModal（Device Flow Modal）
- **元素**：5 态内容区域：loading spinner；waiting（提示文字+user_code 大号 monospace+复制图标+「打开授权链接」按钮）；approved（绿色 checkmark SVG+成功文字）；expired（错误文字+重试按钮）；error（错误文字+重试按钮）；页脚 cancel 按钮（waiting 状态禁用）
- **交互**：user_code 区域整体 click 复制；「打开链接」→ window.open 新标签；cancel 在 waiting 状态禁用（防误操作）；自动轮询 3s 一次
- **UX 细节**：user_code 用 font-size:28px / letter-spacing:4px 强可读性；code box hover 边框高亮提示可点击；approved 后 1s 延迟关闭（让用户看到成功态）

#### XaiOAuthLoginModal（xAI OAuth Modal）
- **元素**：loading spinner；waiting（提示文字+「打开授权」按钮+「复制链接」按钮）；approved（绿色 checkmark）；expired；error；页脚 cancel（waiting 禁用）
- **交互**：startLogin 自动弹新窗口；「打开授权」再次打开；「复制链接」复制 URL；2s 轮询（比 Device flow 3s 快）
- **UX 细节**：无 user_code，改为复制完整 URL，适合 redirect_uri 流；比 device flow 少一步

#### AuxiliaryModelsPanel（辅助模型配置面板）
- **元素**：面板头（标题+说明+刷新按钮）；4 列表格（任务名/provider-model/timeout/操作）；每行 edit+clear 按钮；编辑 modal（2 列网格：provider 下拉+model 下拉+timeout+download_timeout（vision 专有）+extra_body textarea 全宽）
- **交互**：edit → 弹 modal 编辑（表单联动：provider 变 → model 重置）；clear → 直接重置为 auto（无确认）；refresh → 重新拉取
- **UX 细节**：provider 选 auto 时 model 禁用且 extra_body 清空（auto 不支持自定义）；vision 任务多出 download_timeout 字段（按需显式字段）；4 列表格比卡片列表更紧凑，适合多任务对比

#### ProfilesPanel（配置集面板）
- **元素**：响应式 grid（同 ProvidersPanel 结构）；空态
- **交互**：父级提供创建/导入触发按钮
- **UX 细节**：ProfileCard 的 active 态用绿色边框（success 色，非蓝色 accent），区别于 provider 的 hover 态

#### ProfileCard（配置集卡片）
- **元素**：头部（ProfileAvatar 28px + name + active tag）；信息行（model）；折叠展开区（provider/path/skills count/hasEnv/hasSoulMd）；操作栏（切换/删除/导出三按钮）
- **交互**：折叠箭头 click → 懒加载 detail；切换 → dialog 二次确认 → 成功后 location.reload()；删除 → dialog 确认（default profile 和 active profile 禁用删除）；导出 → 下载 tar.gz
- **UX 细节**：折叠图标 rotate 180deg 过渡；detail 区有 NSpin loading 态；删除按钮 disabled 而非隐藏，配合 title 说明原因

#### ProfileCreateModal（新建配置集弹窗）
- **元素**：name 输入（实时字符过滤）；校验提示文字（NText warning）；clone switch；clone 说明文字（NText secondary）
- **交互**：输入时 transform 过滤非法字符；clone=on 显示清理说明；提交成功时若有 stripped 清单则 info(6000ms) 提示
- **UX 细节**：clone switch 比 radio/checkbox 更语义化；6000ms 超长 toast 确保用户能读完清理清单

#### ProfileImportModal（导入配置集弹窗）
- **元素**：NUpload（max=1，accept .tar.gz/.tgz/.gz/.zip）；选文件按钮；确认按钮（disabled when no file）
- **交互**：beforeUpload 校验扩展名；选文件后确认按钮 enabled；上传为 multipart FormData POST
- **UX 细节**：Upload 原生组件，不 drag-and-drop 全宽；max=1 限制单文件

#### ProfileRenameModal（重命名弹窗）
- **元素**：new name 输入（同 create 的字符过滤逻辑）；校验提示
- **交互**：与 CreateModal 共用相同的 handleNameInput 过滤逻辑
- **UX 细节**：只有 name 字段，极简

#### McpServerCard（MCP 服务器卡片）
- **元素**：头部（server name + transport badge + 状态 badge）；error 文字（-webkit-line-clamp:2 折叠）；工具数量行；工具标签列表（88px 高 overflow-y auto，max 20 + more）；操作栏（edit/manageTools/test/reload/remove + NPopconfirm 确认删除）；右侧 NSwitch 快速启停
- **交互**：remove 用 NPopconfirm（inline 确认，不弹 modal）；manageTools disabled when disconnected；switch toggle → emit toggleEnabled 给父级；所有操作均 emit 给父级
- **UX 细节**：disconnected 时卡片红色边框（error-rgb 0.3）；disabled 时 opacity 0.7 整体暗化；工具 tag hover 有背景加深（区别于不可点的 provider model tag）；NPopconfirm 内联确认比 dialog 轻量

#### SkillList（技能列表面板）
- **元素**：按 category 分组的可折叠列表；每个 category header（箭头+名称+数量 badge）；skill item（source dot + name + modified badge ✎ + description + NSwitch）；独立归档区（底部分隔，默认折叠）
- **交互**：category header click 折叠/展开（用 Set 管理状态）；skill item click → emit select；switch toggle → handleToggle（per-skill loading）；搜索+source filter 实时过滤
- **UX 细节**：source dot 4 色区分来源（builtin 灰/hub 蓝/local 绿/external 黄）；modified ✎ 符号标记本地修改过的 builtin/hub skill；toggle loading 用 togglingSkills Set 精确到 skill 粒度；归档区默认折叠、底部分割线视觉隔离

#### SkillDetail（技能详情面板）
- **元素**：标题行（category/skill 面包屑+pin 按钮+view_count/use_count/patch_count 三项统计）；面包屑（文件导航时显示+返回按钮）；内容区（MarkdownRenderer 渲染 SKILL.md）；附属文件列表（文件 icon+路径，按钮触发查看）
- **交互**：pin 图标 click → toggle pin（filled SVG 表示已 pin）；附属文件 button → viewFile → 面包屑+MarkdownRenderer 切换；back 按钮返回主文档
- **UX 细节**：pin 按钮 active 时 fill SVG（实心图钉）；disabled 时 cursor:wait+opacity:0.3；use_count 用闪电图标/view_count 用眼睛图标/patch_count 用铅笔图标，语义清晰；文件列表 border+hover accent 色，可读性好

### settings 全部面板(15)

#### AccountSettings — 头像区
- **元素**：ProfileAvatar 预览组件(80px) / 上传按钮(触发 hidden file input, accept .png/.jpg/.jpeg/.webp ≤1MB) / 随机头像按钮(multiavatar 生成 SVG) / 重置默认按钮 / 状态 loading 覆盖三个按钮
- **交互**：点击上传→文件选择→自动 compress→POST；点击随机→crypto.randomUUID 种子→POST；点击重置→DELETE/RESET API
- **UX 细节**：上传按钮和随机/重置共用 avatarSaving loading 状态，三个按钮都在 saving 时显示 :loading；file input 隐藏，通过 button.click() 触发，避免丑陋的原生 file 控件

#### AccountSettings — 登录信息区
- **元素**：当前用户名展示文本 / 修改密码按钮 / 修改用户名按钮
- **交互**：按钮打开对应 Modal；Modal 内回车可触发提交(keyup.enter)
- **UX 细节**：修改密码 Modal：当前密码 + 新密码 + 确认密码三个 password input，show-password-on='click'；修改用户名 Modal：当前密码 + 新用户名，密码用于二次鉴权

#### AccountSettings — IP 锁定管理
- **元素**：锁定数量文本 / 刷新按钮 / 全部解锁按钮(NPopconfirm 二次确认) / 锁定列表(IP + 类型 badge(红) + 剩余时间) / 每条解锁按钮(NButton type=error ghost tiny)
- **交互**：全量解锁需 confirm；单条直接调 API；刷新手动重载
- **UX 细节**：lockedUntil 换算剩余分钟，过期显示 i18n 'expired'；locked-badge 红色背景渲染 type(密码错误/IP封锁等)

#### AgentSettings
- **元素**：max_turns: NInputNumber [1..200 step=5] / gateway_timeout: NInputNumber [60..7200 step=60] / restart_drain_timeout: NInputNumber [10..300 step=10] / tool_use_enforcement: NSelect [auto/always/never]
- **交互**：数字输入防抖 300ms 保存；下拉直接保存
- **UX 细节**：每个 SettingRow 带 hint 解释语义；数字框 class='input-sm' 宽度约 100px，label 不截断

#### CompressionSettings
- **元素**：enabled: NSwitch / threshold: NInputNumber [0.1..0.95 step=0.05] / target_ratio: NInputNumber [0.05..0.8 step=0.05] / protect_last_n: NInputNumber [0..200] / protect_first_n: NInputNumber [0..50]
- **交互**：开关直接保存；数字防抖 300ms
- **UX 细节**：defaults 对象兜底 ?? 运算符，保证控件有初始值不显示 undefined

#### DisplaySettings
- **元素**：theme: NSelect [light/dark/system] / streaming: NSwitch / compact: NSwitch / show_reasoning: NSwitch / show_cost: NSwitch / inline_diffs: NSwitch / bell_on_complete: NSwitch / busy_input_mode: NSwitch(interrupt/off 映射 boolean)
- **交互**：主题切换即时生效(setBrightness 改 DOM) + 异步持久化；其他开关直接 saveSection
- **UX 细节**：busy_input_mode 是字符串枚举但对外呈现为 boolean Switch(v ? 'interrupt' : 'off')，简化用户认知

#### GithubPreviewSettings
- **元素**：tag-select: NSelect(filterable, 260px) / Prepare 按钮(primary) / Install 按钮 / Start 按钮(success) / Stop 按钮 / Refresh 按钮 / NAlert info 说明 / NDescriptions 状态表(9行：path/webui_home/current_tag/has_package/installed/running+PID/frontend_url/log_path/dev_log_path) / action_log pre 区 / dev_log pre 区
- **交互**：操作按钮 disabled 当有 activeAction；active_action 时自动 2s 轮询；日志区 pre 滚动 max-height 320px
- **UX 细节**：状态 NTag 颜色：has_package/installed→success/warning；running→success/default + PID；前端 URL 可点击 target=_blank；操作完成时弹 toast（通过 watch last_action_completed_at）

#### MemorySettings
- **元素**：memory_enabled: NSwitch / user_profile_enabled: NSwitch / memory_char_limit: NInputNumber [100..10000 step=100] / user_char_limit: NInputNumber [100..10000 step=100]
- **交互**：开关直接保存；数字防抖 300ms
- **UX 细节**：两个开关分别控制「AI 记忆」和「用户画像」，char_limit 上限约束 prompt token 占用

#### ModelSettings
- **元素**：NSpin 包裹 provider 列表 / 每个 provider 卡片：header(name + builtin/custom badge) / api_key: NInput(password, show-on-click) + Save 按钮 / NEmpty 空状态
- **交互**：Save 按钮 loading=savingKey===g.provider（逐 provider 独立 loading）
- **UX 细节**：内置 provider 和自定义 provider 渲染路径不同（v-if 区分）但 UI 结构完全相同；badge 颜色：builtin=accent-primary, custom=success

#### PlatformSettings — PlatformCard 容器
- **元素**：可折叠 header：platform icon(SVG inline) + name + configured NTag(success/default) + 展开箭头 / body: exclusive NAlert warning + slot
- **交互**：点击 header toggle expanded；configured 状态由 credentials 检测驱动绿色 border
- **UX 细节**：expand-icon CSS transform rotate(-90deg) 折叠动画；exclusive 平台显示「Bot token 互斥」警告，提示同时只能绑一个 bot

#### PlatformSettings — Telegram
- **元素**：bot_token: NInput(password) / require_mention: NSwitch / reactions: NSwitch / free_response_chats: NInput(text, chat_id 逗号分隔) / mention_patterns: NInput(逗号分隔→string[])
- **交互**：字段变化写 draft，最终 Save 按钮统一提交
- **UX 细节**：mention_patterns 是数组但 UI 做逗号分隔字符串 join/split 处理，对用户透明

#### PlatformSettings — Discord
- **元素**：bot_token / require_mention / auto_thread: NSwitch / reactions / free_response_channels / allowed_channels / ignored_channels / no_thread_channels(均逗号分隔 ID)
- **交互**：同 Telegram 草稿模式
- **UX 细节**：auto_thread 控制是否自动在频道里开 thread 隔离对话

#### PlatformSettings — Slack
- **元素**：bot_token(xoxb-...) / require_mention / allow_bots: NSwitch / free_response_channels
- **交互**：同草稿模式
- **UX 细节**：allow_bots 控制是否响应其他 bot 消息，防无限循环

#### PlatformSettings — WhatsApp
- **元素**：enabled: NSwitch(credential 层) / require_mention / free_response_chats / mention_patterns
- **交互**：enabled 开关在 credentialDraft 层，不同于其他平台的 token
- **UX 细节**：WhatsApp enabled 用 switch 替代 token 输入，说明其连接方式不同（推测是本地 QR 扫码常驻）

#### PlatformSettings — Matrix
- **元素**：access_token(syt_...) / homeserver URL / require_mention / auto_thread / dm_mention_threads: NSwitch / free_response_rooms
- **交互**：homeserver 写到 credential.extra.homeserver
- **UX 细节**：dm_mention_threads 控制私聊是否也强制 @mention 才回复

#### PlatformSettings — Feishu/DingTalk/QQBot/WeCom
- **元素**：Feishu: app_id + app_secret + require_mention + free_response_chats / DingTalk: client_id + client_secret + card_template_id + allow_all_users + allowed_users + require_mention + free_response_chats / QQBot: app_id + client_secret + allowed_users + allow_all_users + markdown_support / WeCom: bot_id + secret
- **交互**：统一草稿保存；extra.* 字段用 spread 更新
- **UX 细节**：DingTalk 有 card_template_id（AI 卡片）和 allow_all_users/allowed_users 两层权限，QQBot markdown_support 从 configDraft.extra 读，保留嵌套结构

#### PlatformSettings — Weixin QR 登录
- **元素**：登录/重新登录按钮 / loading spinner(获取中) / 等待/已扫码 hint 文本 / token NInput / account_id NInput
- **交互**：点击 QR 登录→服务端获取 QR URL→新 tab 打开→3s 轮询状态→confirmed 自动保存 token
- **UX 细节**：5 种状态驱动 4 套 UI；expired/error 状态重置回「可再次登录」；token 输入框仍保留供手动填写

#### PrivacySettings
- **元素**：redact_pii: NSwitch
- **交互**：直接保存
- **UX 细节**：最小面板，单开关控制对话内容中 PII 自动脱敏

#### SessionSettings
- **元素**：require_auth: NSwitch(approvals.mode==='manual') / session_reset_mode: NSelect [both/idle/daily/none] / idle_minutes: NInputNumber [10..10080 step=30] / at_hour: NInputNumber [0..23 step=1] / live_monitor_human_only: NSwitch(localStorage)
- **交互**：require_auth 写 approvals section；session_reset 项防抖/直接保存；live_monitor_human_only 写 session-browser-prefs store(localStorage)
- **UX 细节**：live_monitor_human_only 是纯前端偏好用另一个 store 管理，和服务器设置混在同一面板但分不同 store 保存

#### UserManagementSettings
- **元素**：toolbar: 标题 + 描述 + 创建按钮 / NDataTable: username/role NTag/status NTag/profiles NTag 列表/last_login_at/操作(编辑/禁用或启用/删除+confirm) / 创建/编辑 Modal: username input + password input + role select + status select + profiles multi-select(filterable)
- **交互**：行内 status toggle(不需要 modal)；删除带 NPopconfirm；super_admin 不显示 profiles 字段(强制 allProfiles)
- **UX 细节**：createManagedUser vs updateManagedUser 共用 submit 函数；编辑时 password 为空则不修改；profiles 字段 v-if 隐藏于 super_admin

#### VoiceSettings — provider 选择器
- **元素**：TTS provider: NSelect [webspeech/openai/custom/edge/mimo](300px)
- **交互**：切换后展示对应 provider 配置块
- **UX 细节**：用 v-if template 块而非 v-show，切换时不保留 DOM，避免失焦/验证残留

#### VoiceSettings — WebSpeech 块
- **元素**：voice: NSelect(filterable, 320px, 从 SpeechSynthesis.getVoices() 动态加载)
- **交互**：onMounted + onvoiceschanged 异步加载系统 TTS 音色
- **UX 细节**：系统音色列表异步返回，用 onvoiceschanged 事件补全，避免空列表

#### VoiceSettings — OpenAI 块
- **元素**：api_key: NInput(password show-on-click) / base_url: NInput / model: NSelect [tts-1/tts-1-hd] / voice: NSelect [alloy/echo/fable/nova/onyx/shimmer]
- **交互**：全部 localStorage 自动持久化，无 HTTP 发送
- **UX 细节**：base_url 允许自定义兼容 OpenAI API 的代理

#### VoiceSettings — Custom Endpoint 块
- **元素**：provider_hint 文本 / url: NInput / api_key: NInput(password)
- **交互**：同 localStorage 持久化
- **UX 细节**：custom 实质上是 OpenAI-compatible 接口，hint 说明这一点

#### VoiceSettings — Edge TTS 块
- **元素**：voice: NSelect(filterable, 25 个预置中英日韩法德音色) / rate: NSlider [0.5..2.0 step=0.05] + 实时文字显示(speed→Edge rate string) / pitch: NSlider [-20..20 Hz step=1] + 实时文字显示(Hz→Edge pitch string)
- **交互**：Slider 拖动实时换算显示 Edge 格式字符串(如 '+20Hz')
- **UX 细节**：speedToEdgeRate / hzToEdgePitch 工具函数在 ttsHelpers.ts，换算逻辑与 UI 解耦；rate 显示 '1.20x (+10%)' 格式直观

#### VoiceSettings — MiMo TTS 块
- **元素**：api_key / base_url: NSelect(filterable tag 可自输入, 2个预置 URL) / model: NSelect [mimo-v2.5-tts/mimo-v2.5-tts-voicedesign] / 预置音色 voice(model=preset 时显示) / voice_design_desc textarea(model=voicedesign 时显示) / style_prompt textarea(所有 model 显示)
- **交互**：model 切换动态显示 voice 或 voice_design_desc；base_url 支持 tag 模式自定义
- **UX 细节**：voice design 模式用自然语言描述音色（如'25岁女声，活泼'），与预置音色互斥展示

#### VoiceSettings — 试听区
- **元素**：test_text: NInput(360px, enter触发) / 试听按钮(loading时显示'播放中')
- **交互**：enter 键触发 handleTest；testPlaying=true 时两个控件均 disabled
- **UX 细节**：试听与 provider 配置联动，不需要切换页面；统一入口消除 5 种 provider 的试听差异

### files + usage + layout + auth + common 组件

#### 文件树面板 (FileTree)
- **元素**：顶部 Home 图标+根目录标签（可点击回根）；NTree 懒加载树（仅显示目录节点，expand-on-click）；selectedKeys 高亮当前目录
- **交互**：点击目录节点：展开并 navigateTo；点击根标签：navigateTo('')；树节点展开时异步 loadChildren
- **UX 细节**：只显示目录，目录选择和文件列表展示分离，避免树和列表信息重复

#### 文件面包屑 (FileBreadcrumb)
- **元素**：NBreadcrumb + NBreadcrumbItem，第一项为根，后续为 pathSegments 各级
- **交互**：点击任意层级直接跳转到该路径；点击根跳转到 ''
- **UX 细节**：pathSegments 由 currentPath.split('/').filter(Boolean) 派生，面包屑与树的选中状态保持同步

#### 文件列表 (FileList)
- **元素**：列头（名称/大小/修改时间，均可点击排序，带 ↑↓ 指示符）；每行：emoji 图标 + 文件名 + 大小 + 修改时间 + 操作按钮（预览👁/编辑✏️/下载⬇️，hover 才可见）；空目录 NEmpty；加载中 NSpin
- **交互**：双击目录→navigateTo；双击文本文件→openEditor；双击可预览文件→openPreview；右键→触发 FileContextMenu.show；行内按钮单独操作
- **UX 细节**：操作按钮 opacity:0，hover 时 opacity:1（CSS transition），不占空间；移动端隐藏大小和时间列

#### 右键菜单 (FileContextMenu)
- **元素**：NDropdown（manual trigger）：目录→open；文本文件→edit/preview/download；通用→copyPath/rename/delete；分隔线
- **交互**：show(e, entry) 命令式触发，nextTick 重置坐标防残留；点击外部关闭；delete 弹 dialog.warning 二次确认
- **UX 细节**：nextTick 先设 showMenu=false 再 true，强制 NDropdown 重算位置，解决 context menu 跟随光标位置不更新的问题

#### Monaco 文件编辑器 (FileEditor)
- **元素**：顶部 header：路径展示（text-overflow ellipsis）+ 保存按钮（loading 态）+ 关闭按钮；编辑区：Monaco Editor 实例（flex: 1）
- **交互**：Ctrl/Cmd+S 保存；关闭时有未保存改动则弹 dialog.warning 确认；onDidChangeModelContent 实时同步 store.editingFile.content
- **UX 细节**：Monaco worker 用 import.meta.url 构造（ESM，Vite 友好）；theme 跟随页面 dark class；minimap 关闭；automaticLayout=true 响应容器尺寸变化；onBeforeUnmount dispose() 防内存泄漏

#### 文件预览 (FilePreview)
- **元素**：顶部 header：路径 + 关闭按钮；预览区：图片→<img>；markdown→MarkdownRenderer；text→renderHighlightedCodeBlock（含复制按钮）
- **交互**：点击代码块复制按钮：handleCodeBlockCopyClick；关闭：filesStore.closePreview()
- **UX 细节**：图片直接用 getFileDownloadUrl 生成带 token 的 URL 作为 src，无需先 fetch；markdown 和 text 需先读内容；maxHighlightLength=200000 防止 Prism/Highlight 大文件卡主线程

#### 文件工具栏 (FileToolbar)
- **元素**：新建文件 / 新建文件夹 / 上传 / 刷新 四个 NButton（size=small）
- **交互**：前三个 emit 事件给父组件；刷新调 filesStore.fetchEntries()
- **UX 细节**：移动端按钮缩小 padding + font-size，不换行显示

#### 新建/重命名模态 (FileRenameModal)
- **元素**：NModal preset=dialog；NInput（autofocus，Enter 提交）；取消/确认按钮
- **交互**：mode 属性决定标题/placeholder/操作（newFile/newFolder/rename 三合一）；show 变化时 watch 重置或填充 inputValue；submitting 防重复提交
- **UX 细节**：三种场景复用同一组件，通过 mode prop 区分，减少重复代码且行为一致

#### 上传模态 (FileUploadModal)
- **元素**：NModal；NUpload（multiple + directory-dnd + default-upload=false）；拖放区文字提示；上传按钮（显示已选文件数）
- **交互**：handleFileChange 收集 File 对象；handleUpload 批量上传；关闭时清空 fileList
- **UX 细节**：directory-dnd 支持整个文件夹拖入；按钮文字'Upload (N)'让用户确认数量

#### 用量统计卡片 (StatCards)
- **元素**：4 列网格（移动端 2 列/1 列）：总 token（含 input/output 副标题）/ 总会话数（含日均）/ 预估费用 / 缓存命中率（含缓存 token 数）
- **交互**：无交互，纯展示
- **UX 细节**：formatCost '<$0.01' 防止显示 $0.00 误导用户（实际有消耗但不足 1 分钱）

#### 每日用量趋势图 (DailyTrend)
- **元素**：section 标题；堆叠柱状图（input/output/cache 三色段，CSS height 百分比动画）；日期轴（仅首尾两日）；图例；明细表格（日期/各 token/缓存命中率/会话数/费用）
- **交互**：hover 柱子显示 absolute 定位 tooltip（CSS ::after 箭头）
- **UX 细节**：纯 CSS 实现图表（无 echarts/chart.js），transition: height 0.3s；tooltip 用 CSS hover display:block 无 JS 状态开销；日期只显示 MM-DD（.slice(5)）节省空间

#### 模型用量分解 (ModelBreakdown)
- **元素**：section 标题；图例；每个 model 一行：颜色点 + model 名（monospace 截断）+ 水平堆叠进度条（input/output/cache 三色）+ token 数字（cache token 橙色小字）
- **交互**：无交互，tooltip 用 title 属性
- **UX 细节**：model 颜色由 getModelColor 字符串哈希确定，同 model 在 StatCards/DailyTrend/ModelBreakdown 颜色一致；进度条 min-width:2px 防止极小值不可见

#### 侧边栏 (AppSidebar)
- **元素**：Logo 区（图片+文字）；折叠按钮（绝对定位叠在 Logo 右上角）；5 个导航分组（Conversation/Agent/Monitoring/Tools/System，每组可折叠）；ProfileSelector；ModelSelector；底部：登出按钮 / WS 连接状态点 / LanguageSwitch / 版本号（点击 changelog）/ ThemeSwitch / 更新提示按钮
- **交互**：折叠侧边栏：宽度 240px→约 60px，文字消失，ModelSelector 隐藏，ProfileSelector 只显示头像；分组 label 点击折叠/展开（持久化）；版本号点击弹 changelog modal；updateAvailable 时显示一键更新按钮
- **UX 细节**：折叠态通过 CSS :deep 精确控制子组件样式（.model-selector display:none / .profile-selector justify-content:center）；nav-item &.active 用 accent-primary 色+浅背景区分；logout hover 用 error 色警示；移动端侧边栏 position:fixed + translateX(-100%) 滑入

#### 模型选择器 (ModelSelector)
- **元素**：触发按钮（当前模型名+箭头，border-radius input 风格）；搜索输入框；分组列表（可折叠，显示 count）；每个 model 行：名称+别名/canonical+preview/disabled/custom 标签+删除按钮+选中对号；自定义模型区（provider 下拉+输入框+提示文字）
- **交互**：搜索实时过滤 model 名和 displayName；点击分组 header 折叠；点击 model item 切换（disabled 的 pointer-events 阻止）；自定义 model Enter 提交；× 按钮删除自定义 model
- **UX 细节**：safeLower 防御 null 导致的 toLowerCase crash；disabled model cursor:not-allowed + opacity:0.45；自定义 model 有橙色 custom badge 和 × 删除；disabled 双重防护（列表点击 + 输入框提交都检查）

#### Profile 选择器 (ProfileSelector)
- **元素**：触发区（头像+profile 名，border 卡片风格）；Profile 管理 Modal（720px，列出所有 profile）；每个 profile 行：头像+名称+active badge+运行状态（bridge/gateway 状态点+文字）+诊断信息+4 个操作按钮（定制头像/重启 gateway/重启 profile/切换）；头像编辑 Modal（上传/随机/重置）
- **交互**：点击触发区打开 modal + 触发 loadRuntimeStatuses；refreshing=true 时启动退避轮询；切换 profile 后 window.location.reload()；头像上传校验 mime+size；Random 生成 seed 传后端
- **UX 细节**：active-badge 用 color-mix(in srgb, $success 16%, transparent) 实现半透明绿色背景；runtime-dot 仅 running 时显示 $success + box-shadow 发光；modal 内容 max-height:420px 防 profile 过多超屏

#### 主题切换 (ThemeSwitch)
- **元素**：两个 icon button（28×28px）：亮/暗切换（太阳/月亮图标随状态变化）；风格切换（ink/comic，调色盘/星形图标随状态变化）
- **交互**：toggleBrightness：light→dark 或 dark→light；toggleStyle：ink↔comic
- **UX 细节**：两个正交维度（brightness × style）独立切换，组合出 4 种主题状态（light/dark × ink/comic）

#### 语言切换 (LanguageSwitch)
- **元素**：NSelect（tiny size，10 种语言选项，consistent-menu-width=false）
- **交互**：change 调 switchLocale + localStorage.setItem
- **UX 细节**：consistent-menu-width=false 让下拉菜单宽度自适应最长选项，避免 tiny trigger 下下拉太窄

#### 认证事件监听器 (AuthEventListener)
- **元素**：display:none span，无 UI
- **交互**：监听 window 'hermes-auth-notice' CustomEvent；1200ms 防抖；forbidden→accessDenied toast；其他→sessionExpired toast
- **UX 细节**：1200ms 防抖避免批量 401 爆 toast；纯事件驱动无 store 依赖，解耦 API 层与 UI 层

#### 默认密码提醒 (DefaultCredentialPrompt)
- **元素**：NModal（mask-closable=false）：警告文本 + 稍后提醒按钮 + 去账户设置按钮
- **交互**：route 变化时 watch 触发检查；稍后提醒写 sessionStorage；去设置按钮跳转 hermes.settings?tab=account
- **UX 细节**：mask-closable=false 强制用户做选择；sessionStorage（非 localStorage）保证下次打开浏览器重新提醒；desktop shell 检测跳过（window.hermesDesktop.isDesktop）

### views 全部(22) — hermes-web-ui

#### LoginView
- **元素**：logo 图片、产品标题 h1、描述文本、默认凭据提示(code 字体)、username 输入框(autofocus)、password 输入框(keyup.enter 触发登录)、错误提示 div、lockResetHint 警告框(含 CLI 命令 code 块)、Submit 按钮(loading 禁用态)
- **交互**：Enter 键提交、429/503 时显示解锁提示 CLI 命令
- **UX 细节**：lockResetHint 显示两条具体 CLI 命令，developer-first 设计；默认凭据提示用 code 字体

#### ChannelsView(PlatformSettings)
- **元素**：页面 header + 标题、NSpin 加载遮罩、PlatformSettings 组件(包含 provider 渠道配置)
- **交互**：自动加载 profile 后展示设置
- **UX 细节**：loading/saving 双态共用同一 NSpin

#### ChatView
- **元素**：全屏 ChatPanel 组件(包含消息列表/composer/会话侧栏)
- **交互**：路由 sessionId 变化时切换会话；document.title 跟随活跃会话标题
- **UX 细节**：tab title 实时更新，unmounted 时重置，防止离开后标签页标题残留

#### CodingAgentsView
- **元素**：header(标题+刷新按钮)、错误 Alert、说明文字、agent-blocks 2列网格(Claude Code/Codex各一块)、每块内：logo+名称+provider tag、安装状态(checking/installed/not-installed tag + 版本号)、安装/删除按钮、配置文件标签页(~/.claude/settings.json 等)、内联 config textarea(300px高)、Save/Launch 按钮；Launch 弹窗(profile tag、global/scoped radio、provider 下拉、model 下拉、protocol 下拉、内置终端/原生终端两按钮)；右滑抽屉终端面板(TerminalPanel)
- **交互**：配置文件标签切换加载对应文件内容；Save 按钮只在有未保存变更时可点；Launch 弹窗中 provider 变更自动推断默认 apiMode；抽屉点遮罩关闭
- **UX 细节**：config textarea 固定 300px 使用 monospace font；内置/原生终端双路启动，分别 loading 互斥；node_environment_missing 错误码有专用提示文案

#### FilesView
- **元素**：左侧 240px FileTree 面板(目录树)、主面板：FileToolbar(新建文件/文件夹/上传按钮)、FileBreadcrumb 面包屑、内容区域(FileEditor/FilePreview/FileList 互斥展示)、FileContextMenu(右键菜单)、FileUploadModal、FileRenameModal
- **交互**：右键 FileList 条目触发 contextmenu；新建/重命名共用 FileRenameModal 但 mode 不同
- **UX 细节**：mobile 下 FileTree 变为顶部 200px 水平面板

#### GroupChatView
- **元素**：GroupChatPanel(包含房间列表侧栏+消息区+发送框)
- **交互**：onMounted connect WS 并 loadRooms；路由 roomId 变化时 joinRoom；onUnmounted disconnect
- **UX 细节**：WS 生命周期与 view 生命周期严格绑定

#### HistoryView
- **元素**：session-backdrop(mobile)、左侧 220px 会话列表(可折叠)：header(标题+批量模式切换+全选+批量删除 NPopconfirm)、scope 提示横幅、pinned 区、按 source 分组可折叠列表(SessionListItem)；右侧主区：header(展开列表按钮+会话标题+source badge+workspace badge+轮廓/复制ID按钮)、HistoryMessageList+OutlinePanel；NDropdown 右键菜单(import/pin/copy-link/copy-id)
- **交互**：右键菜单：导入/置顶/复制链接/复制 ID；批量模式下 checkbox 多选+批量删除；组头点击折叠/展开并自动选首个 session；mobile 侧边栏 overlay 模式；outline panel 点击锚点跳转
- **UX 细节**：置顶 session 独立前置区；source-badge 和 workspace-badge 在 header 中辅助定位上下文；分组折叠状态持久化 localStorage

#### JobsView
- **元素**：header(标题+创建按钮)、上下分割布局：上半部 JobsPanel(任务列表)、分割线、下半部 JobRunHistory(历史运行记录)；JobFormModal(创建/编辑弹窗)
- **交互**：点选 job 高亮并过滤 JobRunHistory；再次点击取消选中；profile 切换时清空重载
- **UX 细节**：上下两个 flex:1 区域使用 splitter 分割线视觉区分

#### KanbanView
- **元素**：header：标题、看板选择下拉(260px，显示看板名+任务数)、添加按钮、归档按钮、状态过滤下拉、负责人过滤下拉、创建任务按钮；stats-bar：triage/todo/ready/running/blocked/done/archived+total 状态芯片(彩色左边框)；看板主体：NCollapse 折叠列表(每个状态一组，列头含状态点+计数)、KanbanTaskCard 列表；KanbanTaskDrawer 详情抽屉；创建看板弹窗(slug+name输入)；KanbanCreateForm
- **交互**：状态芯片点击过滤并收起其他组；路由 query.board 双向同步；15s 轮询+SSE 实时刷新；看板切换时 refreshAll
- **UX 细节**：status-color CSS 变量实现每个状态的一致色系(统计芯片/列头圆点/左边框/任务卡背景)；status-dot 有 box-shadow 光晕效果

#### LogsView
- **元素**：header：标题、日志文件选择下拉、日志级别过滤下拉、行数选择下拉(50/100/200/500)、搜索框(160px)、刷新按钮；日志列表：每条 log-entry 含 time/level badge/logger名/message；access log 条目特殊渲染 method(粗体)+path(蓝色)+status code(颜色对应2xx/3xx/4xx)
- **交互**：所有下拉变化立即 loadLogs；搜索框实时过滤(客户端)；level badge 颜色：ERROR红/WARNING橙/DEBUG和INFO灰
- **UX 细节**：log-entry 左边框颜色区分 ERROR(红)/WARNING(橙)；access log 专用 parseAccessLog 提取字段分别样式

#### McpManagerView
- **元素**：header(标题+刷新)；summary-grid：total/connected/disconnected/tools 4张统计卡；toolbar：搜索框+Reload All按钮+添加按钮；server 卡片网格(McpServerCard)；添加/编辑弹窗：JSON/YAML 切换 radio + textarea + 错误提示 + 保存按钮；工具可见性弹窗：server名+Fetch Tools按钮+all/include/exclude 三模式 radio + 工具 checkbox 列表(NScrollbar) + 统计摘要
- **交互**：JSON/YAML 切换自动转换内容；输入实时校验+1500ms自动格式化；指数退避自动重试(5次)；enable toggle 调 reload；edit 恢复原有配置内容
- **UX 细节**：工具可见性弹窗的 all 模式下 checkbox disabled，include/exclude 分别选「包含」和「排除」的工具

#### MemoryView
- **元素**：header(标题+刷新)；三列等宽 memory-section：My Notes / User Profile / Soul；每节：标题行(图标+名称+修改时间)、编辑按钮；查看模式：MarkdownRenderer 渲染或 empty 斜体提示；编辑模式：填满高度 textarea + Cancel/Save 按钮
- **交互**：每次只能编辑一个 section(editingSection 单例)；Cancel 直接复原；Save 成功后重新加载所有数据
- **UX 细节**：三列 flex 布局，mobile 变 flex-direction:column；§ 字符被替换为双换行保持格式化

#### ModelsView
- **元素**：全屏覆盖 overlay spin(刷新缓存时)；header：标题、Refresh Model Cache按钮、添加 Provider 按钮；内容区：AuxiliaryModelsPanel(辅助模型配置) + ProvidersPanel(供应商列表)；ProviderFormModal(创建弹窗)
- **交互**：刷新缓存时整页 backdrop-blur overlay；Copilot token 检查静默失败不阻断
- **UX 细节**：overlay 用 color-mix(in srgb, bg 78%, transparent) + backdrop-filter:blur(2px) 实现毛玻璃遮罩

#### PerformanceView
- **元素**：header：标题、Auto Refresh 切换按钮(primary/default 两态)、手动刷新按钮；summary-grid 4格：系统CPU/内存(含进度条meter)+活跃会话数+worker数/总内存；进程表格：WebUI + Bridge Broker 各行(CPU/内存/运行状态 pill)；worker 表格：profile/PID/CPU/Memory/Running Sessions/最后使用/状态；sessions by profile 列表
- **交互**：Auto Refresh 5s 轮询(onBeforeUnmount 清理)；手动刷新 showError=true；自动刷新 showError=false 静默
- **UX 细节**：状态 pill(running/stopped)用 border-radius:999px 胶囊样式；进度条 meter 用 $accent-primary fill

#### PluginsView
- **元素**：header(标题+刷新)；info/error/warning 多条 Alert；summary-grid 5格：total/active/inactive/disabled/provider-managed；filter-row：搜索框+source/kind/status 三下拉；插件表格：plugin名/描述/版本作者 + status tag + configStatus小字 + source tag + kind tag + capabilities(tools/hooks/env数) + path(code胶囊) + CLI 命令复制按钮；元数据面板(agentRoot/python/cwd/projectPlugins)
- **交互**：source/kind/status 联合过滤；复制 hermes plugins enable/disable 命令；profile 切换重载
- **UX 细节**：path 用 code 胶囊+text-overflow:ellipsis 限宽显示；provider-managed 状态不显示 CLI 命令

#### ProfilesView
- **元素**：header(标题+Import按钮+Create按钮)；ProfilesPanel(profile列表)；ProfileCreateModal/ProfileRenameModal/ProfileImportModal
- **交互**：Create/Import 按钮打开对应弹窗；panel 内部触发 rename 事件冒泡到 view
- **UX 细节**：三弹窗 v-if 条件渲染(不预创建 DOM)

#### SettingsView
- **元素**：header(标题)；NTabs 横向标签：account/users(仅 super admin)/display/agent/memory/compression/session/privacy/models/voice 各对应独立设置 component；NSpin 遮罩
- **交互**：tab 切换同步 router.replace query.tab；非法 query 值 normalizeTab 回退 account；super admin 才可见 users tab
- **UX 细节**：tab 状态持久化于 URL，可直链特定设置页

#### SkillsUsageView
- **元素**：header：标题+副标题、7d/30d/90d/365d 周期切换按钮组、刷新按钮；概览网格：左侧堆叠条形图(每日)+右侧4张统计卡；悬浮 tooltip(日期+各 skill 数量+总计)；top skills 表格：颜色点+名称/loads/edits/占比进度条/最后使用时间；空状态占位
- **交互**：周期按钮切换复用缓存数据；条形图悬停显示 tooltip；tooltip 左右对齐自适应防溢出；列名 mouseenter/focusin 双事件触发 tooltip
- **UX 细节**：is-refreshing 时右上角脉冲圆点动画提示后台刷新；条形图 transition:height 0.2s 平滑动画；share-bar 行内进度条

#### SkillsView
- **元素**：header：标题、sidebar toggle(mobile)、source 图例过滤按钮(builtin/hub/local/external/modified 带彩点)、搜索框(160px clearable)；左侧 280px sidebar：SkillList(分类+技能列表)；主内容：SkillDetail(选中技能详情) 或 recommendations-panel(MarkdownRenderer 渲染推荐文档，空时显示图标占位)
- **交互**：source 图例按钮 toggle 过滤；mobile backdrop 点击收起 sidebar；同一技能再次点击取消选中；mobile 选中后自动收起 sidebar
- **UX 细节**：推荐文档按 locale(zh/en) 动态加载不同 Markdown 文件；legend-dot 颜色：builtin灰/hub蓝/local绿/external橙

#### TerminalView
- **元素**：左侧 220px session list(可折叠)：session-list-header(标题+新建按钮)、session 列表(shell badge+时间/exited状态+删除 NPopconfirm)；主区：header(展开列表按钮+session标题+主题选择下拉130px+新建标签按钮)、xterm.js 终端容器(圆角边框)
- **交互**：WS 二进制/JSON 混合协议：以'{' 开头为 JSON 控制帧，其余直接写 terminal；resize observer 触发 fit+sendResize；touchmove 手动实现行级滚动；主题实时应用到所有 terminal 实例；session close 自动切换到第一个或创建新 session
- **UX 细节**：13种内置主题(Default/Solarized/Monokai/Dracula/Nord/OneDark/GitHub/TokyoNight/Catppuccin/Alabaster/GruvBox等)；xterm-viewport scrollbar 在 mobile 恢复显示；termMap 保持 Terminal 实例不销毁以保留 scrollback buffer；触摸滚动 18px/行

#### UsageView
- **元素**：header：标题、周期选择按钮组(7d/30d/90d/365d，pill 形 border+bg 容器)、刷新按钮；内容：StatCards+ModelBreakdown+DailyTrend 三组件；空/加载态占位
- **交互**：周期按钮更新 selectedPeriod 并 loadUsage；刷新复用当前 period
- **UX 细节**：period-selector 容器加 border+bg-secondary 实现按钮组视觉；内容区 max-width:960px 居中，scrollbar 隐藏

#### VersionPreviewView
- **元素**：header(标题)、GithubPreviewSettings 组件(GitHub 预览版功能订阅/测试设置)
- **交互**：纯展示，无 view 级交互逻辑
- **UX 细节**：最简 view 模式：0行 script 逻辑，只需 i18n 标题

### 前端架构(状态/路由/api/i18n/composables)

#### App.vue 根布局
- **元素**：hamburger 按钮（logo 图片）、mobile backdrop 遮罩、AppSidebar、<router-view>、SessionSearchModal（全局悬浮）、DefaultCredentialPrompt（全局悬浮）、nodeVersionLow 警告条
- **交互**：hamburger 点击 toggle sidebar；backdrop 点击关 sidebar；路由变化自动 closeSidebar（mobile）
- **UX 细节**：v-if='ready' 等 router.isReady() 避免闪路由；naiveTheme 和 themeOverrides 均为 computed，响应 isDark/isComic 实时变化；node 版本低于 23 时顶部黄色警告条

#### 路由页面集（views/hermes/）
- **元素**：ChatView/HistoryView（共用 sessionId param）、GroupChatView（roomId param）、JobsView、KanbanView、ModelsView、ProfilesView（requiresSuperAdmin）、UsageView、SettingsView、FilesView、TerminalView、McpManagerView（requiresSuperAdmin）、PerformanceView（requiresSuperAdmin）、SkillsView/SkillsUsageView、MemoryView、PluginsView、VersionPreviewView（requiresSuperAdmin）、CodingAgentsView、ChannelsView、LogsView
- **交互**：全部懒加载（import()），路由守卫控制 public/auth/superAdmin 三级权限
- **UX 细节**：hash 路由避免 server 配置；session/:sessionId 和 history/session/:sessionId 复用同一 View 组件，由 props 区分

#### SessionSearchModal（Ctrl+K）
- **元素**：搜索输入框、会话列表、高亮匹配文本
- **交互**：Ctrl/Meta+K 打开；Esc 关闭；useSessionSearch singleton 跨组件共享 open 状态
- **UX 细节**：模态状态用模块级单例 ref 管理，避免事件总线或 vuex action

#### 主题切换（useTheme + ThemeSwitch）
- **元素**：brightness 三段（light/dark/system）、style 切换（ink/comic）
- **交互**：toggleBrightness/toggleStyle；系统偏好变化时若当前是 system 模式自动响应
- **UX 细节**：实时 documentElement.classList toggle，naive-ui NConfigProvider themeOverrides 同步响应；comic 模式替换整套字体为手写字体族

#### LanguageSwitch
- **元素**：语言选择器（10 种语言）
- **交互**：切换调用 switchLocale，更新 i18n.global.locale + html lang 属性
- **UX 细节**：无刷新切换，locale 存 localStorage，下次打开自动恢复

#### AppSidebar（layout/AppSidebar.vue）
- **元素**：导航菜单项（图标+文字）、desktop 折叠按钮（icon-rail 模式）、ProfileSelector、ModelSelector
- **交互**：sidebarCollapsed toggle 存 localStorage；mobile 模式下 sidebarOpen 控制显示/隐藏
- **UX 细节**：desktop 折叠仅 icon 模式（节省空间）；mobile 有 backdrop 遮罩点击关闭

#### 聊天界面（ChatView + ChatPanel + ChatInput）
- **元素**：会话列表（左侧，含 pin/filter）、消息列表（VirtualMessageList/MessageList）、工具调用行（toolName/toolStatus/toolDuration）、assistant 推理气泡（reasoning 折叠）、流式光标（isStreaming）、attachment 预览、pendingApproval banner、pendingClarify banner、压缩状态 banner、abort 按钮、消息优化魔棒（composer）
- **交互**：Ctrl+N 新会话；tool 行可展开 toolArgs/toolResult；approval 行点击 once/session/always/deny；speech 播放按钮（per-message）；图片路径点击预览（lightbox）；代码块沙箱预览（html/svg/mermaid）
- **UX 细节**：queued 消息显示不同样式（waiting 徽章）；swallowedError 检测：run 完成后 text+tool 均为空则显示错误提示

#### UsageView（usage 统计）
- **元素**：StatCards（总 token/session/成本/cache 命中率）、DailyTrend（堆叠条形图）、ModelBreakdown（模型用量分布）
- **交互**：天数筛选（7/14/30/90 天）；模型颜色按 hash 稳定分配
- **UX 细节**：cacheHitRate computed 带 null 保护；estimatedCost 精确显示美元；avgSessionsPerDay 只统计有活动的天

#### KanbanView
- **元素**：board 选择器（多 board 支持）、列（todo/in_progress/done/blocked）、任务卡（KanbanTaskCard）、任务详情 Drawer、过滤器（status/assignee）、dispatch 按钮
- **交互**：board 切换清空 scoped state + boardGeneration++；WebSocket event stream 实时更新；drag-drop 移动任务（HTML5 drag API）
- **UX 细节**：capability 动态显示/隐藏功能按钮（不支持的后端版本不崩溃）；boardWarning 提示 fallback 到 default board

#### JobsView（cron 定时任务）
- **元素**：任务列表（name/schedule/next_run/status）、JobFormModal（新建/编辑）、JobRunHistory（运行历史）、pause/resume/run-now 按钮
- **交互**：schedule 支持 cron 表达式/interval/once 三种格式；scheduleToEditableInput 统一转为可编辑字符串回填表单
- **UX 细节**：buildJobUpdateRequest 生成 diff-only 的 PATCH body，不发送未修改字段

## 4. swarmx 借鉴小结（本 repo）

| 优先级 | 借鉴点 | 价值 | 工作量 | 落到 swarmx 哪里 |
|---|---|---|---|---|
| P0 | VirtualMessageList 虚拟滚动引擎(含置底/锚点/翻页保位/切会话保位) | swarmx MessagesPanel 随 worker 活跃可能积累数百条消息；无虚拟化会卡死 DOM。置底/上拉翻页/锚点跳转三套算法可直接在 Rust 后端 WS 推送的消息列表上复用。 | 中：需要引入 vue-virtual-scroller 或等价 Rust 侧预计算高度方案(若前端是 Vanilla JS)；核心算法可照搬 | MessagesPanel.vue / SwarmPanel 消息列表。swarmx 前端为轻量原生 JS，需要自行实现 DynamicScroller 等价逻辑或引入库；captureScrollPosition 翻页保位可直接对应 loadOlderMessages WS 事件。 |
| P0 | 工具调用多维度截断(truncateJsonValue)：深度/节点数/键数/数组项/字符串长度六维限制 | swarmx worker 读 JSONL 时 tool_result 可能是数 MB 的 JSON；直接渲染会卡死浏览器。六维截断是防御性最强的成品算法，可零修改搬入 swarmx ChatMarkdown/AgentActivity 的工具结果展示。 | 低：TypeScript 纯函数，直接复制即用 | packages/client/src/components 的工具结果渲染处(目前 AgentActivity 展示工具调用 payload)。参数常量(TOOL_PAYLOAD_DISPLAY_LIMIT=1000, JSON_MAX_DEPTH=6 等)可按 swarmx 场景调整。 |
| P0 | requestId/requestSeq 版本号防竞态模式 | swarmx 切方向(thread)/切 workspace 时多个 WS fetch 并发，最后一个返回可能覆盖最新 UI 状态。统一用版本号防竞态是最低成本的正确解。 | 低：3 行样板，任何异步数据加载处都可加 | swarmx 所有「切 workspace/方向/agent 触发的 API 请求」：loadMessages、loadWorkspaces、loadBlackboard 等。 |
| P0 | markdownFenceRepair：剥 LLM ```md 外层围栏 | swarmx ChatMarkdown 已踩过 Tailwind v4 灭 list-style 坑；围栏修复是同类渲染正确性问题，LLM 输出 PR draft/架构方案时很常见。 | 低：217 行纯字符串处理，无依赖，直接复制 | ChatMarkdown.vue 的 markdown 预处理步骤，在 markdown-it render 之前调 repairNestedMarkdownFences(content)。 |
| P0 | CLI Worker 认证走 Device Flow UI（Codex/Copilot 式 user_code + polling） | swarmx spawn codex worker 目前靠 PTY 手动输入 token，用户体验差。引入 Device Flow modal：展示 user_code 大号可复制 + 打开链接按钮 + 自动 3s 轮询后端 /poll 端点，和 hermes 一样的 5 态状态机。后端在 PTY 里检测 codex 登录态，并通过 API 暴露给前端。 | 后端：spawn 前 check codex auth 状态并暴露 /api/auth/codex/start + /poll；前端：复用 hermes 的 CodexLoginModal 结构（Vue→原生 JS/HTML），约 3-5 天 | 落在 routes/auth.rs（新增端点）+ 前端 settings 页或 workspace 初始化流程；不需要 OAuth SDK，后端检测 ~/.codex/credentials 或 PTY 输出 |
| P0 | Auxiliary Models（辅助任务模型分配）对应 swarmx 的 per-task role 模型配给 | swarmx 已有角色注册表（role → default_cli + model tier），但 F1 的 per-task model 配给还缺 UI。hermes 的 AuxiliaryModelsPanel 模式可以搬：task_key 表格 + per-task provider/model/timeout 编辑。对应 swarmx 的 role_registry.toml 里每个 role 的 model tier override。 | 后端：roles_config 暴露为 GET /api/roles + PATCH /api/roles/{role}；前端：按 hermes AuxiliaryModelsPanel 的 4 列表格 + 编辑 modal，约 2-3 天 | 落在 src-tauri/routes/roles.rs（或现有 models_config.rs）+ 前端设置页「模型配给」tab；表格结构比卡片更紧凑，适合 10+ 角色并排对比 |
| P0 | SettingRow 原子布局组件 | 统一设置页所有行的 label/hint 左+control 右布局，响应式折叠集中于一处；swarmx 现在各设置行散布 flex 代码，加新设置项需要各自写样式 | 0.5天：新建一个原生 HTML 组件，约 50 行 CSS+HTML | 前端 settings 页面，直接在现有设置页各面板统一采用；无 Rust 后端改动 |
| P0 | 两阶段写入：乐观 updateLocal + 防抖 saveSection(300ms) | 数字输入框每次击键立即反映 UI，300ms 后才发 HTTP；Switch/Select 直接保存。现在 swarmx 设置页 onChange 直接发 fetch，数字输入特别卡 | 1天：给所有数字输入框加 debounce wrapper，store 加 updateLocal 方法 | 前端 settings store + 各设置面板组件；后端无需改动 |
| P0 | 可折叠卡片 + configured 状态检测（搬 PlatformCard 模式） | swarmx MCP 管理页每个 MCP server 配置可以做成可折叠卡片，检查 api_key/path/url 任意一个非空即标 configured；已配绿色 border 区分 | 1天：抽 CollapsibleCard 组件，configured computed 检查 3~5 个字段 | 前端 MCP 管理页(routes/mcp_admin.rs 已有数据)；configured 状态 badge 与现有 per-CLI 开关并列展示 |
| P0 | Usage/Cost 可观测面板（StatCards + DailyTrend + ModelBreakdown） | swarmx 现在完全没有 token 用量可视化。hermes 的三组件给出完整参考：StatCards（总量/费用/缓存命中率）、每日堆叠柱状图（纯 CSS，零依赖）、按模型水平进度条。对程序员受众来说这是刚需——知道钱花在哪个 agent/模型上 | 中（后端需要读 claude/codex 会话 JSONL 汇总 token 数据，已有 transcript.rs 基础；前端搬 hermes 三组件框架，把 model 维度改为 agent_id + cli_type + model） | 新增 /api/usage?days=N 端点（axum handler），读 SQLite sessions 表聚合；前端在现有设置页或新增 /usage 路由挂三个组件。getModelColor 哈希函数直接复用。token 数据来源：claude 会话 JSONL 有 usage 字段（input/output/cache），codex 来自 SSE stream 末尾的 usage event |
| P0 | Usage/Cost 可观测页：token 消耗 + 费用，按日趋势图 + 模型分布 | swarmx 现在对 claude/codex 每次会话的 token 用量完全不可见；有了这个页面可以帮用户控制成本，也是 MEMORY 记录的 P0 缺口 | 中：前端参考 UsageView+StatCards+DailyTrend+ModelBreakdown 四组件；后端需要从 JSONL transcript 中提取 usage 字段并聚合到 SQLite，已有 transcript.rs 作为基础 | 新增 GET /api/usage?days=N 接口(按 agent_id/workspace 聚合)；前端参考 hermes 的 UsageView 布局(period 按钮组+三组件)，复用 DailyTrend 的堆叠条形图思路用纯 CSS 实现，不引入图表库 |
| P0 | Kanban 任务面板：triage/todo/ready/running/blocked/done/archived 七状态，支持多看板，SSE 实时推送 | swarmx 的编排任务当前只在黑板(共享区/台账)中以文本形式存在，缺乏结构化任务追踪和可视化；Kanban 是把台账升级为人机共写控制平面的关键 | 大：需要后端 SQLite kanban_tasks 表、REST CRUD + SSE 推送、前端完整 KanbanView；但 hermes 代码可直接参考移植逻辑 | 复用 KanbanView 的状态芯片 filter + NCollapse 折叠列看板布局；后端用 axum + tokio broadcast 实现 SSE；任务 assignee 对应 swarmx 的 agent_id；看板 slug 对应 workspace_id |
| P0 | 内置 xterm.js 终端(多 session，WS 驱动，13 种主题，DOM 复用保留 scrollback) | swarmx 目前缺少内置终端；开发者需要在外部终端运行命令再回到 swarmx UI；嵌入终端能大幅提升工作流连贯性，也是 CodingAgents launch 的基础 | 中：后端 WS + PTY 已有基础(swarmx 用 PTY 驱动 agent)，可复用；前端 TerminalView 的 WS 协议+DOM 复用+触摸滚动+主题切换代码可直接借鉴 | 后端新增 /ws/terminal endpoint，spawn pty_process::Child；前端 TerminalView 的 buildWsUrl/getOrCreateTerm/mountActiveTerminal/applyTheme 可几乎原封不动适配；DOM 复用策略(moved appendChild) 直接解决多 agent 并发终端切换问题 |
| P0 | Usage/Cost 可观测页（token 用量 + 成本 + cache 命中率） | swarmx 已知空白：usage 可观测。hermes 的 useUsageStore 模式完整：requestId 竞态保护、computed 派生多个维度指标（dailyUsage/modelUsage/cacheHitRate/estimatedCost）、模型按 hash 分配固定颜色。可直接参照实现 | 中，需后端补 usage 聚合 API（按 worker/agent/模型/天汇总）+ 前端 Usage 页 | swarmx 后端 SQLite 已有 session/agent 活动记录，扩展 API 返回 input_tokens/output_tokens/cost；前端新增 /usage 路由页，参照 useUsageStore 的 requestId 竞态模式和 StatCards/DailyTrend 组件结构 |
| P0 | SafeFileStore 原子写（tmp→rename）+ per-path Promise 锁 | swarmx 写 YAML config/MCP config 目前直接 std::fs::write，有并发写损坏风险（多 agent 同时改 CLAUDE.md 等场景）。SafeFileStore 的 tmp+rename+per-path 互斥是正确做法。 | 小（Rust 已有 tempfile crate + tokio::sync::Mutex per path） | services/safe_file_store.rs，在 mcp_admin.rs / profile 相关写操作使用，替换裸 write_file。 |
| P0 | Usage/Cost 可观测（per-session input/output/cache_read/cache_write/reasoning tokens，按 model+天聚合） | swarmx 目前无 token 用量统计。hermes 的 usage_store 结构完整，分别记录 cache read/write/reasoning，按 model 和时间聚合，直接对标 P0 借鉴清单。 | 中（增加 DB 表 + transcript 解析时提取 usage + API 端点 + 前端面板） | db/usage_store.rs（新增）；transcript.rs 解析 JSONL 时提取 usage 字段写入；routes/usage.rs 暴露聚合 API；前端成员栏/设置页增加 usage 面板。 |
| P1 | highlight.ts unified diff 行号+折叠渲染 | swarmx merge-resolver 产生 diff、worker 修改文件都会输出 unified diff；行号分列+未变更行折叠是 code review 类产品的标配，直接提升 merge 可读性。 | 中：依赖 highlight.js，369 行，可作为独立模块引入 | ChatMarkdown.vue 的代码块渲染 + AgentActivity drawer 的工具结果展示(调 extractUnifiedDiffPayload 从 JSON 工具结果中挖 diff)。 |
| P1 | slash 命令下拉语言(bridgeCommands)：/compress /steer /plan /goal status\|pause\|resume\|done\|clear /subgoal 等 | swarmx 聊天输入框目前无 slash 命令；hermes 的完整命令集(17条)覆盖 swarmx 大部分操作(压缩/中止/派任务/设目标)，可对应 swarmx 的 blackboard 操作/orchestrator 指令。 | 中：需要设计 swarmx 的命令语义，前端 UI(下拉逻辑)可直接照搬 | swarmx chat 输入框 + CommandPalette。可映射：/compress→手动触发上下文压缩、/spawn [role]→派 worker、/merge→合并到主线、/abort [agentId]→中止 agent。 |
| P1 | 输入草稿按 sessionId 持久化(localStorage hermes_chat_input_drafts_v1) | swarmx 聊天输入无草稿保存；切方向/切 agent 再切回时输入丢失。按会话/方向 ID 分组存储是标配体验。 | 低：20 行 localStorage 操作，逻辑完全可搬 | swarmx chat composer 输入框。key 改为 swarmx 的 direction_id 或 agent_id，其他逻辑不变。 |
| P1 | context 用量进度条 + 可点击编辑上下文限制 | swarmx 目前有模型配给页但无实时 context 用量可视化；用量/限制/剩余的三段式显示+颜色预警(>60%黄/>80%红)+可直接编辑限制，是 Usage/Cost 可观测的最小可行方案。 | 中：需要后端 API 返回每个 session 的 input/output tokens；前端 UI(进度条+Modal)低成本 | swarmx chat 输入框顶部工具栏，对接 transcript.rs 已有的 token 统计字段；设置模型上下文限制需要在 models_config.rs 增加 per-model context_length 覆盖 API。 |
| P1 | ConversationMonitorPane 只读会话监控：15s 轮询+silent refresh+requestId 防竞态 | swarmx「旁观 worker 当前在干嘛」的只读视图；对比 swarmx 现有的 transcript JSONL tail 推送，轮询模式实现成本更低且可作为 fallback。 | 低：330 行自包含组件，轮询改为 WS 订阅后可升级 | swarmx 蜂群页(SwarmPanel)的 agent 详情 drawer，或方向列表中「旁观正在运行的 worker」面板。后端已有 /api/agents/{id}/activity 类 endpoint。 |
| P1 | OutlinePanel：从消息提取 Q+h1-h3 层级大纲并锚点跳转 | swarmx 长蜂群会话(几十条消息 multi-turn)需要快速定位；纯前端计算无后端依赖，headingIdPrefix 与 MarkdownRenderer 的 heading-N 命名规则一一对应。 | 低：312 行，依赖 VirtualMessageList.scrollToAnchor | swarmx MessagesPanel/SwarmPanel 右侧面板，需要先实现 VirtualMessageList(P0)。 |
| P1 | SessionSearchModal：防抖搜索+键盘导航+动态注入 session | swarmx「方向搜索」缺失；hermes 的「无搜索词显示最近8条」默认列表+matched_message_id 跳具体消息+动态 addOrUpdateSession 注入，UX 完整度高。 | 中：后端需要支持 /api/sessions/search；前端465行，键盘导航逻辑可直接复用 | swarmx 的方向/会话搜索入口(CommandPalette 或独立搜索 modal)。matched_message_id 对应 swarmx 的 message.id 跳转到指定消息。 |
| P1 | Context 压缩策略 per-workspace 可配置（triggerTokens/maxHistoryTokens/tailMessageCount） | swarmx 当前无 context 压缩机制，PTY worker 跑长任务容易超限导致 CLI 被截断或卡死。引入 per-workspace token 预算 + 可配置触发阈值，达阈值时自动 spawn 一个 compressor worker（用 claude -p headless 做摘要），把历史消息摘要后写回 blackboard，worker 后续拿摘要而非全历史 | 高：需后端 token 计数（读 JSONL transcript 累计）+ compressor worker 类型 + 摘要写黑板协议 | 落在 transcript.rs（已有 tail 能力，加 token 累计）+ spawn 一个 compressor 角色（blackboard 写摘要）+ workspace 配置表（Workspace 结构体加三字段）+ 前端设置页加折叠区；不依赖原生 SDK，compressor 就是一个 claude -p headless PTY worker |
| P1 | @mention 全自定义实现（DOM mirror + position:fixed 下拉，IME 安全） | swarmx 聊天输入框若要支持 @agent 定向发送，现有方案（若有）可能被 IME 组字 Enter 误触发送，或 NDropdown/naive-ui 弹层 z-index 干扰。hermes 的 DOM mirror span 精确定位 + dragCounter 模式 + IME compositionEnd requestAnimationFrame 延迟是经过验证的组合 | 中：纯前端，约 200 行 JS，需要配合 mention-options 的保留词逻辑（@all 广播） | 落在前端 chat/composer 组件；@all = 广播所有 worker；@agentId = 定向 PTY inject；mention-options 的 buildMentionOptions 逻辑可直接搬，agent 列表从 swarmx WS 成员状态取 |
| P1 | Tool payload full/display 双份存储 + 事件委托复制 | swarmx 活动面板展示 tool call 时若直接渲染全量 JSON 会撑爆 DOM（特别是 Read 大文件）。hermes 的 truncateJsonValue（深度/节点数/字符串长度四重截断）+ full 保留用于复制，是正确做法 | 低：纯前端工具函数，约 80 行（truncateJsonValue），配合已有 highlight.ts | 落在 activity drawer 或 transcript 展示区；full 串存 IndexedDB 或 ref，display 串走 DOM；复制按钮用 data-copy-source 事件委托减少 listener 数量 |
| P1 | WS 连接状态点（connected 绿+glow / disconnected 红） | swarmx 蜂群页已有 WS，但连接状态指示不够明确（用户不知道是否实时）。8px 圆点+glow shadow 是极低成本的实时状态可观测 | 极低：纯 CSS + 一个 computed | 落在蜂群页 header 或全局 navbar；ws.readyState 映射到 connected/disconnected；glow shadow 用 swarmx 已有 CSS 变量 --success-rgb |
| P1 | Provider/Model 可见性过滤（include 模式） | swarmx 模型配给页目前直接列出所有 model，大型 provider（如 Alibaba/OpenAI）有几十到上百个模型，下拉极长。引入 hermes 的 visibility include 模式：用户勾选想看到的 model 子集，存为 include 规则；全勾时自动升级为 all 节省空间。 | 后端：app_settings 增加 provider_visibility 字段（已有 settings store）；前端：在模型选择下拉旁增「过滤」入口 + checkbox modal，约 1-2 天 | 落在 models_config.rs（settings 层）+ 前端 models 设置页 ProviderCard 操作栏 |
| P1 | Model Alias（用户友好别名层） | swarmx 的 model tier 已有 abstract→concrete 映射，但 concrete model ID 对用户不友好（如 claude-opus-4-5-20251101）。引入 hermes 的 alias 机制：本地存储 per-model 别名，显示时「别名 + 小号原始 ID」。不污染服务端配置。 | 前端：app_settings（localStorage/SQLite）增 model_aliases Map；显示层统一通过 displayModelName 函数；alias 编辑 modal 复用 hermes 结构，约 1 天 | 落在前端 store/settings（model_aliases 字段）+ 所有展示 model 名的地方（spawn 配置页/成员栏/角色卡）统一走 displayModelName |
| P1 | MCP 服务器状态可视化（connected/disconnected/disabled 三态卡片） | swarmx MCP 管理页（routes/mcp_admin.rs）已有基础，但卡片缺乏状态可视化。引入 hermes McpServerCard 的三态设计：connected（绿色 badge）/disconnected（红色边框+badge）/disabled（opacity 0.7）；工具标签列表（88px scroll）；内联 NSwitch 快速启停；NPopconfirm 轻量删除确认。 | 后端：MCP server 增 connected 字段（连接探测）+ /test 端点；前端：McpServerCard 结构直接对应，改为原生 JS/HTML，约 2 天 | 落在 src/mcp_admin.rs（增 test 端点 + connected 状态）+ 前端 mcp 页的卡片组件 |
| P1 | 草稿隔离 + unsavedChanges JSON diff 门控 Save 按钮 | MCP 配置编辑时先写草稿，JSON.stringify diff 后才 enable Save 按钮，防止误触 API 重启。现在 swarmx MCP 页即时写，每次开关都触发后端 reload | 1天：前端加 draft/touched reactive + hasChanges computed | 前端 MCP 管理页组件；配合后端 mcp_admin.rs 的「只在 Save 时写 config 文件」 |
| P1 | config section vs credentials 分离 API（敏感数据单独 endpoint） | MCP API key 等敏感凭据走独立 POST /credentials endpoint，与普通配置分离；敏感数据不 round-trip 回前端。现在 swarmx MCP 配置全量发，api_key 在 GET 响应里明文可见 | 2天：后端加 /api/mcp/credentials PATCH，前端分两路提交 | 后端 routes/mcp_admin.rs 新增 route；前端草稿分 configDraft/credentialDraft 两棵树 |
| P1 | 会话重置策略面板（idle_timeout / daily_reset / at_hour） | swarmx 无 agent 会话超时管理。长时间无操作的 agent 应该自动 graceful stop 释放 PTY 资源；daily reset 对长跑 orchestrator 尤其有用 | 3天：后端 session_reset 定时器（tokio cron）+ 前端设置面板 | 后端 agents/session.rs 新增超时检测；前端 settings 页新增 SessionSettings 面板；与现有 autokill grace 机制互补 |
| P1 | context 压缩策略可配置（threshold/target_ratio/protect_last_n/protect_first_n） | swarmx 没有 context 压缩。长时间运行的 agent PTY 会 overflow context limit 导致 truncation 无提示失败；hermes 暴露 5 个细粒度参数让用户调优 | 5天：后端 context 压缩逻辑（插入 summary 消息）+ 前端 CompressionSettings 面板 | 后端 spawn 侧读取 compression config，在 inject_turn 前检查 token 估算；对 claude/codex 两种 CLI 策略相同（都是 PTY 注入 summary prompt） |
| P1 | IP 锁定管理面板（暴力破解防护） | swarmx 如果暴露 Web UI 到局域网/公网，需要登录防暴力破解。hermes 的 lockedIps 面板让管理员可以查看/解锁被锁 IP，避免合法用户被锁死 | 2天：后端 IP 限速逻辑(axum middleware) + locked_ips 表 + 解锁 API + 前端面板 | 后端 auth middleware + SQLite locked_ips 表；前端 AccountSettings 或独立安全面板 |
| P1 | 文件浏览器（FileTree + FileList + FileEditor + FilePreview + FileContextMenu + FileToolbar + 模态组件） | swarmx 的 worker 在 workspace cwd 下产出文件，用户现在只能通过 terminal/IDE 查看。内置文件浏览器让用户直接在 Web UI 里看 worker 产物、编辑配置文件、下载结果——闭环操作不离开 swarmx | 大（后端需要实现 files API：list/read/write/delete/rename/mkdir/copy/upload/download 共 9 个端点，axum + tokio::fs，路径限制在 workspace.root_path 下防 path traversal；前端 9 个 Vue 组件 + Pinia store + Monaco Editor 懒加载） | 后端新增 routes/files.rs，路径限制在 workspace.root_path 下（防 path traversal）；下载端点 GET /api/workspace/{id}/download?path=...&token=... 把 token 放 query param（<a> 标签限制）。前端：isTextFile/isImageFile 工具函数和 EXT_LANG_MAP 直接搬，isAffected 删除/重命名失效逻辑直接复用，Monaco worker ESM 构造方式直接复用 |
| P1 | token 防双包装下载 URL 保护（getDownloadUrl 里 startsWith 检测 + decodeURIComponent 防双重编码） | swarmx 聊天里 agent 经常在回复中输出文件路径，如果把路径直接拼下载 URL，agent 可能已经输出过包含 download endpoint 的 URL，二次拼接导致双包装 404 | 极小（在 getFileDownloadUrl 工具函数里加 5 行防御检查） | 落在 src/api/files.ts 的 getFileDownloadUrl 函数，加 startsWith 检测 + URL 解包逻辑，同时做 decodeURIComponent 防双重编码 |
| P1 | 堆叠柱状图纯 CSS 实现（无 echarts/chart.js，hover tooltip 用 CSS ::after） | swarmx 前端包体积已经较大（Tauri + Monaco 等），token 用量图用纯 CSS 柱状图可以零增量，且 CSS transition height 效果够用 | 小（DailyTrend.vue 约 290 行含 CSS，完整搬来改数据绑定） | 落在 /usage 页面的 DailyTrend 组件，数据来自 Rust 后端聚合的 daily_usage JSON；把 sessions 字段改为 agent_runs，cost 字段从 claude pricing 表估算（input*3$/Mtok + output*15$/Mtok） |
| P1 | ModelSelector：provider 分组 + 搜索过滤 + disabled/preview/custom 标签 + custom model 输入 | swarmx 现有模型选择器（设置页）相对简单，没有搜索和自定义模型输入。hermes 的 ModelSelector 设计更完整，特别是 disabled 双重防护（列表点击 + 输入框提交都检查）和 custom model 管理 | 中（约 480 行，需要把 hermes 的 appStore.profileModelGroups 对应改为 swarmx 的 models_config.rs 数据格式） | 落在 swarmx 设置页的「模型配给」区域（已有 per-CLI 模型配置），把现有的 tier→model 映射 UI 升级为带搜索/分组/自定义的完整选择器；provider 字段对应 swarmx 的 cli_type（claude/codex） |
| P1 | PerformanceView：系统 CPU/内存仪表盘+进程表+worker 表+session 统计，5s 自动刷新 | swarmx 的 Performance/Observability 是已知空白；多 CLI agent 并发时资源情况完全不可见；worker OOM/hang 只能靠查日志 | 中：后端用 sysinfo crate 采集数据；前端直接参考 PerformanceView 的布局(summary-grid + process-grid + worker-table)；格式化函数 formatBytes/formatPercent/formatDuration 可直接移植 | GET /api/performance 返回 PerformanceRuntimeSnapshot 格式数据；agent 对应 worker；5s auto-refresh 用前端 setInterval；process 部分显示 swarmx-server PID + 各 agent PID |
| P1 | History 页的分组折叠+置顶+批量删除+分享链接，状态持久化 localStorage | swarmx 现有会话列表是平铺的，随着 agent 增多会混乱；按方向/workspace 分组+折叠+置顶可以大幅提升列表可用性 | 小：纯前端功能，参考 HistoryView 的 groupedSessions/pinnedSessions/collapsedGroups/sessionSelectionKey 逻辑，适配 swarmx 的 agent/direction 分组维度 | 蜂群页的成员列表替换为分组可折叠列表，group by direction/workspace；置顶 agent；批量 kill/archive；session 分享链接带 workspace_id query param |
| P1 | 登录鉴权页：username+password 登录，429/503 速率限制提示，session token 存储 | swarmx 目前无认证机制，多用户/暴露到内网时无法控制访问 | 小：参考 LoginView 的完整实现(~100行)；后端 axum 增加 /api/auth/login endpoint + session token 验证中间件 | LoginView 代码几乎可直接移植；sessionToken 存 localStorage setApiKey；所有 API 请求带 Authorization header；速率限制用 governor crate |
| P1 | MCP 管理页增强：JSON/YAML 双格式输入+自动格式化+工具可见性 include/exclude 过滤+指数退避自动重试 | swarmx 现有 MCP 管理页(/mcp)已做基础 CRUD；hermes 的这三个增强(yaml 支持/工具过滤/自动重试)能显著提升可用性 | 小：yaml 和工具过滤是纯前端；自动重试逻辑在后端或前端均可实现；参考 McpManagerView 的 parseConfig/extractServers/openToolsModal/scheduleReload | 在现有 /mcp 页添加 YAML tab；工具过滤写入 per-agent MCP config 的 tools.include/exclude 字段；指数退避用 setTimeout 链 |
| P1 | SkillsUsage 纯 CSS 堆叠条形图 + per-period 缓存 + 请求序列号防竞态 | swarmx 未来需要展示 skill/tool 使用频率和 cost 分布；hermes 的零依赖实现和防竞态方案可以直接采用 | 小：纯前端技巧，chartSegments/colorForSkill/tooltipAlignment 函数可直接复制；statsByPeriod 缓存策略和 requestSeq 防竞态是通用模式 | 用于 Usage 页的工具调用次数日趋势图；skill 替换为 tool_name；colorForSkill 按 tool 列表排序取调色板 |
| P1 | 模块级单例 ref 替代轻量 store | useTheme/useSessionSearch/useToolTraceVisibility 模式：模块作用域 ref 天然单例，任意 import 共享。swarmx 前端目前有些 UI 偏好可能散在各组件 localStorage 读取，统一用此模式，减少样板代码 | 低，纯前端重构 | 适用于 swarmx 前端的 theme/侧边栏折叠/工具 trace 可见性/方向选择等轻量全局偏好，对应 styles/theme.ts + 各页面 useState |
| P1 | Cron/定时任务 UI（JobsView + JobFormModal） | swarmx 已知空白：cron 定时。hermes 的 Job 数据结构（schedule union type: interval\|cron\|once）、scheduleToEditableInput/scheduleToDisplayText 工具函数、pause/resume/run-now 操作完整可参照 | 高，后端需实现 job scheduler（类 cron）+ REST API，前端参照 hermes 的 JobsPanel/JobCard/JobFormModal | swarmx 后端新增 jobs 表 + axum routes；前端 /hermes/jobs 对应 /jobs 路由；scheduleToEditableInput 逻辑直接搬 |
| P1 | Kanban 任务面板（能力发现式 + WebSocket 实时刷新） | swarmx 已知空白：kanban 任务面板。hermes 的 capability 发现（assertCapability）使 UI 适配不同后端版本、boardGeneration 双重竞态保护、WebSocket event stream + debounce 刷新模式值得学习 | 高，后端需 kanban CRUD + WebSocket event stream；能力发现模式降低前后端耦合 | swarmx TaskCreate/TaskUpdate 工具已有任务 CRUD 概念，Kanban 页可复用这些语义；WebSocket event stream 参照 kanbanStore.connectEventStream 模式，接入 swarmx 已有的 WS 广播体系 |
| P1 | requestId 竞态保护模式（标准化到所有异步 fetch store） | swarmx 多处可能有 async 状态竞态（workspace 列表、agent 列表刷新），hermes 的 latestRequestId 自增模式标准、简洁，应推广到所有 store 的 fetch 函数 | 低，模式统一，逐个 store 加 requestId check | swarmx 前端 stores/ 目录下所有含 async fetch 的 store，在 loading start 前 ++requestId，response 里检查是否等于 latestRequestId |
| P1 | JWT 客户端解码做路由级权限守卫 | swarmx 若要加登录鉴权（已知空白：登录鉴权），路由守卫可在客户端解析 JWT 的 role 字段控制页面可见性，无需额外 server 请求 | 低，前提是后端先实现 JWT 鉴权 | swarmx router/index.ts beforeEach 中实现类似 isStoredSuperAdmin 逻辑，读 JWT payload.role 决定 admin-only 路由（如 MCP 管理页、性能监控页）的访问权 |
| P1 | claude-code-proxy + codex-proxy 协议转换层：把 OpenAI/Anthropic API 包成本地 HTTP 代理，claude CLI 直接对接 | swarmx 目前 PTY 驱动是唯一入口。如果将来支持「API 模式」（非 PTY）让 worker 更轻量，可以用相同的 registerTarget+routeKey+token 模式，在本地 axum 路由层做协议适配。对 codex 来说尤其有价值（codex 原生 Responses API）。 | 大（需要在 Rust 实现 SSE 流式转换 + 协议适配） | routes/claude_proxy.rs + routes/codex_proxy.rs，注册 target 时生成 routeKey，插入 tower/axum 路由。SSE 转换用 async generator 等价的 Rust async Stream。 |
| P1 | 登录限流（per-IP + 全局双层，持久化 JSON 锁文件） | swarmx 目前无 auth 限流。如果加 Web UI 鉴权（已有 AUTH_TOKEN 概念），需要防暴力破解。hermes 的双 IP map（password/token 交叉锁）+ 全局窗口 + 同步写持久化 + pruneIpMap 清理策略可以直接搬。 | 中（Rust 用 DashMap + 文件写即可，逻辑直接翻译） | services/login_limiter.rs，在 routes/auth.rs 鉴权路径调用 check_password/record_failure。 |
| P1 | ContextEngine 双路径压缩（增量快照 + 全量压缩 + CJK-aware token 估算） | swarmx 群聊/上下文无压缩机制，超长历史直接截断。hermes 的 Path A/B 策略 + per-room 锁 + 降级策略是工程上可靠的实现，CJK token 估算对中文用户尤其重要。 | 大（需要 Rust port + 配套 DB 表 gc_context_snapshots + 接入 LLM summarize 接口） | services/context_engine.rs（新增）；group_chat 消息处理路径接入；summarize 调用可复用现有 gateway/claude proxy。 |
| P1 | 群聊消息 ID 相位排序（tool_call/tool_result/assistant 三态 phase） | swarmx 聊天消息在 agent 并发工具调用时可能乱序显示。hermes 的 groupRunOrder + sortGroupMessages 通过 ID 编码语义+稳定排序解决这个问题，无需额外字段。 | 小（前端或后端排序逻辑，ID 命名约定） | 前端 chat.js 消息列表排序，或后端 routes/messages.rs 查询时按 ORDER BY 规则处理。 |
| P1 | OpsRuntimeSnapshot 系统+进程指标快照（跨平台：/proc、vm_stat、PowerShell） | swarmx 已有成员栏活动行，但无 CPU/内存监控。hermes 的 ops-monitor 包含完整的跨平台（Linux /proc, macOS vm_stat, Windows PowerShell）实现，以及 agent bridge workers 的 per-PID 指标。 | 中（Rust 用 sysinfo crate 更简单，参考 hermes 的数据结构设计） | services/ops_monitor.rs（新增），通过 /api/perf 端点暴露；前端 performance-monitor 组件展示 web + agent workers 内存/CPU。 |
| P2 | 批量删除 session：isBatchMode + selectedSessionKeys Set + Popconfirm 二次确认 | swarmx 蜂群列表有批量操作需求(批量终止 worker/删方向)；hermes 的 profile\0id 复合 key 防跨维度冲突的设计值得借鉴。 | 低：UI 逻辑可照搬，后端 batchDeleteSessions API 需要在 swarmx 侧实现 | swarmx 方向列表/蜂群列表的批量操作。复合 key 可改为 direction_id 或 workspace_id。 |
| P2 | SessionListItem 移动端长按触发 contextmenu(合成 MouseEvent 含触点坐标) | swarmx 如果有移动端用户，右键菜单功能完全不可用；500ms 长按方案是 iOS/Android 的标准 fallback，touchmove 清 timer 防滑动误触。 | 低：~30 行，SessionListItem.vue 或 SwarmMember 列表项均可加 | swarmx SessionListItem 等价组件，或蜂群成员列表项。 |
| P2 | FolderPicker：懒加载树形目录选择器(扁平 FlatNode[] + childrenCache Map) | swarmx 已有 setSessionWorkspace 功能但缺 UI 选择器；hermes 的扁平数组避免递归组件+缓存子目录防重复请求是成熟实现。 | 低：~250行，后端 /api/hermes/workspace/folders 对应 swarmx 的文件系统 API | swarmx 设置 workspace 路径的 Modal(ChatPanel 已有 setWorkspace 操作)。后端接口可复用 swarmx 现有文件系统浏览 endpoint 或新增。 |
| P2 | Kanban 任务面板（triage/todo/ready/running/blocked/done 状态机） | swarmx 已知短板：无 kanban 任务面板。hermes 的状态机设计（7 状态、border-left 颜色编码、canMutateTask 终态保护、两步 complete/block、parent/children 链接）完整且可搬。配合 swarmx blackboard 的 typed handoff key，kanban task 可成为编排单元 | 高：需后端新增 kanban 表（SQLite）+ REST API + WS 广播任务状态变更；前端四组件全写 | kanban 任务 id = blackboard key 前缀，task.status 变更通过 BlackboardChanged 广播；assignee = role slug（角色注册表已有）；running 状态对应 agent 正在执行；done = .done 写入；前端 KanbanColumn 五列 = swarmx 的 pending/in_progress/blocked/done/error 状态 |
| P2 | 定时任务（Cron Job）面板：cron 表达式 + deliver 渠道 + 运行历史 | swarmx 无 cron 定时触发能力。hermes 的 JobCard+JobFormModal 模式（preset 下拉+手填 cron+repeat 次数+deliver 渠道）直接对标 swarmx 的 orchestrator spawn：定时 spawn orchestrator，传入 prompt，结果写 blackboard 或推渠道 | 高：后端 cron 调度器（tokio-cron 或 cron crate）+ job 持久化 SQLite + 运行历史日志；前端 JobCard 样式可直接参考 | 落在 swarmx-server 新增 jobs.rs 模块；deliver 对应 swarmx workspace 的 WS 广播（local）；deliver 到外部渠道暂不做，只做 local+blackboard；JobRunHistory 中的 MarkdownRenderer 对应 swarmx 已有的 markdown 渲染 |
| P2 | Job 编辑时 diff-only update（buildJobUpdateRequest） | swarmx 编辑 agent/workspace 配置时全量覆盖，容易踩 cron 调度重置的坑。diff-only 只发送变化字段，后端只更新 diff，避免无意义写入触发副作用（如 cron 重排） | 低：纯前端工具函数，约 30 行 | 落在前端 api/agents.ts 或 api/workspace.ts，编辑表单提交前对比 originalData 和 formData |
| P2 | Kanban task 与 AI chat session 双向关联（search-sessions API） | swarmx 每个 agent 有 session JSONL，若 kanban 任务由 agent 执行，应能从 task 反查执行过程的 session，形成可溯源的任务-执行双向链接 | 中：后端 search API（按 task_id+agent_id 关联）+ 前端 task drawer 内嵌 session 列表 | 落在 kanban task detail；agent session = swarmx agent 的 JSONL transcript 文件路径；关联记录在 task run 表（agent_id + task_id + started_at） |
| P2 | dragCounter 计数解决 drag-leave 误判 + 附件管理（objectURL + 去重 + revokeOnRemove） | swarmx composer 若支持附件（图片/文件传 agent），需要正确的拖拽状态管理，dragLeave 误判是经典坑。hermes 的 dragCounter 模式直接可用 | 低：约 30 行 JS，附件管理 addFile/removeAttachment 约 30 行 | 落在 swarmx 前端 composer 组件；附件 url 走已有的 GET /api/file 端点 |
| P2 | 两步式危险操作（block/complete 展开输入框后二次确认） | swarmx 已有 Popconfirm 模式，但对需要附带信息（block reason / complete summary）的操作不够用。两步式（第一次点击展开 input，第二次才提交）更轻量且收集必要信息 | 极低：showBlockInput/showCompleteInput ref 控制，约 20 行模板 | 落在 worker 强制停止（需填停止原因）或方向合并前的确认（需填 commit message）；已有类似需求场景 |
| P2 | Workspace/Profile 导出导入（tar.gz） | swarmx workspace 是 git 目录，无独立备份机制。hermes 的 profile export（tar.gz blob 下载）+ import（multipart upload）模式：前端触发 POST /export → 拿 blob → 临时 <a> 下载；import 走 FormData POST。可用于 workspace config（CLAUDE.md/settings.json）备份/迁移。 | 后端：/api/workspaces/{id}/export（tar.gz 打包 .claude/*.json + CLAUDE.md）+ /import；前端：ProfileImportModal 结构直接复用，约 2-3 天 | 落在 routes/workspaces.rs；不需要打包 git 历史，只打包 swarmx 托管的配置文件 |
| P2 | Skill 系统（per-workspace 编排脚本库） | swarmx 编排靠 system_prompt + typed handoff，无可复用脚本库。hermes 的 skill = category/name/SKILL.md + 附属文件 + source（builtin/hub/local）+ toggle enabled + pin + usage stats。可为 swarmx 实现「编排模板库」：常用 orchestrator prompt 模板 + worker 角色定义脚本，按 category 管理，支持 enable/disable。 | 后端：skill = ~/.swarmx/skills/{category}/{name}/SKILL.md；CRUD + toggle + pin + usage stats；前端：直接复用 hermes SkillList + SkillDetail 结构，约 5-7 天 | 落在 routes/skills.rs（新模块）+ 前端新增「模板库」页；skill 内容为 orchestrator/worker system prompt 的 markdown 模板，spawn 时可 include |
| P2 | 确定性生成头像（multiavatar/seed）替代用户上传头像 | swarmx agent 成员栏目前用 hash 取色 emoji 头像。multiavatar 库按 seed 生成确定性 SVG，视觉更丰富、不重复，且零存储成本。seed 默认取 agent_id 或 profile_name，用户不需要上传图片就有独特头像。 | 前端：npm install @multiavatar/multiavatar，替换成员栏和历史记录中的 emoji 头像组件，约 0.5 天 | 落在前端 AgentAvatar 组件（新建）+ 成员栏/蜂群页/聊天气泡中引用；Tauri 下可打包进 bundle |
| P2 | Skill 使用统计（view/use/patch 三维追踪） | swarmx 缺乏对编排 prompt/模板使用情况的可观测性。hermes 的 skill usage stats（按天+按 skill 的 view_count/use_count/patch_count）提供了「哪些模板被频繁使用/编辑」的洞察。对应 swarmx 可追踪「哪些角色模板被频繁 spawn」。 | 后端：sqlite 增 skill_usage 表（skill_key, event_type, timestamp）+ /api/skills/usage/stats 聚合查询；前端：在 skill detail 标题行显示三项统计，约 2 天 | 落在 db/schema.sql（新表）+ routes/skills.rs + 前端 SkillDetail 标题行 |
| P2 | Profile Clone 时的凭据清理+清单反馈 | swarmx 多「方向」(thread)时，新方向可能复用主 worktree 配置。hermes 的 clone profile 在服务端自动清理独占凭据（OAuth token 不能共享），并把 strippedCredentials/disabledPlatforms 清单返回给前端用 6000ms info toast 展示。这个模式适合 swarmx 在「新建方向」时检测并提示 .claude/settings.json 中的敏感内容。 | 后端：新建 thread/direction 时扫描 worktree 配置，提取需要隔离的字段；API 返回 stripped_fields 清单；前端：长 toast 展示，约 1-2 天 | 落在 routes/threads.rs（新建方向逻辑）+ 前端 thread 创建 modal |
| P2 | TTS 试听面板（多 provider：WebSpeech/OpenAI-compatible/Edge） | swarmx agent 活动实时日志用文字展示，加 TTS 后可以让用户「听」agent 正在做什么，尤其适合离开屏幕时后台监控 | 3天：前端 VoiceSettings 组件(直接搬) + /api/tts/proxy 后端 Edge TTS 代理；OpenAI-compatible 部分纯前端 | 前端新增 VoiceSettings 面板；后端 routes/tts.rs 代理 Edge TTS 请求（避免 CORS）；与 bell_on_complete 联动 |
| P2 | localStorage 分层持久化 + schema 版本迁移 | swarmx 前端 UI 偏好（主题/紧凑模式/是否显示推理过程）目前都走服务器配置；部分用户级偏好应该纯 localStorage（多用户共享一个服务器时各人偏好不互扰） | 1天：提取 client-only prefs 到 localStorage composable，加 storage key 版本后缀和 migrateOldKeys | 前端新 composable useClientPrefs.ts；从 settings store 中剥离 display.compact/display.show_reasoning 等纯 UI 字段 |
| P2 | completionNotificationsReady 门控防初始化虚假通知 | swarmx 活动通知有类似风险：用户打开设置/通知面板时可能看到历史完成事件的 toast。用 ready 标志 + 记录初始 timestamp 可以消除 | 0.5天：在 watch 回调加 ready 门控布尔和初始值记录 | 前端通知组件/活动日志 watch 逻辑；不影响后端 |
| P2 | UserManagement 面板（多用户 RBAC：admin/super_admin + profile 权限） | swarmx 目前单用户。如果将来支持团队使用（已有 workspace 概念），需要用户管理：创建用户、分配角色、限制可见 workspace/profile | 5天：后端 users 表 + RBAC middleware + 用户 CRUD API；前端 UserManagementSettings 面板 | 后端 auth/users.rs + SQLite users 表；前端设置页新增 UserManagement 面板；与现有 workspace 权限模型对接 |
| P2 | usePersistentRecord 轻量 localStorage 绑定（手动 flush，不自动 watch） | swarmx 前端侧边栏折叠状态、设置页各种 boolean 开关已经用 localStorage，但散落各处 getItem/setItem。usePersistentRecord 提供统一 reactive record + 显式 persist() 的模式，避免频繁 IO | 极小（25 行 composable，直接搬） | 落在 src/composables/usePersistentRecord.ts，用于侧边栏 collapsedGroups、设置页各种 boolean 开关的持久化 |
| P2 | Auth 事件总线（CustomEvent 'hermes-auth-notice' + 1200ms 防抖 + AuthEventListener 组件） | swarmx 目前没有 token 过期处理，HTTP 请求返回 401 时用户看不到提示就失去会话。hermes 的模式把 API 拦截器和 UI 提示完全解耦，批量 401 只显示一次 toast | 小（API 拦截器发 CustomEvent，AuthEventListener 组件 20 行） | 落在 src/api/client.ts 的 request 函数的 401/403 处理分支，发 CustomEvent；AuthEventListener 挂在 App.vue 根组件。swarmx 已有简单 token 机制（getApiKey），只需补 401 拦截和 toast 通知 |
| P2 | 指数退避轮询 + 状态驱动取消（scheduleRuntimeStatusPoll 模式） | swarmx 前端有些地方需要轮询 agent 状态，scheduleRuntimeStatusPoll 的模式（首次快、后续慢、最多 N 次、通过状态变量提前退出，无需 clearTimeout）比 setInterval 更优雅 | 极小（提取为通用 composable，约 20 行） | 落在 src/composables/useStatusPoll.ts，用于需要短暂轮询直到状态稳定的场景（如 spawn agent 后等待 ready 状态、merge 操作完成检测） |
| P2 | RouteLinkItem：RouterLink v-slot custom 模式 + aria-current 注入 | swarmx 侧边栏导航项目前自己写 :class=activeRoute，没有统一 active 判断逻辑，也没有 aria-current='page' 无障碍支持 | 极小（30 行组件，直接搬） | 落在 src/components/common/RouteLinkItem.vue，替换侧边栏各 NavItem 的 active 判断散兵代码，顺便补 a11y |
| P2 | DefaultCredentialPrompt：per-user sessionStorage dismiss + isDesktopShell 白名单 | swarmx 如果后续支持多用户或 web 部署，默认密码提醒是安全必备。per-user sessionStorage key 和桌面壳白名单（Tauri 不弹）的设计值得直接复用 | 小（100 行组件） | 落在 src/components/auth/DefaultCredentialPrompt.vue，桌面壳检测改为 window.__TAURI__ 检测（替代 window.hermesDesktop.isDesktop），web 部署时弹提醒 |
| P2 | Settings 页多 tab URL 持久化(query.tab)，normalizeTab 防御非法值 | swarmx 设置页当前无法直链特定 tab；URL 持久化后可以分享设置链接、刷新后恢复位置 | 极小：参考 SettingsView 的 normalizeTab + handleTabUpdate + watch(route.query.tab) 共 10 行逻辑 | 在 swarmx 现有设置页加 watch route.query.tab + router.replace，normalizeTab 防御已知 tab 名列表 |
| P2 | CodingAgents 视图：CLI 安装/卸载状态检测+版本显示+配置文件内联编辑(多文件切换+dirty 检测)+launch 弹窗(global/scoped 模式+自动推断 API protocol) | swarmx spawn 前需确认 CLI 已安装且版本兼容；配置文件编辑当前需要手动用系统编辑器 | 中：后端需 CLI 探测(which/version)和 config 文件读写 API；前端参考 CodingAgentsView 的 hasConfigUnsavedChanges+loadConfigFile+defaultLaunchApiMode | 整合进 swarmx 设置页或新增 Agent Tools 页；配置文件范围：~/.claude/settings.json/.claude.json/CLAUDE.md 和 ~/.codex/config.toml/auth.json/AGENTS.md；defaultLaunchApiMode 的 provider 探测逻辑可用于 spawn 时自动配置 API mode |
| P2 | mergeMessagesWithFallback 递归 deep-merge i18n | swarmx i18n（zh/en）若部分翻译，整体 key 缺失会 fallback 到 undefined。hermes 的逐 leaf key 递归 merge 保证部分翻译安全 | 低，纯工具函数，30 行 | 直接复制 mergeMessagesWithFallback 到 swarmx 前端 i18n/messages.ts，在组装 messages 时替换 vue-i18n 原生 fallback |
| P2 | tab 可见性重同步（visibilitychange） | 用户后台放置页面后切回，WS 事件可能已丢失，hermes 的 visibilitychange 触发 resume session 重同步是必要的健壮性措施 | 低，在 swarmx 前端 chat 或 swarm 状态 store 里监听 document.visibilitychange | swarmx 蜂群页的 agent 状态/活动记录可在 visibility=visible 时触发一次 GET /api/agents 刷新，防止后台 WS 断联后状态陈旧 |
| P2 | LocalStorage 写入 QuotaExceededError 自愈 + 旧缓存 prefix 清理 | swarmx 若 localStorage 存大量 agent 记录/blackboard 快照，可能超配额。setItemBestEffort + recoverStorageQuota 模式可直接借用 | 低，工具函数级别，约 40 行 | swarmx 前端 utils/ 中添加 storage.ts 提供 setItemBestEffort，所有持久化 localStorage 调用改用此函数 |
| P2 | FOUC 防护：主题类在 createApp 之前写 documentElement | swarmx 若支持 dark mode，main.ts 必须在 mount 前读 localStorage 写 class，否则首屏闪白 | 极低，5 行代码 | swarmx 前端 main.ts 顶部，在 createApp 之前读 brightness localStorage key 并 classList.add('dark') |
| P2 | mentionDepth 限制 agent-to-agent @mention 链式调用深度 | swarmx 群聊（如果未来加）需要防止 agent 互相无限 @mention 调用。hermes 的 mentionDepth 从消息级别传递、逐层+1、超阈值停止路由是简洁有效的方案，无需全局状态。 | 小（聊天路由时检查 mention_depth 字段） | routes/group_chat.rs 或 services/mention_routing.rs，在 process_mentions 时传递 depth 参数。 |
| P2 | Kanban 「功能支持矩阵」自文档化（getCapabilities → supported/partial/missing） | swarmx 的功能集在快速迭代中，前端有时不知道某功能是否可用。hermes 的 capabilities 静态声明模式让 API 自文档化，前端可按能力分级渲染 UI（如灰化 missing 功能）。 | 小（定义 capabilities 结构体 + GET /api/capabilities 端点） | routes/capabilities.rs，声明 directions/merge/git/mcp 等功能的 supported/partial/missing 状态，前端命令面板/设置页按此渲染。 |
| P2 | Agent Bridge 子进程就绪感知（stdout JSON line {event:'ready'}） | swarmx PTY 驱动 agent 时也有「等待 CLI 就绪」的问题（trust prompt/hooks dialog）。hermes 通过 stdout JSON line protocol 精确感知就绪，配合超时+指数退避重启，比 swarmx 当前的 PTY auto-answer 更结构化。 | 中（需要 CLI 端配合输出 ready 事件，或在 PTY 层检测） | services/process_manager.rs，在 spawn_worker 时读取 PTY 输出流，匹配 ready 模式后才标记 agent 为 ready 状态，替代当前的定时等待。 |

