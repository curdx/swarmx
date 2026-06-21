# hermes-webui 深度借鉴报告（2026-06）

> nesquena / hermes-webui — 全栈 WebUI (FastAPI 风 api + 原生 JS 前端 + PWA)
>
> 由 9 个专读 agent 穷举产出 · 函数级条目 **596** · 框架思想 **110** · 页面元素 **63** · 借鉴点 **91**
> 
> 本文为机器穷举 + 结构化整理，未做删减；引用格式 `文件:符号`。

**目录**：0 定位与架构哲学 · 1 框架思想/设计模式 · 2 函数级地图（穷举）· 3 页面元素穷举 · 4 swarmx 借鉴小结

## 0. 定位与架构哲学

nesquena 的 hermes 全栈 WebUI——后端 52 个 FastAPI 风 `api/` 模块 + 前端 5 万行**原生 JS（无框架）** + PWA（v0.51）。一句话：把一个 agent runner 包成**可多人用、可离线、带 worktree / 计量 / 会话恢复**的 Web 应用。它的 `api/` 分层（session 生命周期、turn/run journal、worktree、metering、route_approvals）与 swarmx 的 Rust 后端职责高度重叠，是「后端 API 该切成哪些模块、会话状态机怎么建」的对照表；前端无框架 5 万行则反向印证 swarmx「轻量原生前端」路线的可行边界与代价。

## 1. 框架思想 / 设计模式

### 前端 UI 渲染骨架 ui.js + panels.js

- **单例 S 对象作为前端唯一数据源（Single Source of Truth）** — `ui.js:8 - const S={session, messages, entries, busy, ...}`
  - 无 VDOM 框架时用全局单例避免状态散乱；所有视图函数从 S 读取数据，所有 SSE 事件更新 S 后调用 render 函数。代价是必须手动维护 render 时机，但对 1 万行单文件 JS 而言比引入框架依赖更简单
- **Stash-Token Pipeline for Markdown Rendering** — `ui.js:renderMd - blockquote/MEDIA/fence/math/rawPre 依次 stash 为 \x00X{n}\x00 token`
  - 多遍 regex 管线中各遍之间存在互相干扰（如 diff 行被 list regex 误识别）。stash 把已处理的块替换为不含 markdown 特殊字符的占位符，下游 regex 不会命中。最后 restore 顺序必须反向（先恢复深层 stash 再恢复浅层）。这是无解析器树的 regex markdown 系统的标准解法
- **两级消息渲染缓存（per-message + per-session）** — `ui.js:_renderCache(Map) + _sessionHtmlCache(Map)`
  - 长会话每次 SSE 事件都触发 renderMessages()，95% 的消息内容不变。per-message cache 跳过 renderMd 计算；per-session cache 在切换 session 时跳过整个 innerHTML 重建。关键 guard：streaming 中跳过 session cache 防止覆盖 live smd DOM 节点
- **DOM Window（消息分页）+ 滚动位置保持** — `ui.js:_messageRenderWindowSize + _showEarlierRenderedMessages`
  - 长会话 DOM 节点数影响 frame rate。只渲染最新 50 条，点 'Load earlier' 批量展开。扩窗口时保存 scrollHeight 差值使视口锚定，避免跳动。类似虚拟化但更简单（无 absolute 定位，牺牲 DOM 节点数换实现简单性）
- **model 状态三层 fallback（session > localStorage > profile default）** — `ui.js:syncTopbar, _reconcileModelDropdownSelection, _applyPendingSessionModelForSession`
  - 多页面/刷新/session 切换场景下 model 选择极易丢失或错误跳回。用 sessionStorage（TTL 10min）暂存新建 session 前的用户选择；provider 列表刷新后用 _reconcileModelDropdownSelection 重对齐；resolve_model API 竞态用 _modelResolutionDeferred flag 延迟 persist
- **Panel 状态机（empty/read/create/edit）** — `panels.js:_cronMode, _workspaceMode, _profileMode + setCronHeaderButtons(mode)`
  - 左-主布局中每个 panel 内部独立维护 mode 状态字符串，header buttons 的显隐完全由 mode 驱动（show/hide 函数替代 if-else），确保'创建表单时不显示 Delete 按钮'等约束不会被遗忘
- **SSE → polling fallback 的事件流策略** — `panels.js:_kanbanStartEventStream / _kanbanStartPolling（失败3次后切换）`
  - 部分代理/CDN 会剥离 text/event-stream，EventSource 会静默失败。记录失败次数，超过阈值后永久切换到 30s polling，避免反复重连 SSE 的 overhead。同样模式用于 gateway SSE 的软恢复
- **data-* 属性作为 DOM 序列化的持久化边界** — `ui.js:buildToolCard - data-memory-save / data-skill-update；ui.js:ensureActivityGroup - data-activity-event-id`
  - innerHTML snapshot/restore（session 切换时）会丢失 JS 对象属性（如 row._tcData），但保留 data-* attribute。把分类标志镜像到 data-* 使 _syncToolCallGroupSummary 在 restore 后仍能正确计数。这是 'serialize-safe state' 模式
- **inert markup + 事件后绑定（避免 XSS）** — `panels.js:_buildPluginCard - 先 innerHTML 无 onclick，再 querySelector + addEventListener`
  - innerHTML 中插入 plugin.key/tab.path 的 onclick 字符串会面临 JS 字符串注入（HTML escape 不足以防止 '...' 字符串被 })}} 提前关闭）。分离声明与绑定：innerHTML 只生成 inert 元素，用闭包捕获原始值后 addEventListener，完全规避注入
- **localStorage tab order + visibility chips 拖拽重排** — `panels.js:_applyTabOrder / _wireTabChipDrag / _moveTabOrderPanel`
  - sidebar tab 的顺序和可见性存 localStorage，chip 支持 HTML5 DnD 重排，拖拽后 250ms suppress click 防止 dragend 触发 toggle。双 DOM 目标（.rail 和 .sidebar-nav）用同一套数据驱动，保持 desktop/mobile 一致
- **inflight state 紧凑化 + QuotaExceeded 降级** — `ui.js:saveInflightState, _compactInflightState, _writeInflightStateMap`
  - localStorage 上限约 5MB，长会话消息可超出。_compactInflightState 截取最近 N 条消息，字符串截断，多 session 时按 updated_at 排序保留最新的。QuotaExceeded 先清空再只写当前 session，确保 reload 至少恢复一个
- **活动计时器与 DOM 生命周期耦合（timer 跟随 group 节点）** — `ui.js:_startActivityElapsedTimer, _updateActiveActivityElapsedTimer`
  - active turn 的秒级 elapsed timer 不依赖全局定时器，而是检查 group 节点的 isConnected + data-live-activity-current，节点不在 DOM 或 turn 结束时自动 clearInterval。避免 timer 游离导致内存泄漏或空更新
- **懒加载第三方库（js-yaml/mermaid/Prism/KaTeX）** — `ui.js:_loadJsyamlThen, renderMermaidBlocks, highlightCode`
  - CDN 依赖按需注入 script 标签，失败时降级（tree view 降级 raw pre，mermaid 降级 pre）。_loadJsyamlThen 用轮询等待 CDN 加载，避免回调注册时机问题。highlightCode 用 :not([data-highlighted]) 实现增量高亮

### 会话列表 + 消息渲染 (sessions.js + messages.js)

- **竞态 stale 响应守卫：generation 计数 + _loadingSessionId 令牌** — `sessions.js: loadSession, renderSessionList, _loadOlderMessages`
  - 异步链上每个 await 之后都检查 _loadingSessionId !== sid 或 gen !== _renderSessionListGen，确保后发先到的旧响应不覆盖新状态。这是无框架纯 JS 下防止 race 的最轻量方案。
- **两阶段懒加载：metadata(0ms) + messages(defer)** — `sessions.js: loadSession Phase1/Phase2`
  - Phase1 只取 ~1KB 的元数据（messages=0）用于立刻渲染 topbar/模型标签，Phase2 才取消息体；模型 resolve 再 setTimeout(0) 延后。把首屏响应和数据完整性解耦，视觉上切换会话瞬间完成。
- **INFLIGHT 乐观状态机 + 持久化恢复** — `sessions.js: send, attachLiveStream, loadSession INFLIGHT 分支`
  - 发送消息后立刻把 {messages, toolCalls, uploaded} 写入 INFLIGHT[sid] 并 saveInflightState（sessionStorage），页面刷新或切换回来时 loadSession 检测到 INFLIGHT 并走恢复路径，合并 live tail。避免了流式中途刷新丢失已收到 token。
- **流式 markdown 增量渲染：smd streaming parser + rAF 节流** — `sessions.js: _smdWrite, _scheduleRender, _smdNewParser`
  - 使用 smd.min.js 的 parser_write 接口逐字符增量构建 DOM，每次 token 事件只 feed delta，避免 innerHTML 全量替换。rAF 节流 66ms（fade 模式 33ms）防止 60fps 下 GC 压力导致渲染崩溃。
- **流式渐显（Stream Fade）：EWMA 词速估算 + 错开淡入动画** — `sessions.js: _streamFadeNextText, _streamFadeRenderer, _streamFadeAppendOffset`
  - 不是直接显示到达的 token，而是维护可见文本缓冲，每帧按估算词速（22-160 wps）揭示少量词，每词包裹 <span class='stream-fade-word'> 携带 animationDelay。EWMA 平滑突发 token 块，避免整段内容瞬间弹出的廉价感。
- **SSE + HTTP polling 双通道降级架构** — `sessions.js: startApprovalPolling, startClarifyPolling, ensureSessionEventsSSE, startGatewaySSE`
  - 所有实时功能首选 EventSource SSE，SSE 报错后立即切换 HTTP 轮询 fallback（带固定间隔）。SSE 连接恢复自动切回（指数退避）。这样在恶劣网络或不支持 SSE 的代理后都能工作。
- **多维未读状态机：viewed-count + completion-unread 双轨** — `sessions.js: _hasUnreadForSession, _markSessionCompletionUnread, _setSessionViewedCount`
  - viewed-count 记录用户最后看过时的消息数；completion-unread 是流式结束时的特殊标记（解决 polling 检测延迟窗口期）。两者组合确保后台完成的会话总能在列表显示圆点，切换到该会话后立即清除。
- **乐观 UI + 服务端协调的 session 列表合并** — `sessions.js: _mergeOptimisticFirstTurnSessions, upsertActiveSessionForLocalTurn`
  - 发送消息后立即在本地 _allSessions 插入/更新 session 行，并用 _shouldKeepLocalOnlyOptimisticSessionRow 决定保留时间窗口（5s 内）。轮询响应到达后做 3路 merge（local+fetched 按 busy/streaming 状态决策各字段），避免「消息已发却看不到会话」的闪烁。
- **ephemeral 字段携带（_carryForwardEphemeralTurnFields）** — `sessions.js: _carryForwardEphemeralTurnFields, _EPHEMERAL_TURN_FIELDS`
  - 前端在 done 事件后给最后一条 assistant 消息附加 _turnUsage/_turnDuration/_turnTps/_gatewayRouting/_statusCard 等 client-only 字段。任何 S.messages wholesale-replace（reload/force-reload/error recovery）前都先 carry-forward 这些字段，防止使用量徽章在刷新后消失。
- **推理标签多模式解析（ThinkPairs）** — `sessions.js: _thinkPairs, _streamDisplay, _parseStreamState, _splitThinkFromContent`
  - 抽象出 [{open, close}] 数组同时支持 <think>、<\|channel>thought\n、<\|turn\|>thinking\n 三种推理标签格式，提取 thinkingText 和 displayText 做分离渲染；done 时再把 inline think 移入 m.reasoning 字段防止内容膨胀。
- **Handoff 摘要：外部渠道会话的跨轮次上下文切换** — `sessions.js: _checkAndShowHandoffHint, _generateHandoffSummary, _buildHandoffSummaryToolMessage`
  - 对 Telegram/WeChat 等外渠道会话，超过 10 轮后显示提示条，用户点击调后端 AI 生成摘要，摘要以虚假 role:tool（handoff_summary_card）消息插入前端消息列表（不持久化），既利用 tool 渲染 UI 又不污染上下文。
- **FLIP 动画：session 列表行位移** — `sessions.js: _captureSessionReflowPositions, _playSessionRowsReflowFromPositions`
  - 数据更新前快照每行 getBoundingClientRect().top，DOM 更新后计算 delta，用 CSS --session-reflow-offset 变量 + transition 做物理位移动画，用户感知新会话「滑入」而非「突现」。遵循 prefers-reduced-motion。
- **composer 草稿跨 session 持久化** — `sessions.js: _saveComposerDraft, _saveComposerDraftNow, _restoreComposerDraft`
  - 防抖 400ms 写服务器，session 切换前立即同步写（await），恢复时跳过同 session force-reload 场景（preserveActiveInput），防止用户正在输入时被旧草稿覆盖。本地 composer 是权威，跨 session 切换才从服务端恢复。

### 启动引导 + 命令面板 + 工作区 (boot/commands/workspace)

- **三态面板状态机 (closed \| browse \| preview)** — `boot.js _workspacePanelMode / _setWorkspacePanelMode`
  - 比布尔值更精确地表达面板状态：browse 是用户主动打开浏览文件，preview 是文件点击触发，closed 是完全收起。状态转换规则明确（preview 关闭只清预览不关面板、无 session 时 browse 保持 preview 关），避免竞态导致面板闪烁或不一致。
- **Boot 时双轨 flash prevention：内联 script + JS IIFE 接管** — `boot.js _restoreSidebarState / _restoreTabVisibility`
  - index.html 有在 DOM 解析期即执行的内联 script 用 data-sidebar-collapsed 属性控制折叠，防止 JS 加载前的布局闪烁；JS 加载后 IIFE 把该属性促进到 CSS class 体系接管状态。两段代码各自负责不同阶段，分工明确。
- **声明式命令注册表 + handler fall-through** — `commands.js COMMANDS 数组 / executeCommand`
  - 命令以数据（name/fn/noEcho/arg/subArgs）声明，不是散落的 if/else。handler 可返回 false 主动放弃拦截（/reasoning 某些参数要透传给 agent），实现命令层和 agent 层的协议分层。noEcho 字段把「是否回显用户消息」的逻辑收归到注册表，send() 无需知道命令语义。
- **补全来源聚合层 (builtin + skill + agent + path)** — `commands.js getMatchingCommands / getSlashAutocompleteMatches`
  - 补全结果是多个异步来源（内置、技能、agent API、文件系统路径）的联合，通过 seen Set 去重。每个来源带 source 字段，UI 可据此做差异化渲染（badge/样式）。这使补全系统可无限扩展新来源而不改动补全 UI。
- **路径 token 就地补全（不清空整行）** — `commands.js _findComposerPathToken / showCmdDropdown path 分支`
  - 检测光标位置前后的 ~/xxx 路径 token，选中补全后只替换该 token（tokenStart/tokenEnd），行内其他文本保留。这让路径补全在普通消息正文中也能工作，而不只在行首。
- **Busy input mode 策略模式 (queue \| interrupt \| steer)** — `messages.js send() / commands.js cmdQueue/Interrupt/Steer`
  - busy 时有三种行为由 window._busyInputMode 驱动，每种都有对应的显式斜杠命令（/queue /interrupt /steer）可以 override 当前 mode。_trySteer 内部再做一层 fallback（steer endpoint → interrupt+queue），graceful degradation 链完整。
- **工具调用路径变更的 turn 级追踪 + 预览自动刷新** — `workspace.js noteWorkspaceMutationsFromToolCall / refreshOpenPreviewIfMutated`
  - 把工具调用中的文件路径提取和预览刷新解耦：turn 进行中只记录变更集，turn 结束时统一检查是否需要 bustCache 刷新。防止了每次工具调用都刷新预览的频繁闪烁。
- **Artifact 双格式解析（OpenAI tool_calls + Anthropic content block）** — `workspace.js collectSessionArtifacts`
  - 从消息体中提取 artifact 要同时处理两种 API 格式（function.arguments JSON string vs content[].type=tool_use input 对象）。统一规范化为 fakeTc 对象再走同一 _artifactCandidatesFromToolCall 管道，格式兼容在解析层完成，上层无感知。
- **localStorage 与 server 的 reconcile 策略（localStorage 显式值优先）** — `boot.js 主 IIFE 中 appearance reconcile`
  - 新用户没有 localStorage 值时以 server 配置为准；老用户有明确 theme/skin 选择时以 localStorage 为主（防止 autosave 失败后下次刷新回退）；当两者不一致时把 localStorage 值推回 server 保持同步。这是典型的「乐观本地状态 + server 作为 source of truth 兜底」。
- **bfcache 感知的 pageshow 恢复** — `boot.js pageshow listener`
  - bfcache 冻结整个页面状态，包括失效的 SSE 连接和残留的搜索字符串。pageshow 的 event.persisted=true 分支专门处理 bfcache 恢复，重置所有易腐烂的 UI 状态。这是 SPA 里较少见但重要的健壮性处理。
- **Drop handler 组合而非赋值（addEventLitener vs onclick）** — `workspace.js _bindWorkspaceOsUploadDropTarget`
  - 同一 DOM 元素上需要同时响应 OS 文件拖拽（上传）和 workspace tree 内部拖拽（移动），两者必须共存。用 addEventListener 组合而非 ondrop 属性赋值，再用 _isOsFilesDrag() 在每个 handler 内门控，实现互不干扰的并列 drop 处理。
- **versioned model 名匹配的反贪心策略** — `commands.js _bestModelMatch / _looksLikeVersionedModel`
  - 「用户输入 mimo-v2.5，只有 mimo-v2.5-pro 在 catalog」时不应静默选 pro——这可能意味着费用升级。结尾是数字的 versioned query 对更长的变体采用拒绝匹配策略，改用 _nearestModelSuggestion 提示「你是不是想要」。这是命令系统中罕见的反贪心安全设计。

### 终端 + 引导 + 登录 + PWA

- **Supervisor 线程 + 队列串行 spawn** — `api/terminal.py:_spawn_supervisor_loop`
  - ThreadingHTTPServer 每请求一个线程，Linux PR_SET_PDEATHSIG 是 per-thread 的，导致 shell 在线程退出（~10ms）时立即被 SIGHUP 杀死。将所有 Popen 调用转给固定 supervisor 线程，让 shell 的 '父进程' 是长活线程而非短命请求线程，根治这个 bug。代价是 spawn 变串行但 terminal 本来就是低频操作。
- **env allowlist 而非 blocklist 隔离 PTY 环境** — `api/terminal.py:start_terminal _SAFE_ENV_KEYS`
  - PTY shell 是用户可见的交互面，服务器侧的 OPENAI_API_KEY/ANTHROPIC_API_KEY 等必须剥离，allowlist 比 blocklist 安全：新加的 env var 默认不透传，需显式放行，反过来做则新凭据自动泄漏。
- **SSE for PTY 输出，HTTP POST for PTY 输入** — `terminal.js:_connectTerminalOutput + /api/terminal/input`
  - PTY 输出是推式高频流（shell 渲染），用 SSE 单向推；PTY 输入是低频离散事件（按键），用 POST。二者解耦：SSE 断线只影响显示，不影响 shell 进程继续跑；符合 HTTP 语义，浏览器侧 xterm.onData 自然钩入。
- **终端面板折叠态（dock）/ 展开态（expanded）/ 关闭三态状态机** — `terminal.js:TERMINAL_UI.open + TERMINAL_UI.collapsed`
  - 折叠时终端进程和 SSE 连接保持存活（不重建 PTY），只隐藏面板显示 dock 标签条；输出到来时可自动展开（_terminalAutoExpandOnOutput）。这避免了折叠/展开时重建 shell 带来的状态丢失，体验接近原生终端 minimize。
- **CSS 变量 + rAF 三次测量 补偿 transcript 空间** — `terminal.js:_syncTerminalTranscriptSpace`
  - 终端面板的高度影响消息列表的可见区域，直接 padding-bottom 会跳动；用 CSS 变量 + rAF + 420ms setTimeout 三次测量（覆盖 CSS transition 持续期），让消息列表平滑适应，维持底部对齐。
- **引导向导的 inline probe-gate：Continue 按钮 blocking probe** — `onboarding.js:nextOnboardingStep + _runOnboardingProbe`
  - self-hosted provider 配置了不可达 URL 后如果允许跳过验证直接 finish，用户会卡在无响应的模型调用上。probe 同步在 nextOnboardingStep 里执行，probe.status !== 'ok' 时 throw 阻断 step 前进，但 probe 本身有 (provider\|url\|key) 三元组缓存，页面内 Test button 已探成功的情况下 Continue 不会重复请求。
- **引导向导幂等保护：unchanged config + no apiKey → 跳过 POST** — `onboarding.js:_saveOnboardingProviderSetup`
  - OAuth 配置的 provider 如果用户没改任何字段，跳过 POST 防止意外覆盖。这是一个设计决策：onboarding 既是 first-run 也是 reconfigure，幂等保护使得'重新进向导确认设置'不会破坏已有 OAuth 令牌。
- **OAuth 设备码 + 长轮询统一接口** — `onboarding.js:startCodexOAuth / startAnthropicOAuth`
  - Codex 走标准设备码流（user_code + verification_uri + 3s 轮询）；Claude Code 走凭据文件探测（不需要用户操作，先探 → 若无则提示 claude setup-token 再轮询）。两者复用同一套 flow_id + /api/onboarding/oauth/poll 接口，前端轮询逻辑可复用，只是初始 UI 不同。
- **SW 缓存策略：按 URL pattern 分四档** — `sw.js:fetch handler`
  - API/stream/health → 直通（保证实时性）；login 相关 → 永不缓存（防 stale auth 锁死）；navigate → network-first + 降级 cached shell（保证 auth redirect 正常，离线时优雅降级）；shell assets → network-first + cache fallback（版本参数让缓存 miss 在升级时自然刷新）。不是简单 cache-first 或 stale-while-revalidate，是对每类资源特性的精确匹配。
- **PWA 安装态早期检测（bundle 之前执行）** — `pwa-startup.js:syncMode + beforeinstallprompt 拦截`
  - 在主 JS bundle 加载前就同步 CSS class（pwa-standalone/pwa-browser），避免 FoUC（未安装态 UI 短暂闪现）。beforeinstallprompt 被 preventDefault + 存储，让应用自控安装提示时机而非浏览器随机弹。
- **WebAuthn 零依赖实现（passkeys.py 内嵌 CBOR parser + COSE 解析）** — `api/passkeys.py:_Cbor + _public_key_from_cose`
  - 避免引入 webauthn/fido2 大包依赖（Python 生态的 webauthn 包历史上 API 不稳定）。只实现 ES256（alg=-7）子集，够用且可审计；只在 cryptography 包可用时启用，否则返回清晰错误而非 ImportError 崩溃。
- **challenge 防重放：消费即删 + TTL + per-context 数量上限** — `api/passkeys.py:_store_challenge / _consume_challenge / _evict_oldest_challenges`
  - challenge 存文件（非内存）是为了跨进程重启不失效；消费时 pop + 原子写防重放攻击；per-context 上限（8）防止单客户端洪泛 challenge；全局上限（128）防 DoS。
- **登录页连通性 IIFE：区分 VPN 断开与密码错误** — `static/login.js:checkConnectivity`
  - 用户远程访问（Tailscale/VPN）时断网，表单提交会超时，错误信息含糊。IIFE 主动探 /health，不可达时显示'检查 VPN 连接'并禁用表单；恢复后 auto-reload 刷新 auth state。比靠 form submit 超时报错 UX 好很多。

### 页面 DOM 骨架 + 主题/设计 token (hermes-webui)

- **双轴正交主题系统：theme(dark/light/system) x skin(accent palette) 独立控制** — `static/style.css :root / :root.dark / :root[data-skin=X] / :root.dark[data-skin=X]`
  - theme 轴控制所有背景/文字/边框色（--bg/--text/--surface 族），skin 轴只覆盖 --accent 族（5 个变量）。两轴通过 CSS 属性选择器组合，JS 只需写一个 class + 一个 data attribute，N 个 theme 和 M 个 skin 自动组合出 NxM 效果，而不是 NxM 份 CSS。新增皮肤等于只加一段 CSS，零 JS 改动。
- **闪屏预防 FOUC：内联 script 在 head 同步读 localStorage 写 html 元素** — `static/index.html head 的 6 段内联 script`
  - theme/skin/font-size/workspace-panel/sidebar-collapsed 的初始值全在首字节解析时就写入 html 元素，CSS 文件加载时已有正确 class/attribute，页面首绘直接是目标样式。这是 Next.js/Remix 等 SSR 框架解决 hydration 闪白的标准套路，hermes 用纯原生 HTML 实现了同等效果。
- **三段 CSS 选择器分层：:root（全局 token）-> :root.dark（暗色 override）-> :root[data-skin]（accent override）** — `static/style.css 1-400 行`
  - token 层级与覆盖顺序一致：全局 < 暗色 < 皮肤，且皮肤同时处理 :root[data-skin] 和 :root.dark[data-skin] 两个变体，保证皮肤在任何 theme 下都可读。这个设计允许 sienna/catppuccin 等个别皮肤做全量 palette 重写，而 mono/slate 等皮肤只改 3-5 个变量——同一扩展点支持轻量和重量两种皮肤。
- **Rail + Sidebar 双导航：桌面竖向 icon rail + 移动横向 tab bar，共用 data-panel 属性驱动** — `static/index.html .rail/.sidebar-nav，static/panels.js switchPanel()`
  - 两套导航 DOM 结构不同但共用 data-panel 属性，switchPanel() 用 querySelectorAll('[data-panel=X]') 一次命中两者。@media(min-width:641px) 控制 rail 显示、sidebar-nav 隐藏，布局层完全分开但逻辑层零重复。active 状态在 rail 用 ::before 左侧色条指示，在 sidebar-nav 用 border-bottom 指示——同一语义，适配各自容器的视觉语言。
- **CSS 属性驱动动画：JS 只写 html attribute，CSS 处理 transition** — `static/style.css html[data-workspace-panel=closed] .rightpanel，static/boot.js syncWorkspacePanelState()`
  - rightpanel 的展开/收起用 width:0 + opacity:0 + transform:translateX(14px) + border-left-color:transparent 四条过渡实现弹出感，CSS transition 写在 .rightpanel 本身（.24s cubic-bezier(.22,1,.36,1)），JS 只改 html attribute，渲染线程自己驱动动画不阻塞 JS。
- **自托管图标字典 + li() 工厂函数（无 CDN，无 sprite）** — `static/icons.js LI_PATHS 字典 + li() 函数`
  - 所有 Lucide 图标路径以字符串常量存入对象，li() 运行时拼 SVG 字符串。优点：零 CDN 依赖、零 HTTP 请求、可按需扩展。选这个方案是因为 CSP 严格环境下 CDN SVG sprite 会触发 integrity 失败。
- **纯 CSS data-tooltip 自定义 tooltip（data-tooltip 属性 + ::after 伪元素）替代原生 title** — `static/style.css .has-tooltip/.has-tooltip::after/.has-tooltip--bottom/.has-tooltip--left`
  - 原生 title tooltip 有约 1.5s 延迟，hermes 用 CSS ::after + opacity transition(delay:.15s) 实现 150ms 响应。tooltip 内容存在 data-tooltip attribute，CSS 用 content:attr(data-tooltip) 读取，i18n.js 同步本地化值到该 attribute。三个方向变体（右/下/左）通过级联实现，无 JS 定位计算。
- **设置分两层持久化：localStorage（客户端即时、防 FOUC）+ /api/settings（服务端跨设备同步）** — `static/boot.js _applyTheme/_applySkin，static/panels.js saveSettings()`
  - 外观类（theme/skin/font-size）写 localStorage 立即生效，同时异步 POST /api/settings。页面刷新时先从 localStorage 恢复（IIFE），然后 boot 阶段再 GET /api/settings 做服务端覆盖。这个双写模式保证了断网时外观照常切换，联网时跨设备共享。
- **Tab 可见性/顺序在首绘前同步还原（IIFE + boot.js 二次兜底）** — `static/index.html 187 行 IIFE script + static/boot.js _restoreTabVisibility()`
  - Tab 可见性和顺序持久化在 localStorage hermes-webui-hidden-tabs/hermes-webui-tab-order，用户拖拽排序立即存。两处还原保证不同时序下都不会看到错误布局：HTML 里的 IIFE 在 paint 前同步执行（防闪），boot.js 里的 IIFE 是后备。
- **字体大小分档覆写（非 rem 缩放）：直接覆盖各组件 px 值** — `static/style.css 29-84 行 :root[data-font-size] 属性选择器`
  - 因为 stylesheet 里大量组件用硬编码 px（历史包袱），改 :root font-size 只影响少数 em/rem。hermes 改用分档精确覆写：session-item/msg-body/各级标题/code/table/textarea/file-item 逐一给 small/large/xlarge 写 override。代码量大但语义精确，不会误伤用了 px 的第三方组件。

### api 会话引擎（生命周期/事件/恢复/流式）

- **Monotonic generation counter + segment ownership（session_lifecycle）** — `api/session_lifecycle.py`
  - session 不是不可变的，可多次 reopen。用 generation 计数而非时间戳解决并发 commit 顺序问题；segment 把多 turn 打包为可重试单元，agent 重建不覆盖旧 segment 归属。这是比「锁住整个 session」更细粒度的并发控制：commit 序列化靠 in_flight guard + Condition，而 turn 追加无需等待 commit 完成。
- **Coalescing bounded-queue pub/sub（session_events）** — `api/session_events.py`
  - SSE 长连接用 maxsize=1 Queue 承载失效事件，防止慢消费者无限堆积。关键设计：队列满时不丢最新也不丢最旧，而是 coalesce 合并——同 profile 直接替换（幂等），不同 profile 合并为无作用域广播（保证任何 tab 都能刷新）。避免了 profile 隔离带来的事件丢失问题。
- **双层锁顺序 + stale-object guard（session_ops）** — `api/session_ops.py`
  - agent_lock → LOCK 的固定顺序防死锁。get_session() 有缓存竞态：两个并发调用可能缓存不同实例，第二个会覆盖第一个——必须在持 LOCK 后重新从 SESSIONS dict 取规范实例。这个 stale-object guard 模式是多层缓存系统中保证写到正确对象的标准解法。
- **Multi-source 崩溃恢复 + 分类修复（session_recovery + session_discoverability）** — `api/session_recovery.py, api/session_discoverability.py`
  - session 有四个并行存储（JSON sidecar/index/state.db/API 内存），任何一个的故障都可能让 session「消失」。恢复体系分三层：(1) 启动时 .bak 恢复；(2) state.db 反向重建 sidecar；(3) 四源交叉审计 + 分类（repairable vs unsafe_to_repair）。所有修复都是原子写（tmp + rename/link），fail-open（DB 异常时假设存在），dry-run 模式先 plan 不 apply。
- **Source normalization 作为稳定 API 契约（agent_sessions）** — `api/agent_sessions.py`
  - raw source 字符串散布在 state.db 的多个表里，不同版本 schema 可能缺列。normalize_agent_session_source 提供一个稳定的 {session_source, source_label} 契约，把路由决策从「解析 raw 字符串」变成「匹配枚举值」。sidebar policy 只对 session_source 做决策，与 raw 值解耦。
- **压缩链折叠 + 活跃度排序覆盖（agent_sessions）** — `api/agent_sessions.py:_project_agent_session_rows`
  - LLM context compression 会产生 parent→child session chain，sidebar 不应把它们显示为多个条目。折叠时保留 root 的 title/started_at（user 识别锚点），但用 tip 的 last_activity 覆盖排序键——不然长期活跃的压缩链会被老 timestamp 埋到列表深处。
- **Eager stream release + writeback guard（streaming）** — `api/streaming.py:cancel_stream, _stream_writeback_is_current`
  - 取消时立即清除 active_stream_id（不等 worker 真正退出），让 UI 立刻可以发新请求。但这带来竞态：被取消的旧 worker 完成时不能再写回。_stream_writeback_is_current 用 active_stream_id 匹配作为写回门控。_stream_writeback_can_supersede_recovery_marker 再开一个窄口：如果 recovery 路径误插了 interrupted 标记，真正完成的 worker 可以替换它。三个函数共同保证「最后写回者即当前所有者」。
- **Append-only JSONL journal + seq cursor + synthetic event（run_journal）** — `api/run_journal.py`
  - 每个 stream 一个 JSONL 文件，事件不可变追加，seq 编号允许客户端游标增量拉取（类似 Kafka offset）。worker 消失时 stale_interrupted_event 合成一个 synthetic apperror 事件填补 gap，而不是修改 journal——append-only 不变性得以保持，恢复逻辑在读取层而非写入层。
- **Turn journal vs Run journal 分层（turn_journal + run_journal）** — `api/turn_journal.py, api/run_journal.py`
  - Turn journal 记录 user 发起的 turn 边界（submitted/completed/interrupted），粒度是「一次用户意图」，用于 session recovery 和 retry。Run journal 记录 SSE 流中每一帧事件（seq 编号），粒度是「一次 agent 执行流」，用于 client replay 和 worker-crash 检测。两层分工：turn journal 驱动 session 修复，run journal 驱动客户端恢复。
- **Explicit profile threading（state_sync）** — `api/state_sync.py`
  - worker thread 中 TLS profile context 不传播（Python threading 的标准坑）。解法：调用方必须显式传 profile= 参数，_get_state_db 拒绝使用 TLS fallback。这把「传错 DB」的风险从运行时静默错误变为显式拒绝（return None）。是 dependency injection 在线程安全场景的具体应用。
- **LRU agent cache + session boundary lifecycle（streaming）** — `api/streaming.py:_close_evicted_agent_at_session_boundary`
  - AIAgent 对象被 LRU 缓存以保留 memory provider state（避免每 turn 重建）。eviction 时必须按序执行：commit → unregister → discard → shutdown_memory_provider → close session_db。任何步骤失败都不继续后续（保守关闭）。discard 在最后清理 lifecycle dict entry 防 #3506 无限增长。

### agent_health / gateway_chat / gateway_watcher / runner_client / runtime_adapter / route_approvals / clarify / compression_anchor / request_diagnostics / system_health

- **alive 三态设计（True / False / None）** — `api/agent_health.py:build_agent_health_payload`
  - 将「宕机」和「未配置」强制区分为不同语义：True=确认存活，False=确认宕机，None=未知/未配置。避免「无网络→显示宕机」的假阳性，前端可按此决定是否显示警告 banner。swarmx worker 的 health 展示同样可借用，区分 worker hang（None）和 worker exit（False）
- **多探测路径 fallback（PID → 文件新鲜度 → 远程 HTTP 多路径）** — `api/agent_health.py`
  - 跨容器 / 跨进程部署时 PID namespace 隔离导致 os.kill 失效，用 gateway_state.json 的 updated_at 作等价活跃信号。远程部署时再 fallback 到 HTTP probe 遍历 /health/detailed → /health → /v1/health。每一层都有明确的失效边界，互不耦合
- **Protocol + 三种 RuntimeAdapter 实现（直通/日志/runner-local）** — `api/runtime_adapter.py`
  - 用 Python Protocol 定义 8 方法接口作为缝合点，三种实现完全解耦：legacy-direct 返回 None 完全绕开，legacy-journal 注入 delegate callable（不持有状态），runner-local 注入 HTTP client。关键原则：adapter 只做翻译，不持有 worker 线程/queue/flag，防止 facade 膨胀
- **Approval / Clarify 对称 FIFO 队列 + SSE 推送（在锁内通知）** — `api/route_approvals.py, api/clarify.py`
  - 每个 session 维护一个 per-session list（非单值），支持并发审批（#527）。通知逻辑必须在持锁内调用，防止两个并发 submit 的 notify 乱序。Clarify 额外用 threading.Event 阻塞 agent 线程等待响应，实现同步等待+异步 SSE 推送双模
- **两阶段变更检测（cheap fingerprint → expensive projection）** — `api/gateway_watcher.py:_cheap_change_fingerprint + GatewayWatcher._poll_loop`
  - 先做仅扫 sessions 表的轻量指纹，不变则完全跳过 JOIN messages 的昂贵投影。针对 replace_messages 场景追加 per-session message 聚合弥补单表指纹的盲区。适合任何「高频轮询 + 低变更率」的后台监控
- **慢消费者队列满时主动踢除 + sentinel 唤醒** — `api/gateway_watcher.py:GatewayWatcher._notify_subscribers`
  - put_nowait 失败（queue.Full）则把该订阅者标记为 dead，发 None sentinel 让 SSE handler 感知并关闭连接，浏览器 EventSource 自动重连。避免无界增长，把背压直接转为重连，不丢失最终一致性
- **HTTP client 禁用重定向 + scheme 白名单校验** — `api/runner_client.py:HttpRunnerClient._opener + __init__`
  - 防止配置错误的 URL（file:///etc/passwd）或恶意 runner 用 3xx Location 窃取 Bearer token。这是内部 HTTP client 的安全基线，在无沙箱环境（如 swarmx 直接 HTTP 调用外部 runner）中尤为重要
- **压缩锚点 auto_compression flag 区分两套规则** — `api/compression_anchor.py:visible_messages_for_anchor`
  - 手动压缩和自动压缩对「哪些消息算有效内容」的定义不同（provider-style 消息类型），用 flag 而非两套函数避免重复，且调用方必须显式声明语境，防止规则误用
- **请求级 watchdog：阶段计时 + 超时时快照全线程栈** — `api/request_diagnostics.py:RequestDiagnostics`
  - 对延迟敏感路径挂一个 threading.Timer，超时前 cancel() 正常路径不打印，超时则捕获所有线程栈帮助定位卡点。stage() 方法细粒度标记各处理阶段，报告包含各段 ms 用于定界
- **无 psutil 的系统指标采集（/proc/stat + /proc/meminfo + shutil）** — `api/system_health.py`
  - 不引入重依赖，纯 stdlib 实现 CPU/内存/磁盘采集。各指标独立 try/except，一个失败不影响其他。返回三态 status（ok/partial/unavailable），适合嵌入式/轻量化部署
- **Gateway chat bridge 保持 WebUI 内部事件协议不变** — `api/gateway_chat.py:_run_gateway_chat_streaming`
  - /api/chat/start 仍返回本地 stream_id，/api/chat/stream 仍发 WebUI SSE 事件名（token/tool/done/apperror 等），后台 worker 把 OpenAI-compatible Gateway SSE 翻译为这些事件。浏览器无感知后端切换，可复用现有前端代码

### kanban/goals/metering/workspace-git/worktree

- **Bridge 模式：WebUI 不拥有数据库，只桥接 hermes_cli.kanban_db** — `api/kanban_bridge.py`
  - kanban_db 是唯一数据源，WebUI 层只是 REST/SSE 适配层；延迟导入+503 graceful degradation 让 WebUI 可以独立部署
- **三值 HTTP 分发返回约定（True/None/False = 成功/已响应/未匹配）** — `api/kanban_bridge.py:handle_kanban_get/post/patch/delete`
  - 同步 BaseHTTPServer 下无法用异常做 404 路由，三值约定让路由器链式 dispatch 同时能区分「路径不匹配」和「已响应错误」
- **SSE cursor + Last-Event-ID 断线续传** — `api/kanban_bridge.py:_handle_events_sse_stream`
  - 每 events 帧发 id: <cursor>，浏览器 EventSource 在重连时自动附带 Last-Event-ID；服务端优先读 Last-Event-ID 作为 cursor fallback，实现无 JS 代码的断点续传
- **claim_lock 协议保护 running 状态，直接写 running 被 HTTP 400 拦截** — `api/kanban_bridge.py:_patch_task, _set_status_direct`
  - running 状态的进入由 dispatcher/claim_task() 独占，UI 只能 block/unblock/complete/triage/ready；离开 running 时清 claim 字段防止 dispatcher phantom-running
- **Goal 状态机（active/paused/done/cleared）+ 每轮 judge + turn budget** — `api/goals.py:_ProfileGoalManager`
  - 用 LLM judge_goal 判定目标达成，budget 耗尽自动 pause（非 clear），resume 可重置 turns_used；状态持久化到 profile 的 state.db，多 profile 并发安全
- **snapshot/restore 幂等保护：stream 创建前快照 goal state，失败后回滚** — `api/goals.py:goal_state_snapshot, restore_goal_state`
  - 防止 kickoff 流创建失败时 goal 被错误激活；使用 copy.deepcopy + _save() 实现原子性，不依赖事务
- **Todo 状态双路同步：SSE 实时路径 + session GET 冷加载路径** — `api/todo_state.py`
  - 两路均通过 _normalize_snapshot 规范化，保证前端只需一种 deserialization 契约；侦测逻辑与 agent 侧 _hydrate_todo_store 完全对称，防止 panel 与 agent 状态分叉
- **TPS 计量：per-stream 计量 + 全局平均 + 60min rolling HIGH/LOW** — `api/metering.py:GlobalMeter`
  - idle 时不记录（防止 idle 稀释 HIGH/LOW）；get_interval() 让 ticker 在无流时自动降频到 10s 退出，节省 CPU
- **Profile 隔离 workspace：per-profile workspaces.json + last_workspace.txt** — `api/workspace.py`
  - clean 时主动过滤跨 profile 泄漏路径，并持久化清洁版；migration path 一次性迁移 legacy global file
- **TOCTOU 防御：openat + O_NOFOLLOW 逐组件 anchored 打开** — `api/workspace.py:open_anchored_fd, open_anchored_create_fd 等`
  - safe_resolve_ws 只做静态验证，之后的文件操作必须重走 anchored fd 链，防止 resolve 后 symlink 竞态替换；Windows 无 dir_fd 时优雅退化
- **Git 操作：环境清洗 + repo-root 级互斥锁 + 分类错误码** — `api/workspace_git.py`
  - GIT_DIR/GIT_CONFIG_* scrub 防止宿主环境污染；lock by repo_root 而非 workspace（共享 repo 多 worktree 不互锁）；错误码结构化让前端能渲染「push 被拒绝 - 请先 pull」等精确提示
- **临时 GIT_INDEX_FILE 实现 selected-file commit** — `api/workspace_git.py:_selected_temp_index_env`
  - mkstemp + GIT_INDEX_FILE env 创建隔离 index，add → diff → commit 全在临时 index 中，正常 index 零污染；finally 确保删除临时文件
- **stash-and-checkout + 自动恢复目标分支 stash** — `api/workspace_git.py:git_stash_and_checkout`
  - stash 名带 'hermes-webui branch switch to X' 标记，切换后扫描 stash list 找匹配的恢复，提供类 IDE 的「切分支自动保存/恢复工作区」体验
- **Worktree 删除前多重守卫（stream lock / terminal lock / dirty / unpushed）** — `api/worktrees.py:remove_worktree_for_session`
  - 分层守卫：非 force 时 dirty/unpushed 都 block；force 时改为 warning 而非拒绝；删完后 git worktree prune 清理悬空引用
- **Checkpoint 用 shadow git repo 存储，id 白名单 + workspace allowlist 双重安全** — `api/rollback.py`
  - checkpoint dir = SHA-256(realpath(workspace))[:12]，与 agent 逻辑完全对齐保证目录可找到；checkpoint id 正则白名单防路径遍历；restore 只 overwrite 已有文件不删新文件（保守策略）
- **Background task tracking：内存 dict，get_results 只移除 done 保留 running** — `api/background.py`
  - running 任务不移除防止 complete_background 找不到导致结果丢失；设计上接受内存丢失（进程重启后 running 任务不可恢复）

### 整体架构(server/mcp/bootstrap/routes/启动/文档)

- **薄壳路由 + 业务分层（server.py = shell，api/ = 业务）** — `server.py / api/routes.py`
  - server.py 只做 HTTP 机制(ThreadingHTTPServer、TLS、auth middleware、CSP 头、连接参数)，所有业务在 api/ 模块。routes.py 是扁平 if/elif 分发无框架——刻意选择无依赖，保持'agent 可在终端修改'的哲学。swarmx 的 axum router 已天然满足这个分层，但 routes 分层清晰度可借鉴
- **多策略级联探测（agent dir / python / workspace）** — `bootstrap.py:discover_agent_dir, api/config.py:_discover_agent_dir`
  - 对每个外部依赖(agent、python解释器、workspace)都维护一个有序候选列表，env var > 协议约定目录 > sibling > 系统路径，最后兜底用 shebang 反向解析。失败时输出具体 fix-it env var。这是无注册表分布式安装的标准模式；swarmx 的 agent 目录探测、HERMES_HOME 等类似问题可参考
- **MCP server 直接 import 内部模块（绕过 HTTP 层）** — `mcp_server.py`
  - MCP server 和 web server 共用同一 Python 进程空间的模块（api.models, api.profiles），只有需要 cache 同步的写操作才走 HTTP API。原则：只读操作直接文件访问；写操作必须经过 HTTP 以保证内存 SESSIONS 缓存一致性。这是'同进程 MCP'的关键设计——swarmx 的 MCP 管理页已独立进程，反而需要关注这个 cache drift 问题
- **Profile-scoped 线程本地上下文 + worker thread 显式 profile_home 传递** — `api/config.py:get_config_for_profile_home, api/profiles.py`
  - HTTP handler thread 通过 cookie 设置 thread-local active_profile，但 SSE worker thread 不继承。解法：把 session 的 profile_home 显式传给 worker，用 get_config_for_profile_home() 按路径直读 YAML，不污染全局缓存，race-free。swarmx 的多方向(thread)设计有同样问题：per-request profile 不会自动传给 spawned worker
- **PBKDF2-600k + HMAC 签名 cookie + CSRF 派生 + 懒过期清理** — `api/auth.py`
  - auth 完全 stdlib 无外部依赖：PBKDF2 hash 进程内缓存(double-checked locking)、签名 cookie 含版本兼容旧截断签名、CSRF token = HMAC(key, 'csrf:'+session_token)无需存储、IP 速率限制持久化到文件、Passkey 默认 off。这套方案是'零依赖本地优先工具'鉴权的标杆，swarmx 目前无鉴权是已知短板
- **启动序列健壮化：端口检测 + FD limit + 权限修复 + session recovery + 依赖安装** — `server.py:main, api/startup.py`
  - 启动前先 probe /health 防双实例；提升 RLIMIT_NOFILE 防 macOS launchd 限制；修 sensitive file 权限；从 .bak 恢复损坏 session；按需 pip install agent 依赖。每一步独立 try/except 不阻断启动。swarmx 启动检查目前较简单，可借鉴此顺序
- **测试时网络隔离 monkey-patch（socket level）** — `server.py 顶部的 HERMES_WEBUI_TEST_NETWORK_BLOCK`
  - 不需要 mock 框架：在 server.py import 时检查 env var，直接替换 socket.create_connection 和 socket.socket.connect，只允许 loopback/RFC1918 地址。测试配置中 conftest.py 设置 env var，server 进程自动隔离出网。swarmx 的 harness-check 可用类似思路做测试时的 PTY/CLI 隔离
- **RuntimeAdapter Seam（协议接口 + 三种执行模式）** — `api/runtime_adapter.py`
  - 用 frozen dataclass 定义 StartRunRequest/RunStartResult 等协议类型，legacy-direct / legacy-journal / runner-local 三种模式由 env var 选择。这是'在不破坏现有代码路径下插入新抽象'的标准 seam 模式。swarmx 的 PTY worker 和未来可能的 ACP/native SDK worker 之间也需要类似 seam
- **廉价指纹 + 昂贵 projection 双层检测（gateway watcher）** — `api/gateway_watcher.py:_cheap_change_fingerprint`
  - 后台轮询先做 O(sessions) 无 JOIN 的指纹计算，只有指纹变化才做 O(sessions × messages) 的完整 projection。这是轮询场景下的标准性能优化：cheap gate → expensive compute。swarmx 的 transcript.rs tail 广播工具级活动有类似问题，当大量 agent 并发时全量读 JSONL 代价高
- **原子文件写入模式（tempfile + os.replace + chmod）** — `api/auth.py:_save_sessions, api/passkeys.py:_atomic_write_json`
  - 所有敏感状态文件（sessions, passkeys, login_attempts）统一用 mkstemp + write + chmod 0600 + os.replace。crash-safe（内核保证 replace 原子性）+ 权限安全。这是文件系统持久化的 best practice，swarmx 的 settings.json/blackboard 写入可参考
- **Supervisor 自动检测 + foreground/detached 双模启动** — `bootstrap.py:_detect_supervisor`
  - 通过 INVOCATION_ID/JOURNAL_STREAM/NOTIFY_SOCKET/XPC_SERVICE_NAME 检测 systemd/launchd，XPC_SERVICE_NAME 做 noise filter（Terminal shell 也会设置）。前台模式用 os.execv 替换进程（supervisor 看到原始 child），Windows 改 Popen+exit。swarmx 的 ctl.sh 等效，但无自动 supervisor 检测

## 2. 函数级地图（穷举）

### 前端 UI 渲染骨架 ui.js + panels.js  ·  108 项

- `ui.js:S` — 单例全局状态对象，持有 session/messages/entries/busy/pendingFiles/toolCalls/activeStreamId/currentDir/activeProfile/todos/todoStateMeta 等运行时状态，是整个前端的单一数据源
- `ui.js:assistantDisplayName` — 返回 bot 显示名，优先用 activeProfile 驼峰化，fallback window._botName 或 'Hermes'
- `ui.js:initOfflineMonitor` — 入口：patch window.fetch + 监听 visibilitychange，离线时显示 banner 并启动 2.5s 探针定时器
- `ui.js:_patchOfflineFetch` — Monkey-patch window.fetch：TypeError 时自动 probe /health，失败则展示 offline banner；保留原始 fetch 供 probe 使用，避免无限递归
- `ui.js:_recoverFromOfflineSoftly` — 软恢复：隐藏 banner → 重启 gateway SSE → 重拉 session，仅在失败时才 hard reload；避免 Android PWA 后台化时的频繁全页刷新
- `ui.js:showOfflineBanner / _hideOfflineBanner` — 带状态机的 offline banner 显隐，区分 browser/network 两种原因，控制'Check now'按钮的 disabled 状态
- `ui.js:renderProviderQuotaIndicator` — 在 composer 区渲染 provider quota pill：status/用量/重置时间，支持 exhausted/warning/ok 三态颜色
- `ui.js:renderModelDropdown` — 完整的模型选择浮层：按 Configured/Provider 分组，带实时搜索过滤 + 自定义 model ID 输入框；每次打开时重建 DOM，选中后写 localStorage
- `ui.js:_filterModels` — renderModelDropdown 内嵌闭包，对 _modelData 按名称/ID 子串过滤后重建 dropdown DOM，并去重 Configured 模型（语义 provider::modelKey）
- `ui.js:_reconcileModelDropdownSelection` — 在 provider 列表刷新后协调下拉选中状态：优先级为 session.model > previousState > profile 默认
- `ui.js:_readPersistedModelState / _writePersistedModelState` — localStorage 持久化 {model, model_provider}，兼容旧版 hermes-webui-model key
- `ui.js:_rememberPendingSessionModel / _readPendingSessionModel` — sessionStorage TTL=10min 暂存'新建 session 前用户选的 model'，解决切换 session 时模型被误重置的竞态
- `ui.js:syncModelChip` — 同步 composer 中 model chip 的标签、颜色、provider badge 显示，调用 getModelLabel + _getConfiguredModelBadge
- `ui.js:toggleModelDropdown / closeModelDropdown` — 协调 model/reasoning/toolsets/profile/workspace 等多个 dropdown 的互斥关闭逻辑
- `ui.js:_applyReasoningOptions / fetchReasoningChip / toggleReasoningDropdown` — reasoning effort 选择器：从 API 拉支持的 effort 列表，渲染 chip + dropdown，选择后 patch session
- `ui.js:_applyToolsetsChip / _populateToolsetsDropdown / toggleToolsetsDropdown` — toolsets 选择浮层：多选 checkbox 列表，选中后 patch session.toolsets
- `ui.js:renderMd` — 核心 Markdown 渲染器，10000+行无框架实现：stash pipeline（blockquote→MEDIA→fence→math→rawPre），对 diff/csv/json/yaml/mermaid/svg/html 代码块特判输出 HTML，支持 KaTeX inline math，递归渲染 blockquote
  - 💡 用 \x00X{n}\x00 stash token 而非 DOM 操作，避免多 pass 之间正则互相干扰；blockquote 先整体提取再递归调 renderMd 本身
- `ui.js:_getCachedRender` — 为 renderMd 和 _renderUserFencedBlocks 提供 Map 缓存，key = role:'u'/'a' + 内容（长文本用 length+prefix+suffix），上限 300 条
- `ui.js:renderMessages` — 主消息渲染循环：DOM windowing（默认 50 条）、session HTML cache（sid→{html,msgCount}）、compression anchor 插入、jump-to-question 索引建立、date separator、assistant turn 合并
  - 💡 有两级缓存：per-message renderMd cache 和 per-session innerHTML cache；streaming 时跳过 innerHTML cache 防止覆盖 live smd DOM
- `ui.js:_visWithIdxCache / clearVisibleMessageRowCache` — visWithIdx 数组（过滤后消息+原始索引）的惰性缓存，仅在 messages.length 变化时重建
- `ui.js:_messageRenderCacheSignature` — 消息渲染签名（消息数+toolCalls+压缩状态+模型+会话ID等）用于判断 session HTML cache 是否过期
- `ui.js:_showEarlierRenderedMessages` — 加载更早消息：扩大 renderWindowSize 并保持滚动位置（保存 scrollHeight 差值）
- `ui.js:jumpToSessionStart / jumpToTurnQuestion` — 跳转导航：jumpToSessionStart 先 ensureAllMessagesLoaded 再滚到顶部；jumpToTurnQuestion 先展开 window 再 scrollIntoView + 闪烁高亮
- `ui.js:buildToolCard` — 构建工具调用卡片 DOM：icon/name/preview(args)/detail/diff 展开；按 data-memory-save/data-skill-update 标注类型（data-* 存活于 innerHTML 序列化）
- `ui.js:_formatToolArgPreview / _toolArgPreviewKeyIsHidden` — 将 tool.args 格式化为折叠摘要，自动隐藏含 apikey/token/secret/password 等子串的参数名，优先展示 path/query/url/command 等语义键
- `ui.js:appendLiveToolCard / clearLiveToolCards` — 流式推送阶段实时插入/清除工具卡片，避免 renderMessages 全量重建
- `ui.js:ensureActivityGroup / _appendActivityEvent / _syncToolCallGroupSummary` — Live activity disclosure group：每个 turn 一个 summary/details 元素，事件以 data-activity-event-id 去重更新，折叠状态持久化到 localStorage
- `ui.js:appendThinking / finalizeThinkingCard / removeThinking` — 思考过程卡片生命周期：流式追加 → finalizeThinkingCard 转为折叠 details；支持 strip visible echo（避免 thinking 内容与回答重复显示）
- `ui.js:_buildTreeDOM` — 递归构建 JSON/YAML 的可折叠 Tree view DOM，depth>=2 默认折叠，叶节点按 null/boolean/number/string/array/object 分别着色
- `ui.js:initTreeViews` — 懒初始化代码块 Tree view：JSON 直接 parse，YAML 懒加载 js-yaml CDN（失败降级原始文本），短块(<10行)默认 raw view
- `ui.js:postProcessRenderedMessages` — 消息渲染后处理管线：highlightCode → addCopyButtons → loadDiffInline → loadCsvInline → loadExcalidrawInline → loadPdfInline → loadHtmlInline → renderMermaidBlocks → renderKatexBlocks → initTreeViews
- `ui.js:highlightCode` — 增量 Prism.js 高亮：只处理 :not([data-highlighted]) 的 code 块，避免 rerenderMessages 重复高亮
- `ui.js:addCopyButtons` — 为所有 pre 块注入 Copy 按钮，已有按钮的跳过；复制成功后文字改'Copied!'并 1s 后还原
- `ui.js:loadDiffInline / loadCsvInline / loadExcalidrawInline / loadPdfInline / loadHtmlInline` — 内联渲染扩展：diff 着色、csv 表格、Excalidraw SVG、PDF embed、HTML sandbox iframe
- `ui.js:renderMermaidBlocks` — 懒加载 mermaid 库并渲染 .mermaid-block 块；已渲染的跳过，失败时降级 pre
- `ui.js:renderKatexBlocks / _isStreamingEquationPending` — KaTeX 渲染 .math-display/.math-inline，流式时 abort 部分公式避免报错
- `ui.js:_openImgLightbox / _openImgLightboxWithNav / _navigateLightbox / _closeImgLightbox` — 图片灯箱：overlay 全屏 + 键盘左右导航 + ESC 关闭，支持多图 prev/next
- `ui.js:_mediaPlayerHtml / _mediaSpeedControlsHtml / _initMediaPlaybackObserver` — 媒体播放器：audio/video tag + 倍速按钮（0.5x-2x），倍速存 localStorage，用 IntersectionObserver 懒初始化速率
- `ui.js:setBusy / updateSendBtn / getComposerPrimaryAction` — composer 状态机：busy=true 时按钮变 Stop，busy=false 时按钮按 session.busy/inflight/queue 决定 Send/Stop；还处理 session queue drain
- `ui.js:setCompressionUi / clearCompressionUi / appendLiveCompressionCard / renderCompressionUi` — 上下文压缩 UI：running 阶段显示 live card + elapsed timer，settled 阶段转为 summary card + reference card；支持 auto/manual 两种模式
- `ui.js:_compressionAnchorIndex` — 定位压缩摘要卡应插入的 visWithIdx 位置：优先按 anchorMessageKey（message._compression_anchor）匹配，fallback 按 anchorVisibleIdx
- `ui.js:saveInflightState / loadInflightState / clearInflightState` — browser reload 恢复机制：流式数据紧凑化后存 localStorage（每 session 限 maxMessages 条+maxToolCalls 条），TTL 10min，QuotaExceeded 时降级只存当前 session
- `ui.js:scheduleTodosRefresh / _todosHash` — todos 渲染防抖：用 requestAnimationFrame 合并同帧多个 todo_state SSE 事件，_todosHash 用 id+content+status 串联指纹避免重复渲染
- `ui.js:_hydrateTodosFromSession` — 冷加载 todo_state：优先读 session.todo_state（新服务端），fallback 反向扫 messages 提取 todo 工具调用（兼容旧服务端）
- `ui.js:snapshotLiveTurnHtmlForSession / restoreLiveTurnHtmlForSession` — session 切换时 snapshot/restore live turn 的 innerHTML，维持流式内容在回切后不丢失
- `ui.js:syncTopbar` — 顶栏同步入口：更新 document.title / topbarTitle / modelSelect / profileChip / reasoningChip / toolsetsChip / workspace / terminalButton；处理 session.model 缺失时的 fallback 逻辑和 resolve_model 竞态
- `ui.js:renderBreadcrumb` — 文件树面包屑：每段路径都可 drag-drop（_bindWorkspaceMoveDropTarget）和 OS file 上传（_bindWorkspaceOsUploadDropTarget）
- `ui.js:_renderTreeItems` — 递归渲染文件树：dir 展开/折叠（S._expandedDirs Set），文件单击 debounce 300ms 区分单/双击（单击开文件，双击重命名），右键 context menu，拖动改路径
- `ui.js:_showFileContextMenu` — 文件右键菜单：Rename/Copy path/Copy name/Download/Delete，动态构建 fixed position div，点击外部关闭
- `ui.js:showToast / setToastDismissTimer / clearToastDismissTimer` — 全局 toast：默认 2.8s 消失，error 型 20s，支持复制 toast 文本
- `ui.js:showConfirmDialog / showPromptDialog / _ensureAppDialogBindings` — Modal 对话框：Escape/Enter 键处理，焦点陷阱（focusCancel 选项），Promise resolve；同时只能有一个 dialog
- `ui.js:speakMessage / stopTTS / _playEdgeTtsChunked / _splitForTTS` — TTS：优先 Edge TTS API chunked streaming，fallback SpeechSynthesis；按句子/段落切分避免浏览器 utterance 长度限制
- `ui.js:renderFileTree` — 文件树渲染入口：从 S.entries/_dirCache 构建树，隐藏 .DS_Store/node_modules/.git 等，支持 showHiddenFiles toggle
- `ui.js:renderTray` — composer 文件 tray：图片显示缩略图，非媒体文件显示 paperclip chip，超大文件显示 warning
- `ui.js:_renderUserFencedBlocks` — 用户消息中代码块渲染：仅 fence 块内容，不做完整 markdown 管线，避免用户 input 中 **bold** 等被渲染
- `ui.js:_statusCardHtml` — 状态卡片 HTML：用于渲染 agent 发回的 status_card 对象，含 title/subtitle/rows 列表 + session_id copy button
- `ui.js:_setMessageScrollToBottom / scrollIfPinned / scrollToBottom / _settleMessageScrollToBottom` — 消息区滚动管理：_scrollPinned 标志追踪用户是否在底部，流式时 requestAnimationFrame 批量 settle，避免频繁 reflow
- `ui.js:_syncCtxIndicator / _mergeUsageForCtxIndicator / _setCtxCompressButton` — 上下文 token 指示器：input+output+cache 组合成 progress bar，接近上限变红，显示 compress 按钮
- `ui.js:startSystemHealthMonitor / renderSystemHealth / setSystemHealthUnavailable` — 系统健康监控：CPU/内存/磁盘 5s 轮询，progress bar 渲染，panel 不可见时停止轮询
- `ui.js:startAgentHealthMonitor / _showAgentHealthAlert / dismissAgentHealthAlert` — Agent 健康告警：30s 轮询，检测 stalled/error agent，显示 dismissible banner，用 localStorage 记录 dismiss 状态
- `ui.js:_renderUpdateWhatsNewLinks / _showUpdateBanner / dismissUpdate` — 版本更新 banner：解析 update diff links，显示版本号+更新链接，持久化已生成的 summary 避免重复调 API
- `ui.js:editMessage / cancelEdit / autoResizeTextarea` — 消息内联编辑：点击 Edit 替换气泡为 textarea，Submit 时 truncate+重发，Cancel 还原 DOM
- `ui.js:_syncMobileComposerConfigButton / closeMobileComposerConfig / toggleMobileComposerConfig` — 移动端 composer 配置折叠面板（model/reasoning/toolsets/workspace 等控件），小屏下收起
- `panels.js:syncAppTitlebar` — 顶部 titlebar：chat panel 时显示 session title + message count + source badge；双击 title 启动 inline rename（InputElement 替换 span，防止 MutationObserver 递归）
- `panels.js:_beforePanelSwitch` — panel 切换前守卫：settings dirty 时阻断并记录 _pendingSettingsTargetPanel，触发 unsaved bar
- `panels.js:switchSettingsSection` — Settings 子页切换：conversation/appearance/preferences/providers/plugins/system，同步 sidebar menu active 类、pane active 类和 mobile dropdown 选值；lazy load providers/plugins
- `panels.js:_renderCronDetail / _setCronHeaderButtons / openCronDetail / _clearCronDetail` — Cron 任务详情：左-主布局，detail card 含状态/下次运行/上次运行/schedule/model；header buttons 按 mode(read/create/edit/empty) 切换显隐
- `panels.js:_loadCronDetailRuns / _loadRunContent` — 异步加载 cron 运行历史：最近 50 条，展开时加载 content，用 renderMd 渲染 markdown，显示 usage strip
- `panels.js:_renderCronForm` — Cron 创建/编辑表单：name/schedule(cron expr)/prompt/deliver/profile/toast_notifications/no_agent/script，内含 cron expr 实时解析+提示
- `panels.js:_renderCronSkillTags / _bindCronSkillPicker` — Cron 关联 Skills 多选：tag chip 显示 + 搜索过滤，选中后存入 _cronSelectedSkills
- `panels.js:openCronCreate / openCronEdit / duplicateCurrentCron` — Cron CRUD 入口；duplicate 智能去重名（'(copy)' → '(copy 2)' 等）
- `panels.js:_startCronWatch / _stopCronWatch / _injectRunningIndicator` — Cron 运行监视：打开 running job detail 时轮询 /api/crons/history，动态注入 running indicator，完成后停止
- `panels.js:startCronPolling / updateCronBadge / _clearCronUnreadForJob` — 后台 cron 完成轮询：每 30s 拉 /api/crons/recent，完成时 toast 通知 + 在 tasks nav tab 显示未读数字 badge，查看后 per-job 清除
- `panels.js:_kanbanCard` — Kanban 卡片 HTML：显示 id/priority badge/tenant badge/title/body(markdown inline)/assignee/comment count/link count/age/staleness class，支持 draggable
- `panels.js:_kanbanRenderBoard / _kanbanRenderColumn / _kanbanRenderSidebar / _kanbanRenderProfileLanes` — Kanban 看板渲染：可切换 by-column 和 by-profile-lane 两种视图，侧边栏显示列统计
- `panels.js:_kanbanVisibleTasks / _kanbanCurrentFilters` — Kanban 过滤：assignee/tenant/archived/onlyMine 四维过滤，返回 filtered columns
- `panels.js:loadKanban / refreshKanbanEvents` — Kanban 数据加载：先解析 active board，拉 config+assignees+board，处理 read-only 标志；event-driven 更新通过 SSE/polling 增量触发
- `panels.js:_kanbanStartEventStream / _kanbanStartPolling` — Kanban 实时更新：优先 SSE（EventSource /api/kanban/events/stream），失败 3 次后 fallback 30s polling
- `panels.js:_kanbanRenderTaskDetail / _kanbanFormatDetailValue / _kanbanDetailSection` — Kanban 任务详情面板：渲染 title/status/description/assignee/links/comments/events/runs 等全字段
- `panels.js:openKanbanCreate / _kanbanResetTaskModalFields / _kanbanSetTaskModalLabels / closeKanbanTaskModal / _trapModalFocus` — Kanban 任务创建/编辑 modal：焦点陷阱，Escape/Enter 键绑定，status hint 随状态动态变化
- `panels.js:dragKanbanTask / finishKanbanDrag / allowKanbanDrop / clearKanbanDrop` — Kanban 卡片拖拽移列：HTML5 DnD API，ondragover 防 300ms click 误触（_kanbanSuppressNextCardClick）
- `panels.js:_renderKanbanBoardMenu / toggleKanbanBoardMenu / _kanbanGetSavedBoard / _kanbanSetSavedBoard` — Kanban 多看板切换：board menu dropdown，当前 board slug 持久化到 localStorage
- `panels.js:_renderInsights` — Insights 面板渲染：overview 统计卡(sessions/messages/tokens/cost) + 每日 token 柱状图(input/output 分层) + 模型用量表 + 星期/时段活跃度热力条
- `panels.js:_bucketDailyTokensForChart` — 日 token 数据分桶：超过阈值时按周/月合并，保证图表不横向溢出
- `panels.js:renderSkills / filterSkills / _toggleCatCollapse` — Skills 面板：按 category 分组渲染，category 可折叠（状态存 _collapsedCats Set），实时搜索过滤
- `panels.js:_renderSkillDetail / _enhanceSkillMarkdown / _skillMarkdownHtml` — Skill 详情：渲染 YAML frontmatter 去除后的 markdown，增强渲染（linked files 超链接）
- `panels.js:_renderSkillForm / openSkillCreate / editCurrentSkill / cancelSkillForm` — Skill CRUD 表单：name/category/content 三字段，create/edit 模式
- `panels.js:_renderMemoryDetail / _renderMemoryEdit / editCurrentMemory / selectExternalNotesSource` — Memory 面板：多 section 选择，支持编辑 core/project/profile memory，可接入外部 notes source (Notion/Obsidian)
- `panels.js:renderWorkspacesPanel / _renderWorkspaceDetail / renderWorkspaceDropdownInto` — Workspaces 面板：列表 + detail 双列布局，workspace dropdown 同时在 composer 中渲染，支持创建/编辑/删除
- `panels.js:scheduleWorkspacePathSuggestions / _renderWorkspacePathSuggestions / closeWorkspacePathSuggestions` — Workspace path autocomplete：输入时 debounce 300ms 请求 /api/workspace/path_suggest，上下键导航，Enter 选中
- `panels.js:_renderProfileConceptHelp / _renderProfileDetail / renderProfileDropdown` — Profiles 面板：概念帮助卡（首次引导），profile detail 含 model/instructions/skills 字段，profile dropdown 带 active 标记 + switch 操作
- `panels.js:_refreshProfileSwitchBackground` — Profile 切换后台刷新：生成器模式防止过时回调覆盖最新状态
- `panels.js:_sanitizeTabPanelList / _getHiddenTabs / _setHiddenTabs / _getTabOrder / _setTabOrder` — Tab 可见性和顺序管理：localStorage 持久化，ALWAYS_VISIBLE_TABS 白名单保护 chat/settings
- `panels.js:_renderTabVisibilityChips / _wireTabChipDrag / _moveTabOrderPanel / _toggleTabVisibilityChip` — Tab 管理 UI：可拖拽排序 chip，toggle 显隐，拖拽后 suppress 250ms 防误点，role=switch aria-checked
- `panels.js:_applyTabOrder / _applyTabVisibility` — 应用 tab 顺序/隐藏：操作 .rail 和 .sidebar-nav 两套 DOM，隐藏当前激活 tab 时自动 switchPanel('chat')
- `panels.js:toggleSettings / _closeSettingsPanel / _showSettingsUnsavedBar / _discardSettings` — Settings 面板开关：dirty check + unsaved bar，discard 恢复，close 时 revert appearance preview
- `panels.js:_scheduleAppearanceAutosave / _retryAppearanceAutosave / _schedulePreferencesAutosave` — 设置自动保存：1s debounce，失败后 5s retry，状态显示 saving.../saved/error
- `panels.js:loadProvidersPanel / _buildProviderCard / _buildProviderQuotaCard` — Providers 设置页：列出所有 configurable/custom/oauth provider，每个 card 含 quota 状态、API key 输入、test 按钮
- `panels.js:_buildProviderQuotaPoolBreakdown / _providerQuotaPoolShouldDefaultOpen` — Quota pool 展开：按 pool 分组显示 used/limit/reset，localStorage 记住 pool 展开状态
- `panels.js:loadPluginsPanel / _buildPluginCard / switchPluginPage / _loadPluginPage` — Plugins 面板：列出已安装 plugin card（hooks badge, activation badge），启用/禁用 toggle；plugin page 以 sandboxed iframe 加载（allow-scripts,allow-forms,allow-popups）
- `panels.js:loadMcpServers / toggleMcpServer / loadMcpTools / _renderMcpTools / filterMcpTools` — MCP 管理：server 列表（transport badge/status badge/tool count/enable toggle），tools 列表（搜索+分页，schema summary 展示）
- `panels.js:loadGatewayStatus` — Gateway 状态卡：渲染 running/not_configured/stale 等状态，含 routing 模式和 failover 链路显示
- `panels.js:trackBackgroundError / showErrorBanner / navigateToErrorSession / dismissErrorBanner` — 后台 agent 错误追踪：非当前 session 出错时显示 sticky banner，FIFO 队列，点击导航到出错 session
- `panels.js:_selectedLogsFile / _filteredLogsLines / _renderLogs / _startLogsAutoRefresh / _syncLogsAutoRefresh` — Logs 面板：可选 log 文件/tail 行数，severity 过滤(all/debug/info/warn/error)，wrap 切换，30s 自动刷新（仅 panel 激活时）
- `panels.js:_renderSystemHealthPanel / _renderLlmWikiStatus / _renderSkillUsage` — Insights 子卡：系统健康 (CPU/mem/disk)、LLM wiki 索引状态（最后更新时间+条目数）、Skill 调用统计
- `panels.js:_cronGatewayNoticeHtml / _cronDiagnostics / copyCurrentCronDiagnostics` — Cron 诊断信息：当 cron 经过 gateway 路由时显示 gateway 状态警告；_cronDiagnostics 聚合 job 信息为可复制文本
- `panels.js:_buildAuxProviderOptions / _buildAuxModelOptions / _onAuxProviderChange / _markAuxDirty` — 辅助 provider/model 选择器：settings 页中第二个 provider/model 选择对，切换 provider 时动态重建 model options，变更时标 dirty
- `panels.js:_applySavedSettingsUi` — 保存的设置回填 UI：解析 settings JSON 并批量设置 form 控件（appearance/preferences/providers/aux），含 TTS/theme/skin/font-size/message-width 等

### 会话列表 + 消息渲染 (sessions.js + messages.js)  ·  86 项

- `sessions.js:_profileMatchesActiveProfile` — 比较事件中的 profile 名与当前活跃 profile 是否等价（含 default 别名处理）
- `sessions.js:_sessionEventProfilesMatch` — 判断 SSE 事件的 profile 是否匹配当前用户 profile，用于跨 profile 过滤
- `sessions.js:_isRestorableNewChatDraftSession` — 判断某个会话是否是可还原的空草稿会话（0条消息、无流、标题是 Untitled/New Chat 且 profile 匹配）
- `sessions.js:_rememberNewChatDraftSession` — 将空草稿会话 session_id 写入 localStorage，用于页面刷新后还原草稿
- `sessions.js:_clearRememberedNewChatDraftSession` — 清理 localStorage 中保存的草稿会话 ID
- `sessions.js:_restoreRememberedNewChatDraftSession` — 从 localStorage 读取草稿会话 ID 并调用 loadSession 还原，返回是否成功
- `sessions.js:_saveComposerDraft` — 防抖 400ms 把 composer textarea 内容（text+files）POST 到 /api/session/draft 持久化
- `sessions.js:_saveComposerDraftNow` — 立即（不防抖）保存 composer 草稿到服务器，切换会话前调用
- `sessions.js:_restoreComposerDraft` — 从服务器草稿数据恢复 composer textarea；含防止同会话 force-reload 覆盖用户正在输入内容的守卫
- `sessions.js:_clearComposerDraft` — 消息发送后清空服务器侧草稿（POST 空 text）
- `sessions.js:_getSessionViewedCounts` — 懒加载并缓存 localStorage 中的已读消息计数 Map
- `sessions.js:_setSessionViewedCount` — 更新某会话已读消息数，并同步清理 completion-unread 标记
- `sessions.js:_markSessionCompletionUnread` — 当会话在后台完成时打上 completion-unread 标记（含 message_count + completed_at）
- `sessions.js:_clearSessionCompletionUnread` — 切换到会话或已读时清除 completion-unread 标记
- `sessions.js:_hasUnreadForSession` — 综合 completion-unread 和 viewed-counts 判断会话是否有未读消息
- `sessions.js:_isSessionActivelyViewedForList` — 判断某会话是否正在被用户主动查看（当前 session + tab 可见 + 窗口聚焦）
- `sessions.js:_isSessionLocallyStreaming` — 仅检查本地 S.busy 来判断当前 session 是否正在流式输出（规避 INFLIGHT 幽灵条目）
- `sessions.js:_isSessionEffectivelyStreaming` — 综合 server is_streaming 和本地 busy 判断会话是否有效流式中
- `sessions.js:_isServerIdleSessionRow` — 服务端视角判断会话是否完全空闲（无流、无 pending 消息）
- `sessions.js:getSessionManualStatus / setSessionManualStatus` — 读写 localStorage 中会话的手动状态（todo/in-progress/done），并触发列表重渲染
- `sessions.js:_cycleSessionManualStatus` — 循环切换会话手动状态（todo→in-progress→done→null）
- `sessions.js:_reconcileActiveSessionIdleStateFromList` — 通过轮询到的 session 列表数据，清除当前活跃会话的过期 INFLIGHT/busy 状态
- `sessions.js:_purgeStaleInflightEntries` — 清理 INFLIGHT Map 中已无会话或服务端确认非流式的幽灵条目，防止内存无限增长
- `sessions.js:_markPollingCompletionUnreadTransitions` — 在轮询 session 列表时检测「流式 → 空闲」的状态转变，对后台完成的会话打 unread 标记
- `sessions.js:_markSessionCompletedInList` — 在 SSE done 事件后原地更新 _allSessions 列表中对应行的元数据并清除流式标记
- `sessions.js:newSession` — 创建新会话：调 /api/session/new，携带 workspace/profile/model/prev_session_id，完成后重置 UI 状态
- `sessions.js:loadSession` — 两阶段加载：Phase1 取 metadata（messages=0），Phase2a(INFLIGHT 合并) 或 Phase2b(idle 加载完整消息)；含 stale 竞态守卫、draft 保存与还原、SSE 重连
- `sessions.js:_resolveSessionModelForDisplaySoon` — 异步 setTimeout(0) 二次拉取 resolve_model=1 的会话元数据，修正模型标签显示，不阻塞切换
- `sessions.js:_ensureMessagesLoaded` — 幂等地加载 session 消息（初始限 30 条），调用 _syncToolCallsForLoadedMessages 和 _carryForwardEphemeralTurnFields
- `sessions.js:_messageReloadLimitForSession` — 计算 force-reload 时应请求的消息条数（保留当前已展开宽度，避免长会话缩回到默认 30 条）
- `sessions.js:_loadOlderMessages` — 滚动到顶部时「加载更多」：累进扩展 msg_limit，含后缀连续性校验和 msg_before 竞态 fallback，保持滚动位置
- `sessions.js:_ensureAllMessagesLoaded` — 一次性加载全部消息（undo/导出用），通过 _messagesGeneration 代际令牌防止与 _loadOlderMessages 竞争
- `sessions.js:_mergeInflightTailMessages` — 将 INFLIGHT 中的 live 消息尾巴（从最近 user 消息开始）合并到 base 消息列表，去重
- `sessions.js:_syncToolCallsForLoadedMessages` — 从已加载消息元数据重建 S.toolCalls（流式结束时 message 内已有工具元数据），避免闲置时 Activity 消失
- `sessions.js:send` — 主发送函数：防并发守卫→slash 命令拦截→文件上传→乐观 UI（append user 消息 + INFLIGHT）→POST /api/chat/start→attachLiveStream
- `sessions.js:attachLiveStream` — 建立 SSE EventSource，注册所有事件处理器（token/reasoning/tool/tool_complete/done/apperror/cancel 等），管理 smd parser 和 rAF 渲染循环
- `sessions.js:closeLiveStream / closeOtherLiveStreams` — 关闭 SSE 连接，标记 INFLIGHT.reattach=true 以便重入时恢复；closeOtherLiveStreams 只保留活跃会话连接
- `sessions.js:_wireSSE` — 将 EventSource 对象注册到 LIVE_STREAMS 并绑定所有 SSE 事件监听器，支持重连时复用
- `sessions.js:_streamDisplay / _parseStreamState` — 从 assistantText 中提取 thinkingText 和 displayText，正确处理 <think> / <\|channel>thought 等多种推理标签
- `sessions.js:_stripXmlToolCalls` — 剥离 DeepSeek XML 格式的 <function_calls>...</function_calls> 标签，防止工具调用块显示在用户气泡中
- `sessions.js:_smdNewParser / _smdWrite / _smdEndParser` — 封装 smd.min.js streaming-markdown parser 的生命周期：创建、增量写入 delta、结束并 flush
- `sessions.js:_smdRendererWithoutUnderscoreEmphasis` — 包装 smd renderer，将 _ 和 __ 强调 token 降级为纯文本（避免变量名中下划线被误渲染为斜体）
- `sessions.js:_sanitizeSmdLinks` — 实时扫描 smd 输出的 DOM，将 file://、workspace://、session:// 协议链接重写为合法 URL，拦截危险 scheme
- `sessions.js:_scheduleRender` — rAF 节流渲染（66ms），防止 60fps 全速 DOM 更新在大会话中积压导致主线程崩溃
- `sessions.js:_streamFadeNextText` — 流式渐显算法核心：EWMA 估算到达词速，结合积压词数和时间，每帧只揭示几个词，避免整段突然弹出
- `sessions.js:_streamFadeRenderer` — 包装 smd default_renderer，在 add_text 中为每个词包裹 <span class='stream-fade-word is-new'>，携带 animationDelay 实现错开淡入
- `sessions.js:_drainStreamFadeBeforeDone` — SSE done 事件后先排干渐显缓冲（最多 900ms），等词语完成动画后再调 renderMessages() 替换 DOM
- `sessions.js:_carryForwardEphemeralTurnFields` — 将 prevMessages 中的 _turnUsage/_turnDuration/_turnTps/_gatewayRouting/_statusCard 字段按身份 key 迁移到 nextMessages，防止 force-reload 后使用量徽章消失
- `sessions.js:_messageIdentityKey` — 计算消息身份 key（role\|timestamp\|content前160字符），用于 ephemeral 字段携带匹配
- `sessions.js:_restoreSettledSession` — SSE error/stream_end 后拉取服务端完整 session 数据，如服务端已 idle 则直接渲染并返回 true
- `sessions.js:_handleStreamError` — SSE 网络断开的最终 fallback：置 _streamFinalized，插入「Connection interrupted」错误气泡
- `sessions.js:transcript` — 将 S.messages 序列化为 Markdown 格式的会话文本（role 二级标题 + 附件），用于导出
- `sessions.js:_isMessagingSession / _isExternalSession / _isCliSession` — 多维判断会话来源（messaging/cli/external），影响菜单项、handoff hint、import 逻辑
- `sessions.js:_isReadOnlySession` — 判断会话是否只读（read_only 或 is_read_only 标志），限制编辑操作
- `sessions.js:_checkAndShowHandoffHint / _showHandoffHint / _hideHandoffHint / _dismissHandoffHint` — 外部渠道（WeChat/Telegram/Discord 等）会话超过阈值对话轮次时显示 Handoff 提示条，含 dock 空间同步
- `sessions.js:_generateHandoffSummary` — 调 /api/session/handoff-summary 生成跨渠道摘要卡片，以 role:tool 消息插入会话视图（不存库）
- `sessions.js:renderSessionList` — 全局调度入口：generation 计数 + 队列排队确保最多 1 个在途请求，避免旧响应覆盖新数据
- `sessions.js:_applySessionListPayload` — 将 /api/sessions 响应写入 _allSessions，合并乐观首轮行，启停 streaming poll，触发 renderSessionListFromCache
- `sessions.js:_mergeOptimisticFirstTurnSessions` — 服务端响应到达前保留本地 INFLIGHT 对应的乐观 session 行（title/message_count/streaming 等），防止列表闪烁
- `sessions.js:ensureSessionEventsSSE` — 维护 /api/sessions/events SSE 长连接，接收 sessions_changed 事件触发列表刷新；含指数退避重连
- `sessions.js:startGatewaySSE / stopGatewaySSE` — 监听 /api/sessions/gateway/stream，实时同步 CLI/messaging 渠道 session，并在外部 session 更新时调用 /api/session/import_cli
- `sessions.js:refreshActiveSessionIfExternallyUpdated` — 轮询或 focus/visible 事件触发，检查服务端 message_count 是否增长，若是则 force-reload 当前会话
- `sessions.js:showApprovalCard / hideApprovalCard / respondApproval` — 工具审批卡片（Once/Session/Always/Deny）的显示、折叠、空间同步及响应，含 30s 最短可见守卫
- `sessions.js:startApprovalPolling / stopApprovalPolling` — 优先 SSE（/api/approval/stream），降级为 HTTP 轮询（1.5s），Session 切换或结束时停止
- `sessions.js:showClarifyCard / hideClarifyCard / respondClarify` — Clarification 对话卡片（多选项+自由输入+倒计时），提交后 echo 用户选择为 user 消息
- `sessions.js:startClarifyPolling / stopClarifyPolling` — 同 Approval 的 SSE+fallback 架构，含 60s 静默重连健康检测
- `sessions.js:attachBtwStream` — 渲染 /btw 非持久化旁路 SSE 流（瞬态 assistant 气泡，不写入 S.messages）
- `sessions.js:playNotificationSound / playAttentionSound / sendBrowserNotification` — WebAudio API 合成通知音，Notification API 系统推送；attention sound 带去重 key 防抖 5min
- `sessions.js:_openSessionActionMenu / closeSessionActionMenu` — 三点上下文菜单：动态构建 rename/pin/folder/archive/duplicate/stop/regenerate-title/status/delete 等操作项
- `sessions.js:_buildSessionAction` — 构建单条菜单项 button（icon+label，meta 作 title tooltip，无常驻文字），携带 onclick 闭包
- `sessions.js:applySessionTitleUpdate` — 乐观更新 session 标题（含 provisional 候选匹配守卫，防止用户已手动修改标题时被覆盖）
- `sessions.js:_captureSessionReflowPositions / _playSessionRowsReflowFromPositions` — FLIP 动画实现：先快照每行 top，DOM 更新后计算 delta，用 CSS transform 驱动平滑复位过渡
- `sessions.js:_sessionSearchRanges / _appendHighlightedText` — 全文搜索命中区间计算 + DOM 高亮注入（span.session-search-hit），支持多词分段匹配
- `sessions.js:toggleSessionSelectMode / selectAllSessions / _renderBatchActionBar` — 批量选择模式：复选框 UI + 批量 archive/move/delete，含 worktree 会话的特殊描述文案
- `messages.js:_markSessionViewed` — 包装 _setSessionViewedCount，会话被查看时同步已读计数
- `messages.js:_isSessionActivelyViewed / _isSessionCurrentPane` — 区分「是否是当前 pane」和「是否被用户实际看着」（可见性 + 焦点）
- `messages.js:_markActiveSessionViewedOnReturn` — tab visibility/focus 恢复时更新当前会话已读计数并重渲染列表
- `messages.js:send（在 messages.js 的 send 部分）` — 同 sessions.js:send，处理 busy 模式（queue/steer/interrupt）、slash 命令拦截、文件上传、并发守卫
- `messages.js:_clearStaleBusyStateBeforeSend` — 发送前检查 S.busy 是否是幽灵状态（无 activeStreamId/pending），若是则自动清除
- `messages.js:_selectedTextReplyButton / _positionSelectedTextReplyButton` — 用户在聊天区域选中文字时弹出「Reply with selection」浮动按钮，点击将选中文字格式化为 > 引用插入 composer
- `messages.js:_formatSelectedTextReplyQuote` — 将选中文本按行前缀 > 转为 markdown 引用格式
- `messages.js:_maybeNotifyPersistentStateSaved / _showPersistentStateToast` — 检测工具调用结果中是否包含 save/create 语义，若是 skill 或 memory 类型则弹出成功 toast
- `messages.js:_persistentToastHasWriteIntent` — 启发式判断工具 preview/name 是否包含写入意图（save/write/create 等关键词），排除 read/delete/dry-run 等
- `messages.js:autoResize` — textarea 自动高度调整（max 200px），发送按钮状态同步
- `messages.js:_fetchYoloState / _updateYoloPill / toggleYoloFromApproval` — YOLO 模式（自动通过审批）的状态同步、徽章显示和开关逻辑
- `messages.js:startBackgroundPolling / showBackgroundBadge / hideBackgroundBadge` — 后台任务（/background）轮询，完成后将结果推入 S.messages 并渲染，badge 计数实时更新

### 启动引导 + 命令面板 + 工作区 (boot/commands/workspace)  ·  73 项

- `boot.js:cancelStream` — 取消当前活跃 SSE 流；带 owner guard——只有当 backend 确认 cancelled===false 时才本地清除 activeStreamId，避免 SSE terminal event 还没到就撕掉 transport。
  - 💡 用 owner guard 防止竞态：streamId 在 cancel 请求飞行期间可能已被新流替换，对旧流不清 SSE。
- `boot.js:cancelSessionStream` — 对指定 session 对象执行完整 cancel 流程：停 SSE + 清 INFLIGHT + 停 approval/clarify polling。
  - 💡 相比 cancelStream 多一层对 session 对象的参数驱动，适合旁路（sidebar）场景。
- `boot.js:_savedSessionShouldStaySidebarOnly` — 判断保存的 session 是否因 active_stream_id 或 pending_user_message 存在而应留在侧边栏而非全屏恢复。
  - 💡 防止 boot 时把正在进行的 session 强行恢复到空白状态。
- `boot.js:_setWorkspacePanelMode` — 设置工作区面板模式 (closed\|browse\|preview)，同步 localStorage、CSS class、移动端 mobile-open 状态。
  - 💡 三态状态机：closed / browse / preview 分别对应不同 UI 行为，preview 关闭时只清预览不关面板。
- `boot.js:syncWorkspacePanelState` — 根据当前 session / preview 可见性决定面板应处于何种模式并驱动 _setWorkspacePanelMode。
  - 💡 无 session 时仅在 preview 模式下关面板，browse 模式保持不变，实现 workspace-persist 行为。
- `boot.js:openWorkspacePanel / closeWorkspacePanel / ensureWorkspacePreviewVisible / handleWorkspaceClose` — 工作区面板开/关/预览保证/关闭处理的公开 API，所有调用路径都收敛到 _setWorkspacePanelMode。
  - 💡 openWorkspacePanel 有 guard：无 session 且无 profile default workspace 时不允许开 browse。
- `boot.js:_setButtonTooltip` — 统一设置按钮 tooltip：有 data-tooltip 属性时用 CSS tooltip 并强制清 title，否则用 title；防止双 tooltip 同时显示 (#1775)。
- `boot.js:syncWorkspacePanelUI` — 同步工作区面板所有 UI 状态（toggleBtn aria、disabled 状态、clearBtn tooltip），是面板状态的单一同步点。
- `boot.js:toggleMobileSidebar / closeMobileSidebar` — 移动端侧边栏覆盖层的开/关，维护 mobile-open 和 mobileOverlay visible 两个 class。
- `boot.js:_installPwaSidebarSwipeGesture` — 为 PWA standalone 模式安装左边缘滑动手势打开侧边栏：只在 clientX < 28px 起始、水平移动 > 72px 且垂直漂移 < 48px 时触发。
  - 💡 用 pointer events 而非 touch events，同时过滤 interactive 元素目标防误触。
- `boot.js:toggleSidebar / expandSidebar / _restoreSidebarState (IIFE)` — 桌面侧边栏折叠/展开；boot 时 IIFE 把 HTML 上的 data-sidebar-collapsed 属性促进到 .layout.sidebar-collapsed 类体系，防止 JS 加载前的闪烁。
- `boot.js:_restoreTabVisibility (IIFE)` — boot 时恢复 localStorage 中保存的 tab 顺序和隐藏状态；如果活跃 tab 是隐藏 tab，自动切换到 chat。
- `boot.js:initResize / window._initResizePanels` — 鼠标拖拽调整侧边栏和右面板宽度；宽度持久化到 localStorage，支持 left/right 两个方向的 edge 计算。
- `boot.js:_normalizeAppearance` — 规范化 theme/skin 组合，处理遗留主题名映射（如 monokai→dark+sisyphus）和无效值回退。
- `boot.js:_applyTheme / _applySkin / _pickTheme / _pickSkin` — 应用主题（dark/light/system）和皮肤（accent 色包）；system 主题用 matchMedia 监听系统切换并绑定 change 事件。
- `boot.js:_syncThemeColorMeta` — 从 CSS 变量 --sidebar 读取当前皮肤颜色，同步到 <meta name=theme-color>，让 iOS PWA 状态栏和 WKWebView native chrome 显示正确颜色。
- `boot.js:_buildSkinPicker` — 动态生成皮肤选择器 Grid，每个色包显示三色点预览（primary/secondary/accent）。
- `boot.js:applyBotName` — 把 botName 设置同步到 document.title、sidebar h1、logo 首字母、topbar 标题、composer placeholder。
- `boot.js:主 async IIFE (boot 序列)` — 完整启动序列：加载 /api/settings → 应用所有偏好 → 并行加载 workspace list + onboarding → renderSessionList → 恢复 saved session → S._bootReady=true → startGatewaySSE。
  - 💡 把 model dropdown hydration 设为非阻塞（setTimeout 0）防止卡住 session 列表渲染；reconcile localStorage 与 server appearance 时只在 localStorage 有明确值时以 localStorage 为主，避免新用户首次访问时覆盖 server 默认值。
- `boot.js:pageshow listener` — 处理 bfcache 页面还原：清 sessionSearch、关 dropdown、重新 loadSession + checkInflightOnBoot、重启 SSE；解决 bfcache 冻结 DOM 导致搜索栏残留/SSE 连接死亡问题。
- `boot.js:shutdownServer / _showServerStopped` — 优雅关闭服务器：BroadcastChannel 广播 stop 给所有 tab，替换整个 body 内容为停止提示。
- `boot.js:Voice input IIFE` — 麦克风输入：优先 SpeechRecognition，网络错误后降级到 MediaRecorder+/api/transcribe；_activeCaptureMode 在录音开始时固定以防 mid-recording 切换 rawAudioMode 导致 stop 调用错误后端 (#3169)。
- `boot.js:Turn-based voice mode IIFE` — 链式语音对话：listening → thinking(send) → speaking(TTS) → listening 循环；hook autoReadLastAssistant 检测 agent 完成；支持 browser TTS 和 Edge TTS 两个引擎；_voiceModeThinkingSid guard 防止导航到其他 session 后 TTS 读错内容。
- `commands.js:COMMANDS 数组` — 30+ 个斜杠命令的声明式注册表，含 name/desc/fn/arg/subArgs/noEcho 字段；noEcho:true 的命令不产生用户消息回显。
- `commands.js:parseCommand` — 解析 /cmd args 格式，返回 {name, args} 或 null。
- `commands.js:executeCommand` — 按 COMMANDS 表调度命令；handler 返回 false 表示主动放弃拦截（fall-through 给 agent）；返回 {noEcho} 给 send() 决定是否回显用户消息。
- `commands.js:getMatchingCommands` — 前缀匹配内置命令 + SLASH_SUBARG_SOURCES + _skillCommandCache + _agentCommandCache，过滤 cli_only 命令不出现在补全中。
- `commands.js:_invalidateSlashModelCache` — 清 /api/models 补全缓存，供 panels.js 在 provider 变更后调用，确保 /model 补全实时准确。
- `commands.js:_loadSlashModelSubArgs / _loadSlashPersonalitySubArgs` — 懒加载 /model 和 /personality 子参数补全列表，带 promise 去重防并发重复请求；/model 同时覆盖 extra_models（大 catalog 的截断尾）。
- `commands.js:_parseSlashAutocomplete` — 解析当前输入决定补全种类：无空格→命令名补全，有空格→子参数补全；返回 {kind, query, command}。
- `commands.js:getSlashAutocompleteMatches` — 入口：调用 _parseSlashAutocomplete 后分路到命令名或子参数补全，返回统一格式的 matches 数组。
- `commands.js:_findComposerPathToken / getComposerPathAutocompleteMatches` — 在 composer 输入中识别 ~/ 开头的路径 token（按空白符分隔），请求 /api/workspaces/suggest 返回路径建议。
- `commands.js:_bestModelMatch / _nearestModelSuggestion / _looksLikeVersionedModel` — 模型名模糊匹配：精确→最短包含；versioned query (结尾是数字) 不能匹配带 tier suffix 的更长变体（防止 mimo-v2.5 命中 mimo-v2.5-pro）；_nearestModelSuggestion 用于 did-you-mean 提示。
  - 💡 防止 versioned query 被静默升级到付费 tier 是核心 UX 设计 (#3368)。
- `commands.js:_buildModelCandidates` — 从 /api/models 的 groups.models + groups.extra_models 构建完整候选集（含 provider_id 映射），补全 DOM option 缺失的 catalog 尾。
- `commands.js:cmdModel` — 处理 /model 命令：别名解析→_buildModelCandidates 全 catalog 模糊匹配→inject option→onchange；跨 provider 时直接 PATCH /api/session/update。
- `commands.js:cmdWorkspace` — 处理 /workspace 命令：模糊匹配名称或路径，调 switchToWorkspace 完成目录切换。
- `commands.js:cmdTerminal` — /terminal：无 session 时自动新建 session，再调 toggleComposerTerminal(true) 开嵌入终端。
- `commands.js:_runManualCompression / cmdCompress / cmdCompact` — 手动 context 压缩：preflight 验证 session 存活→POST /api/session/compress/start→轮询 /api/session/compress/status；_pollManualCompressionResult 指数退避（700ms→2000ms）。
  - 💡 anchoring：记录压缩前最后一条消息的 role/ts/text/attachments 作为恢复定位点；resumeManualCompressionForSession 在 boot 时续接中断的压缩任务。
- `commands.js:cmdGoal` — /goal 命令：POST /api/goal 启动长目标执行，拿到 stream_id 后走标准 INFLIGHT + attachLiveStream 流程，同步 approval/clarify polling。
- `commands.js:cmdQueue / cmdInterrupt / cmdSteer + _trySteer + _showSteerIndicator` — busy 模式三种输入策略：queue 排队/interrupt 取消当前再排队/steer 中途注入方向提示；_trySteer 先试 /api/chat/steer，失败时 fallback 到 interrupt+queue；_showSteerIndicator 在 DOM 渲染临时 steer 徽章（不进 S.messages）。
  - 💡 steer 实现了不中断当前 turn 的方向注入，是同类 UI 中少见的设计。
- `commands.js:cmdBtw` — /btw 命令：旁路询问（不中断主流）——POST /api/btw 拿到 ephemeral SSE stream，通过 attachBtwStream 独立渲染结果。
- `commands.js:cmdStatus / _statusCardFromSession` — /status 命令：把 session 元数据组装成结构化卡片（session_id/model/provider/tokens/cost/消息数/是否在跑）推入 S.messages 渲染。
- `commands.js:cmdBranch / forkFromMessage` — /branch 和「从此处 fork」：POST /api/session/branch 创建 session 副本；forkFromMessage 在截断窗口场景下先捕获 absoluteKeepCount = _oldestIdx + msgIdx 再 await _ensureAllMessagesLoaded，避免异步后 _oldestIdx 重置导致 keep_count 错误 (#2184)。
- `commands.js:loadSkillCommands / ensureSkillCommandsLoadedForAutocomplete / _skillCommandSlug / _buildSkillCommandEntry` — 把 /api/skills 中的技能注册为可补全的斜杠命令（skill 名转 slug），保证不与内置命令冲突；ensureSkillCommandsLoadedForAutocomplete 在用户首次键入 / 时懒加载。
- `commands.js:showCmdDropdown / hideCmdDropdown / navigateCmdDropdown / selectCmdDropdownItem` — 命令补全下拉框的渲染、键盘导航（ArrowUp/Down/Tab/Enter/Escape）和选中逻辑；支持 builtin/skill/agent/plugin/subarg/path 六种来源的条目差异化渲染。
  - 💡 path 类型条目选中后在 token 位置原地替换（不清空整行），自动追加 / 提示继续补全。
- `workspace.js:api` — 通用 fetch 封装：相对路径 → baseURI 拼接（支持 subpath 部署）；30s timeout + AbortController；3次重试（只 TypeError 不重试 4xx/5xx）；401 直接跳 login；错误时解析 JSON body 提取 message 字段。
  - 💡 timeout 和 upstream signal 合一的 AbortController 链接设计，防止 upstream cancel 和 timeout cancel 各自为战。
- `workspace.js:recordClientSSEError` — SSE 错误上报 /api/client-events/log，带 readyState/session_id/visibility_state/online 等诊断信息。
- `workspace.js:_wsExpandKey / _saveExpandedDirs / _restoreExpandedDirs` — 按 workspace path 为 key 在 localStorage 持久化文件树展开目录集合，切换 workspace 时自动恢复该 workspace 的展开状态。
- `workspace.js:switchWorkspacePanelTab` — 工作区面板 Files/Artifacts 两 tab 切换；切到 Artifacts 时触发 renderSessionArtifacts。
- `workspace.js:_normalizeArtifactPath` — 清洗工具调用中的文件路径：去首尾引号括号分号、规范化 ~/ 和 ./ 前缀、过滤 ignore 目录和无扩展名/无点号路径。
- `workspace.js:_artifactCandidatesFromText / _artifactCandidatesFromToolCall` — 从 diff/patch fence 文本或工具调用参数中提取 artifact 文件路径；ARTIFACT_MUTATION_TOOLS 集合定义哪些工具名有写意图。
- `workspace.js:noteWorkspaceMutationsFromToolCall / resetTurnWorkspaceMutations / refreshOpenPreviewIfMutated` — turn 级文件变更追踪：工具调用时记录变更路径，turn 结束时检查当前预览文件是否被修改并自动 bustCache 刷新预览。
- `workspace.js:collectSessionArtifacts` — 从 S.toolCalls + S.messages（OpenAI tool_calls 格式 + Anthropic content block 格式）中汇总本 session 所有文件写入路径，去重限 50 条。
- `workspace.js:renderSessionArtifacts` — 渲染 Artifacts tab 内容：去 workspace 前缀的相对路径列表，点击直接 openArtifactPath。
- `workspace.js:loadDir` — 加载目录：清缓存→请求 /api/list→S.entries→renderBreadcrumb+renderFileTree；root 加载时并行预取所有已展开目录（Promise.all 防串行瀑布）；非阻塞触发 _refreshGitBadge。
- `workspace.js:_refreshGitBadge` — 拉取 /api/git-info 渲染 git 徽章：branch name + dirty(N△) + behind/ahead 箭头。
- `workspace.js:openFile` — 文件预览路由：DOWNLOAD_EXTS→下载；IMAGE→raw img；AUDIO/VIDEO→media player；PDF→iframe；MD→rich render（超 256KB/5000 行降级纯文本）；HTML→sandboxed iframe allow-scripts；其他→/api/file + Prism 高亮（binary 时回退下载）。
- `workspace.js:_prismLanguageForPath` — 从文件名/扩展名映射 Prism.js language id，处理 Dockerfile/Makefile 等无扩展名特殊文件。
- `workspace.js:toggleEditMode / cancelEditMode` — 文件内联编辑：进入编辑模式时从 previewCode.textContent 或 _previewRawContent 填入 textarea；保存时 POST /api/file/save 并同步更新只读视图和 _previewRawContent 缓存。
- `workspace.js:forceRenderMarkdownPreview` — 强制 rich render 大 markdown：有 guard（非 dirty、非编辑中、缓存属于当前文件）才调 openFile({forceRichMarkdown:true})，防止 #3378 用旧缓存渲染错文件。
- `workspace.js:renderFileBreadcrumb` — 渲染文件预览模式下的路径面包屑，每段可点击（loadDir 到该层），末段非链接。
- `workspace.js:triggerWorkspaceUpload / uploadToWorkspace` — 文件上传到当前目录：用 FormData POST /api/workspace/upload（timeout 2 分钟），错误时区分 archive extraction 失败 vs 普通上传失败。
- `workspace.js:_collectOsDropUploads / _collectFilesFromEntry / uploadOsDropToWorkspace` — OS 文件拖拽上传：优先 webkitGetAsEntry API（保留目录结构），递归遍历子目录后批量上传到对应子目录。
- `workspace.js:_bindWorkspaceOsUploadDropTarget` — 为目录节点绑定 OS 文件拖拽 drop 处理，用 addEventListener 组合（非属性赋值）避免覆盖同元素上的 workspace tree move drop 处理器。
  - 💡 精确区分 _isOsFilesDrag vs workspace-internal-move drag 类型是并列 drop handler 的关键。
- `panels.js:loadWorkspaceList / syncWorkspaceDisplays` — 从 /api/workspaces 加载工作区列表，存入 _workspaceList 后刷新所有 UI 显示点（chip/dropdown）。
- `panels.js:renderWorkspaceDropdownInto` — 渲染工作区选择下拉框：搜索行（前端 filter）+ 按 name 排序的工作区列表 + footer 操作（新 worktree 对话/手动输入路径/管理页）。
- `panels.js:toggleWsDropdown / toggleComposerWsDropdown / closeWsDropdown` — 管理两个工作区下拉框实例（topbar 的 wsDropdown 和 composer 的 composerWsDropdown）的开/关状态；开时懒加载 workspace 列表；composerWsDropdown 还计算相对 footer 的 left 定位。
- `panels.js:renderWorkspacesPanel` — 渲染工作区管理列表（sidebar 内独立面板）：支持 drag-and-drop 排序，POST /api/workspaces/reorder 持久化；点击行打开工作区详情面板。
- `panels.js:_renderWorkspaceDetail / openWorkspaceDetail` — 工作区详情视图：显示 name/path/状态 badge/checkpoint 列表；根据 isActive/isDefault 控制 Activate/Delete 按钮可见性。
- `panels.js:_renderWorkspaceForm / saveWorkspaceForm / cancelWorkspaceForm` — 工作区新建/编辑表单：edit 模式 path 字段禁用（只能改名）；保存时 edit→/api/workspaces/rename，create→/api/workspaces/add + 可选 rename；有 _workspacePreFormDetail 可取消回退原状。
- `panels.js:_wireWorkspaceFormPathSuggestions` — 为 path 输入框接入文件系统路径自动补全（上下键选择/Enter 确认/Tab 选中），在 /api/workspaces/suggest 返回结果后渲染 suggestions 列表。
- `panels.js:switchToWorkspace` — 切换工作区：无 session 时自动新建→busy guard→dirty preview confirm→PATCH /api/session/update workspace→loadDir('.')；清 _profileSwitchWorkspace 防止后续 newSession 继承旧 workspace。
- `panels.js:promptWorkspacePath` — 通过 prompt dialog 手动输入路径，自动 add + switch；已存在路径友好提示不重复。

### 终端 + 引导 + 登录 + PWA  ·  55 项

- `static/terminal.js:TERMINAL_UI` — 全局状态对象，保存终端面板的 open/collapsed/sessionId/workspace/xterm 实例/fitAddon/ResizeObserver/计时器/拖拽状态/高度等，是唯一的单例真相源
- `static/terminal.js:_terminalEls` — 一次性查找并返回所有终端相关 DOM 元素（panel/inner/dock/viewport/surface/toggle/workspace/dockWorkspace/handle）
- `static/terminal.js:_trackTerminalInput` — 逐字符跟踪用户键入内容，回车时返回完整命令行，处理 Ctrl-C/Backspace/可打印字符；用于拦截 exit/quit 等关闭命令
  - 💡 不直接绑 keydown，而是在 xterm.onData 里解析原始字节序列
- `static/terminal.js:_terminalTheme` — 从 CSS 变量（--code-bg、--text、--accent 等）读取当前主题色，生成完整的 xterm.js Terminal.options.theme 对象，自动适配 dark/light
- `static/terminal.js:syncComposerTerminalTheme` — MutationObserver 回调：监听 html[class] 或 data-skin 变更，将新 theme 实时推给 xterm 实例
- `static/terminal.js:_ensureXterm` — 懒初始化 xterm.Terminal，加载 FitAddon/WebLinksAddon，注册 onData 钩子（转发输入到 /api/terminal/input），不重复创建
- `static/terminal.js:_terminalHeightBounds` — 根据 viewport 宽度（≤700px 为移动端）计算高度 min/max/default，max 额外受 viewport 高度 50% 约束
- `static/terminal.js:_applyTerminalHeight` — 将高度写入 CSS 变量 --composer-terminal-height，同步 aria-valuemin/max/now，触发 fitAddon.fit() 和 transcript 补偿
- `static/terminal.js:_startTerminalHeightResize / _moveTerminalHeightResize / _endTerminalHeightResize` — 基于 Pointer Events + setPointerCapture 实现拖拽调整终端高度，移动端（touch）不激活
  - 💡 用 pointerCapture 避免拖出边界后失去 pointermove 事件
- `static/terminal.js:_handleTerminalResizeKey` — 键盘辅助调整高度：ArrowUp/Down ±16px，PageUp/Down ±64px，Home→min，End→max；无障碍兼容
- `static/terminal.js:_syncTerminalTranscriptSpace` — 终端面板占据底部空间时，向消息列表动态注入 CSS 变量 --terminal-card-height / --terminal-dock-height，保持底部对齐；同时维护 is-near-bottom 滚动状态
  - 💡 用三次测量（immediate + rAF + setTimeout 420ms）覆盖 CSS 过渡期的高度变化
- `static/terminal.js:_connectTerminalOutput` — 建立 EventSource 连接到 /api/terminal/output，处理 output / terminal_closed / terminal_error 三类事件；终端关闭时优雅自清 SSE
- `static/terminal.js:_startComposerTerminal` — POST /api/terminal/start（含 rows/cols/restart），更新 sessionId/workspace，调用 _connectTerminalOutput
- `static/terminal.js:toggleComposerTerminal` — 公开入口：打开/关闭终端，处理已折叠时先展开、InitResizeHandle、ResizeObserver 绑定，open 失败时显示 toast
- `static/terminal.js:collapseComposerTerminal / expandComposerTerminal` — 折叠为 dock 标签条 / 从 dock 展开，保持 SSE 连接存活；展开时触发 fitAddon.fit() + rAF 动画过渡
- `static/terminal.js:closeComposerTerminal` — 关闭 SSE、POST /api/terminal/close（可选 skipApi）、280ms 延迟后 dispose xterm；用 sendBeacon 在 beforeunload 时保证通知后端
- `static/terminal.js:restartComposerTerminal` — 重置 xterm buffer，POST restart=true 重启壳进程
- `static/terminal.js:_terminalBufferText / copyComposerTerminalOutput` — 读取 xterm 活跃 buffer 所有行的文本，优先复制用户选中内容，降级复制完整 buffer
- `static/terminal.js:_scheduleTerminalResize / _resizeComposerTerminal` — 120ms 防抖后 POST /api/terminal/resize（rows/cols），窗口 resize 时触发
- `api/terminal.py:TerminalSession` — PTY 会话数据类：持有 proc/master_fd/output Queue(maxsize=2000)/closed Event/reader Thread；put_output 在队满时 drop 最旧 chunk 而非阻塞
- `api/terminal.py:_SpawnRequest / _spawn_supervisor_loop / _ensure_spawn_supervisor` — 单线程 supervisor 队列：所有 subprocess.Popen 调用都转发给这个固定线程，规避 ThreadingHTTPServer 多线程中 PDEATHSIG 失效问题；支持 5s spawn 超时 + 超时后 SIGHUP/SIGKILL 清理孤儿进程
  - 💡 Linux 上 PR_SET_PDEATHSIG 是 per-thread 的，在请求线程上设会让 shell 在线程退出时（~10ms）立即被杀，用 supervisor 线程规避
- `api/terminal.py:start_terminal` — 用 os.openpty() 分配 master/slave fd，构造最小化的 safe env（allowlist：PATH/HOME/USER/LANG/TZ 等，剥离所有 API key），以 start_new_session=True 启动 shell 进程组，再启动 reader 线程
  - 💡 env allowlist 而非 blocklist，防止 server 侧 API 凭据泄漏到用户终端
- `api/terminal.py:_reader_loop` — select.select 轮询 master_fd，读 8KB 块，用增量 UTF-8 decoder（replace）解码，put_output('output'/{text})；退出时写 terminal_closed 事件
- `api/terminal.py:_set_size` — ioctl TIOCSWINSZ 设置 PTY 窗口大小，再 SIGWINCH 通知 shell
- `api/terminal.py:close_terminal` — signal.SIGHUP → wait 1.5s → SIGKILL → 关闭 master_fd；优雅关闭链路
- `api/terminal.py:close_all_terminals` — atexit 注册，WebUI 优雅退出时批量关闭所有 PTY 壳
- `static/onboarding.js:ONBOARDING` — 引导向导全局状态：status/step/steps 数组/form 字段（provider/workspace/model/password/apiKey/baseUrl）/active 标志/probe 子状态
- `static/onboarding.js:_runOnboardingProbe / _scheduleOnboardingProbe` — 对 self-hosted provider（requires_base_url=true）的 base_url 进行 POST /api/onboarding/probe 连通性探测；结果缓存在 (provider\|baseUrl\|apiKey) 三元组 key，400ms 防抖，force=true 可绕过缓存
  - 💡 设计目的：防止用户填了不可达的 URL 后点 Continue 直接跑到 finish，实际上模型列表也靠探测返回
- `static/onboarding.js:_renderOnboardingBody` — 根据当前 step（system/setup/workspace/password/finish）模板渲染向导主体 DOM；每步内联 oninput 事件绑定到 ONBOARDING.form.*
- `static/onboarding.js:_renderProviderSelectOptions` — 用 <optgroup> 按 category 分组渲染 provider <select>；支持 quick badge 标记（快速启动提供商）
- `static/onboarding.js:_renderOnboardingBaseUrlField` — 仅当 provider.requires_base_url=true 时渲染 base_url input + probe 状态 banner（ok/probing/error 三态）+ Test button
- `static/onboarding.js:_renderOnboardingApiKeyField` — 渲染 API key 密码输入框；key_optional 标志（Ollama/LM Studio）时显示 '(optional)' 并允许空值通过
- `static/onboarding.js:_renderOnboardingProviderOAuthField` — anthropic provider 专属：渲染 'Login with Claude Code' OAuth 卡片，触发 startAnthropicOAuth
- `static/onboarding.js:nextOnboardingStep` — step 前进时做字段校验 + setup 步骤触发同步 probe 验证（probe.status!=='ok' 则 throw），最后一步调 _finishOnboarding
- `static/onboarding.js:_saveOnboardingProviderSetup` — POST /api/onboarding/setup；相同配置+未输入 apiKey 时跳过 POST（幂等保护，避免覆盖 OAuth 配置）
- `static/onboarding.js:_finishOnboarding` — 串行：setup → defaults → complete，完成后隐藏 overlay，loadWorkspaceList，newSession
- `static/onboarding.js:skipOnboarding` — 直接 POST /api/onboarding/complete 跳过，不改任何配置
- `static/onboarding.js:startCodexOAuth / _pollCodexOAuth / cancelCodexOAuth` — 设备码 OAuth 流程（openai-codex）：start→展示 user_code + verification_uri→3s 轮询 poll→success/expired/cancelled 三态终止
- `static/onboarding.js:startAnthropicOAuth / _pollAnthropicOAuth / cancelAnthropicOAuth` — Claude Code credential-link 流程：先探测服务器上是否已有凭据，若无则展示 'run claude setup-token' 提示并轮询检测
- `static/login.js:doLogin` — POST api/auth/login（JSON password），成功后跳转 _safeNextPath()
- `static/login.js:_safeNextPath` — 从 ?next= 参数提取重定向路径，防开放重定向：拒绝 //、\、控制字符、非路径绝对地址
- `static/login.js:doPasskeyLogin` — 完整 WebAuthn 登录：GET /api/auth/passkey/options → navigator.credentials.get → base64url 编解码 → POST /api/auth/passkey/login
- `static/login.js:b64uToBytes / bytesToB64u` — Base64URL ↔ Uint8Array 互转（无依赖纯 JS），供 WebAuthn Credential 序列化
- `static/login.js:checkConnectivity (IIFE)` — 页面加载时探测 /health，不可达则禁用表单 + setInterval 3s 重试，恢复后 location.reload()；区分'VPN 断了'和'密码错了'
- `static/sw.js:deleteOldShellCaches` — install 事件前先清除所有非当前 CACHE_NAME 的旧缓存，避免双缓存窗口
- `static/sw.js:install handler` — 预缓存 SHELL_ASSETS 列表（含 ?v=__WEBUI_VERSION__ 版本参数），addAll 失败非致命，调用 skipWaiting() 立即激活
- `static/sw.js:fetch handler` — 分策略拦截：/api/* /stream /health → 直通网络；/login /login.js → 永不缓存；navigate 请求 → network-first + cache './'; shell assets → network-first with cache fallback；其余 → 直通
  - 💡 不缓存 /login 是为了避免 stale 登录代码在老密码 POST 路径下让有效密码永远失败
- `static/pwa-startup.js:isStandalone / isIOS / syncMode` — 早期（bundle 之前）检测 PWA 安装态（navigator.standalone + display-mode media query），同步 pwa-standalone/pwa-browser/pwa-ios/pwa-offline CSS class 和 data-pwa-display-mode 属性
- `static/pwa-startup.js:HermesPWA.promptInstall` — 保存 beforeinstallprompt 事件（hermesDeferredInstallPrompt），promptInstall() 时调用并返回 userChoice Promise
- `static/pwa-startup.js:visibilitychange 处理` — 页面从后台恢复时 syncMode() + 短暂添加 pwa-resumed class（1200ms），供 CSS 动画使用
- `api/passkeys.py:registration_options / finish_registration` — WebAuthn 注册流程：生成 challenge + RP 配置 → 解析 attestationObject（最小 CBOR parser）→ 提取 COSE ES256 公钥 → PEM 存文件
- `api/passkeys.py:authentication_options / finish_login` — WebAuthn 认证流程：生成 challenge → 验证 clientDataJSON + authenticatorData（RP ID hash + user presence）→ EC P-256 签名验证 → sign_count 防重放
- `api/passkeys.py:_Cbor` — 零依赖的最小 CBOR 解析器（支持 major 0-5,7），用于解析 WebAuthn attestationObject
  - 💡 只实现了 WebAuthn 需要的子集，生产级够用
- `api/passkeys.py:_evict_oldest_challenges / _store_challenge / _consume_challenge` — challenge 存文件（90s TTL），同 context（provider+rp_id+origin）最多 8 个，全局最多 128 个，超限 LRU 驱逐；消费时原子删除防重放
- `api/auth.py:_resolve_session_ttl` — 三级 TTL 解析：env var HERMES_WEBUI_SESSION_TTL > settings.json > 30 天默认，范围 [60s, 1 年]

### 页面 DOM 骨架 + 主题/设计 token (hermes-webui)  ·  30 项

- `static/icons.js:li(name,size)` — 自托管 Lucide SVG 图标工厂：按名查 LI_PATHS 字典返回 SVG 字符串，无 CDN 依赖，尺寸可参数化
  - 💡 50+ 图标路径存成纯字符串字典，li() 运行时拼 SVG 标签，零 DOM 操作、零 HTTP 请求
- `static/boot.js:_applyTheme(name)` — 切换深浅色：给 html 加/去 .dark class，更新 localStorage 和 /api/settings，同步 theme-color meta
- `static/boot.js:_applySkin(name)` — 切换 accent 皮肤：写 html.dataset.skin，default 皮肤则删除该属性；同步 localStorage 和服务端
- `static/boot.js:_pickTheme(name)/_pickSkin(name)` — 用户点击 Appearance picker 时的入口：校验合法值后调 _applyTheme/_applySkin，同步 picker UI 选中状态
- `static/boot.js:_normalizeAppearance(theme,skin)` — 启动时处理历史遗留值迁移（slate->dark+slate, solarized->dark+poseidon 等旧 key），返回合法 [theme,skin] 对
- `static/boot.js:_syncThemeColorMeta()` — 根据当前 theme 更新所有 meta[name=theme-color] 的 content，让 PWA 和 Safari 状态栏跟随主题
- `static/boot.js:_buildSkinPicker(activeSkin)` — 动态生成 Appearance 页皮肤色块网格 HTML：读 SKIN_DEFS 常量，每块展示颜色预览 + 名字 + 选中标记
- `static/boot.js:_applyFontSize(size)/_pickFontSize(size)` — 写/删 html.dataset.fontSize；CSS 通过属性选择器 :root[data-font-size=X] 做分档覆写
- `static/boot.js:_restoreTabVisibility(IIFE)` — 页面首绘前同步读 localStorage hermes-webui-hidden-tabs 和 hermes-webui-tab-order，隐藏/重排 nav-tab，防止 flash
- `static/boot.js:syncWorkspacePanelState()` — 读 localStorage hermes-webui-workspace-panel 决定右侧文件面板开/关；写 html[data-workspace-panel] 属性触发 CSS 动画
- `static/boot.js:toggleWorkspacePanel(force)` — 切换右侧 rightpanel 展开/收起；窄视口下自动转底部 sheet 模式
- `static/boot.js:toggleMobileSidebar()/closeMobileSidebar()` — 移动端侧边栏覆盖层开/关；同时管理 mobile-overlay 遮罩
- `static/boot.js:toggleSidebar(forceState)/expandSidebar()` — 桌面端侧边栏折叠/展开：写 html[data-sidebar-collapsed]；展开时同时打开上次选中的面板
- `static/boot.js:cancelStream()/cancelSessionStream(session)` — 取消正在进行的 agent 流式响应；发 POST /api/session/{id}/cancel；用 AbortController 终止 SSE fetch
- `static/boot.js:initOfflineMonitor()` — 监听 navigator.onLine 变化 + 定时 /health probe；断线时 showOfflineBanner；恢复时 soft reload 不全页刷新
- `static/panels.js:switchPanel(name,opts)` — 切换左侧主面板（chat/tasks/kanban/skills/memory/workspaces/profiles/todos/insights/logs/settings）；同步 rail 和 sidebar-nav active 标记；按需懒加载各面板数据
- `static/panels.js:syncAppTitlebar()` — 更新顶部标题栏的标题文字和副标题；根据当前面板/会话切换显示内容
- `static/panels.js:loadInsights(animate)` — 加载 Usage Analytics 面板：GET /api/insights?period=N，渲染 token 用量/cost/session 数统计卡片
- `static/panels.js:loadCrons()/openCronCreate()/saveCronForm()` — Scheduled Jobs 面板全 CRUD：列表渲染/新建表单/保存，支持 cron 表达式校验和 agent profile 关联
- `static/panels.js:loadMcpServers()/loadMcpTools()/filterMcpTools()` — Settings->System 下 MCP 服务器列表和工具搜索分页；只读展示不写配置
- `static/panels.js:_buildProviderCard(p)/_saveProviderKey(providerId)` — Settings->Providers：每个 LLM 供应商渲染 API Key 输入 + quota 指示器 + 模型下拉，支持动态刷新可用模型列表
- `static/panels.js:saveSettings(andClose)` — 序列化所有 Settings 表单并 POST /api/settings；支持 autosave（appearance 立即保存）和手动 Save 两种模式
- `static/panels.js:loadGatewayStatus()` — 轮询 /api/system/gateway-status；渲染 Telegram/Discord/Slack 网关在线状态卡片
- `static/ui.js:_applySessionContextMetadataUpdate(data)` — 更新 composer 底部 ctx-ring SVG 圆弧和百分比数字——实时显示 context window 用量；同步移动端 config panel 的 context 字段
- `static/ui.js:_openImgLightbox(imgEl)/_openImgLightboxWithNav(src,alt,images,index)` — 点击消息内图片弹出全屏 lightbox；支持左右导航多图；Escape 或点击背景关闭
- `static/ui.js:renderProviderQuotaIndicator(status)` — 在 composer 右侧渲染 provider-quota-chip：余额/百分比，绿/黄/红三色；>=1400px 才显示
- `static/ui.js:_statusCardHtml(card)` — 把 /api/insights 返回的 stat card 对象渲染成 HTML：图标+数字+变化率 delta
- `static/ui.js:_renderUserFencedBlocks(text)` — 用户消息内的 ```代码块渲染：strip fence、Prism 高亮、复制按钮
- `static/ui.js:_reconcileModelDropdownSelection(sel,data,prev,opts)` — 模型下拉刷新时对比 provider/model 前后状态，智能恢复上次选中；处理 provider 不匹配时的 fallback 逻辑
- `static/boot.js:_installPwaSidebarSwipeGesture()` — PWA 模式下注册左边缘右滑手势打开侧边栏（velocity + distance 双条件）；与 _isPwaStandalone() 联动

### api 会话引擎（生命周期/事件/恢复/流式）  ·  60 项

- `api/session_lifecycle.py:register_agent` — 为 session 登记当前 AIAgent 句柄，用于后续 generation 的 memory commit；老的 dirty segment 保留原有 agent 不被覆盖
  - 💡 防止重建/重开 agent 抢走旧 dirty generation 的归属，避免 retry 链路断裂
- `api/session_lifecycle.py:unregister_agent` — 清除 session 的 future-generation agent 句柄；已有 dirty segment 归属保留，保证失败后可重试
- `api/session_lifecycle.py:discard_session` — 安全删除 session 生命周期条目，只在无 in-flight commit 且无未提交 work 时操作，防止 dict 无限增长（issue #3506）
  - 💡 条件删除而非强删，保护未完成 memory extraction
- `api/session_lifecycle.py:mark_turn_completed` — 递增 monotonic generation counter，将当前 turn 归入 segment，相邻同 agent 的 turn 合并进同一 segment
  - 💡 generation 计数 + segment 归属是序列化 commit 和防双提交的核心机制
- `api/session_lifecycle.py:has_uncommitted_work` — 判断 session 是否有 generation > committed_generation 的未提交 turn
- `api/session_lifecycle.py:_first_uncommitted_segment` — 找出第一个尚未提交的 segment（按 end > committed_generation 判断）
- `api/session_lifecycle.py:commit_session_memory` — 序列化执行 memory provider commit：in_flight guard 防并发，wait+deadline 可选阻塞，compare-and-clear 只更新被捕获的 generation，失败后保留 segment 可重试
  - 💡 in_flight 标志配合 Condition.wait 实现无死锁的单 commit 序列化；失败不擦除 segment，保证幂等重试
- `api/session_lifecycle.py:drain_all_on_shutdown` — shutdown 时循环 commit 所有 dirty session，有进展就继续，无进展则停止防止无限等待
- `api/session_events.py:publish_session_list_changed` — 向所有订阅者广播 sessions_changed 事件（递增 version），带 profile 作用域；队列满时做 coalesce 合并而非丢弃
  - 💡 maxsize=1 队列 + coalesce 策略：同 profile 直接替换，不同 profile 降级为无作用域全量刷新
- `api/session_events.py:subscribe_session_events / unsubscribe_session_events` — 管理 Queue 订阅者集合，供 SSE 长连接消费
- `api/session_events.py:_coalesced_sessions_changed_payload` — 合并 bounded 队列中挤掉的 pending 事件：profile 相同则覆盖，不同则清除 profile 字段广播全量刷新
- `api/session_ops.py:retry_last` — 截断 session 到最后一条 user message 之前，返回被截断的文本（/retry 语义），双层锁（agent_lock → LOCK）防竞态
  - 💡 stale-object guard：cache miss 时两个并发 get_session() 可能得到不同实例，需在锁内重新绑定规范实例
- `api/session_ops.py:undo_last` — 移除最近 user message 及其后所有内容（/undo 语义），返回被移除的预览文本
- `api/session_ops.py:session_status` — 返回 session 快照（/status 语义）：token 计数、cost、agent 运行状态（通过 active_stream_id 推断）
- `api/session_ops.py:session_usage` — 返回 token 用量和 estimated_cost（/usage 语义）
- `api/session_ops.py:apply_session_title_rename` — 应用用户重命名语义：非自动标签的 title 设置 manual_title 保护，清空或回到自动标签则解除保护
- `api/session_recovery.py:recover_all_sessions_on_startup` — 启动时扫描 session 目录，从 .bak 恢复消息数缩水的或孤儿 .bak 的 session，可选重建 index
- `api/session_recovery.py:recover_session` — 单 session 从 .bak 恢复：通过 tmp 文件 + atomic replace 保证崩溃安全
- `api/session_recovery.py:inspect_session_recovery_status` — 只读审计：比较 live.json 和 .bak 消息数，返回 recommend: restore/no_action/no_backup
- `api/session_recovery.py:recover_missing_sidecars_from_state_db` — 从 state.db 反向重建缺失的 JSON sidecar，使用 os.link() 原子创建防 TOCTOU
- `api/session_recovery.py:audit_session_recovery` — 只读多维度审计：shrunken_live、orphan_backup、index drift、state_db orphan row、turn_journal pending turn，分类 repairable/unsafe_to_repair
- `api/session_recovery.py:repair_safe_session_recovery` — 对 repairable 分类执行安全确定性修复（before/after 对比），不触碰 unsafe 分类
- `api/session_recovery.py:_state_db_has_session` — fail-open 模式查询 state.db 确认 session 是否存在，DB 异常时返回 True（宁可误恢复也不丢失）
- `api/session_discoverability.py:audit_session_discoverability` — 四源交叉检查（sidecar/index/state_db/api）：找出有消息但不可见的 session、source 错误分类（webui 被标为 cli）、lineage 断链
- `api/session_discoverability.py:repair_session_discoverability` — plan + apply 模式，dry-run 默认，需 backup_dir 才能 apply；修复 is_cli_session stale flag 和 state_db sidecar 缺失
- `api/session_discoverability.py:_lineage_root` — 沿 parent_session_id 链追溯 lineage 根节点，防循环
- `api/agent_sessions.py:normalize_agent_session_source` — 将 raw source 字符串规范化为稳定的 {raw_source, session_source, source_label} 三元组，隔离 UI 路由和 sidebar policy 对原始值的依赖
- `api/agent_sessions.py:is_cli_session_row` — 判断 agent session row 是否为 CLI 导入会话（多维 source 字段综合判断，messaging 源显式排除）
- `api/agent_sessions.py:is_cli_session_row_visible` — CLI session 可见性过滤：空消息不显示，有 lineage 或非默认标题则显示，否则需 ≥2 条 user message
- `api/agent_sessions.py:_is_continuation_session` — 判断 child session 是否为 parent 的延续（compression 或 cli_close 结束 + child 起始在 parent 结束之后）；cross-surface 不合并
- `api/agent_sessions.py:_project_agent_session_rows` — 将压缩链折叠为一个 sidebar 行：保留 chain head 标题/时间戳，指向最新可导入 segment，覆盖 last_activity 保证活跃链排序正确
  - 💡 折叠后 tip 的 last_activity 覆盖到 root 行，使长期活跃链始终排首位
- `api/agent_sessions.py:read_importable_agent_session_rows` — 读取 state.db 可导入 agent session，CTE 候选集 + 8x 过采样限制 JOIN 代价，normalize source，过滤不可见 CLI row
- `api/agent_sessions.py:read_session_lineage_metadata` — 批量读取 sidebar session 的 lineage metadata（lineage_root_id/segment_count），IN 子句分 500 块防超 sqlite 变量限制，最多 20 跳
- `api/agent_sessions.py:read_session_lineage_report` — 单 session 完整 lineage 报告（bounded max_hops=20），列出 segments + children + manual_review 标志
- `api/state_sync.py:sync_session_start` — 幂等注册 WebUI session 到 state.db（source='webui'）
- `api/state_sync.py:sync_session_usage` — 按 absolute=True 模式更新 state.db token 计数（WebUI 自行累计，避免 delta 重复计算），显式 profile 参数防 TLS 跨线程写错 DB（issue #2762）
  - 💡 worker 线程中 TLS profile context 未传播，必须显式传 profile= 才能写到正确的 state.db
- `api/state_sync.py:_get_state_db` — 解析 profile 名称（先验证格式再 resolve），explicit path 解析失败直接 None 不 fallback 到 default（防写错 DB 的防御契约）
- `api/streaming.py:_run_agent_streaming` — 核心 SSE 流式运行函数：启动 background thread，注册 active run，初始化 RunJournalWriter，管理 cancel_event/CANCEL_FLAGS/STREAMS/AGENT_INSTANCES 生命周期，调用 AIAgent 执行 turn
- `api/streaming.py:cancel_stream` — 取消 in-flight stream：持锁快照 partial_text/reasoning/tool_calls 后 eager 释放 session lock，让新请求立即继续；worker finally 用 pop(key,None) 安全幂等
  - 💡 eagerly 清除 active_stream_id 是关键设计：不等 worker 退出就解锁，防止 UI 被旧 stream 卡住
- `api/streaming.py:_stream_writeback_is_current` — 检查 worker 是否仍持有 session 写回权（active_stream_id 匹配），防止被取消的旧 worker 写回覆盖新 transcript
- `api/streaming.py:_stream_writeback_can_supersede_recovery_marker` — 窄条件允许真正完成的 worker 替换自己的 stale-repair 标记（recovery 误插的 'Response interrupted'）
- `api/streaming.py:_classify_provider_error` — 基于字符串匹配将 provider/agent 失败分类为 cancelled/interrupted/quota_exhausted/rate_limit/auth_mismatch/model_not_found/compression_exhausted/no_response/error，驱动前端 UX
- `api/streaming.py:_merge_display_messages_after_agent_result` — UI transcript 与 model context 解耦合并：context 压缩时保留 UI 历史，只追加 compaction marker + 当前轮；deduplicate stale _partial 消息
- `api/streaming.py:_handle_chat_steer` — 向运行中 agent 注入 /steer 文本（不中断 stream），通过 agent.steer() 在下次 tool-result boundary 应用；无 cached agent 或 stream 不活跃时返回 fallback reason
- `api/streaming.py:_build_agent_thread_env` — 构建 agent 线程环境变量（TERMINAL_CWD/HERMES_EXEC_ASK/HERMES_SESSION_KEY/HERMES_SESSION_ID/HERMES_SESSION_PLATFORM/HERMES_HOME），workspace 覆盖 profile env
- `api/streaming.py:_close_evicted_agent_at_session_boundary` — LRU eviction 时 commit memory → unregister → discard session entry → shutdown_memory_provider → close session_db，顺序保证无泄漏
- `api/streaming.py:_refresh_cached_agent_runtime` — 复用 cached AIAgent 时只刷新 credential/client（不重建 agent），保留 memory/tool state
- `api/streaming.py:generate_title_raw_via_aux / generate_title_raw_via_agent` — 异步 LLM 生成会话标题：aux 路径用独立 provider/model，agent 路径复用当前 session agent；探测语言一致性，fallback 到前 N 字截断
- `api/streaming.py:_lifecycle_commit_session_memory / _lifecycle_has_uncommitted_work / _lifecycle_unregister_agent / _lifecycle_discard_session` — streaming.py 对 session_lifecycle 模块的 lazy-import 封装，防止循环导入
- `api/turn_journal.py:append_turn_journal_event` — append-only JSONL 写入一条 turn 事件，flock 序列化防多进程交错，fsync + fsync 父目录保证崩溃可见性
  - 💡 POSIX atomic-write 边界小，大 payload 须 flock；父目录 fsync 保证文件创建持久化
- `api/turn_journal.py:read_turn_journal` — 合并所有 pid-scoped shard（{sid}~{pid}.jsonl）+ legacy 文件，容忍 malformed 行，按 created_at 排序
- `api/turn_journal.py:derive_turn_journal_states` — 按 turn_id 折叠最新事件（latest-by-timestamp），同时检测 double-terminal 碰撞（同一 turn 既 completed 又 interrupted）
- `api/turn_journal.py:append_turn_journal_event_for_stream` — 为 stream_id 关联的 turn 追加事件，若无显式 turn_id 则从 journal 中找最后匹配 stream_id 的 turn_id
- `api/turn_journal.py:iter_turn_journal_session_ids` — 枚举 journal 目录所有 shard，提取 session_id 集合（供 recovery 扫描用）
- `api/run_journal.py:append_run_event` — 带 seq 的 append-only run 事件写入，terminal 事件才 fsync（terminal-only 模式），非 terminal 仅 flush（性能优化）
  - 💡 fsync 策略由 HERMES_WEBUI_RUN_JOURNAL_FSYNC env 控制（eager vs terminal-only），可按需权衡持久性与吞吐
- `api/run_journal.py:RunJournalWriter` — 有状态流式 writer：构造时读取当前 max_seq，append_sse_event 每次原子递增 seq，per-path threading.Lock 防同进程并发
- `api/run_journal.py:read_run_events` — 读取 run journal，支持 after_seq 游标增量拉取
- `api/run_journal.py:stale_interrupted_event` — 检测运行中但 worker 已消失的 stream，合成 synthetic apperror 事件（recovery_control=True）让前端展示恢复提示
  - 💡 synthetic 字段区分真实事件和恢复注入事件，terminal_state='lost-worker-bookkeeping'
- `api/run_journal.py:_summary_from_events` — 从事件序列推导 run 状态摘要（running/completed/interrupted-by-user/interrupted-by-crash/errored/unknown）
- `api/run_journal.py:find_run_summary` — 按 run_id glob 查找跨 session 目录的 journal 文件，返回摘要+路径

### agent_health / gateway_chat / gateway_watcher / runner_client / runtime_adapter / route_approvals / clarify / compression_anchor / request_diagnostics / system_health  ·  43 项

- `api/agent_health.py:_runtime_status_is_fresh` — 判断 gateway_state.json 是否在 threshold_s（默认120s）内新鲜写入，作为跨容器 PID 隔离时的替代活跃信号
  - 💡 时钟偏斜处理：负 age_s（未来时间戳）在 ±threshold 以内视为 fresh，避免容器间时钟漂移误判
- `api/agent_health.py:_runtime_status_is_stale_stopped` — 判断 stopped 状态文件是否已过期（> threshold_s），过期则忽略，避免旧停止记录持续触发警报
- `api/agent_health.py:_runtime_status_is_stale_running` — 判断 running 状态文件是否已过期但仍写着 running，过期时返回 unknown 而非 down，尊重旧版 gateway 不持续更新文件的现实
- `api/agent_health.py:_remote_gateway_base_url` — 从四个优先级顺序环境变量中读取远程 gateway URL，并剥离 /health/* 尾缀，防止拼出 /health/health/detailed
- `api/agent_health.py:_http_probe` — 对给定 URL 做 GET，返回 (ok, status_code, error_name, body)，body 限 64KB 防 OOM，4xx 视为「已应答」让调用方尝试下一路径
- `api/agent_health.py:_probe_remote_gateway` — 遍历 /health/detailed, /health, /v1/health 三个路径探测远程 gateway，结果按 URL 缓存 5 秒，仅刷新 checked_at 时间戳
- `api/agent_health.py:_runtime_detail_subset` — 过滤 gateway_state.json，只暴露 gateway_state/updated_at/active_agents/platform_states 给浏览器，剔除 argv/PID/路径/token 等敏感字段
- `api/agent_health.py:build_agent_health_payload` — 组合所有探测逻辑，返回 {alive: True\|False\|None, checked_at, details}，alive=None 表示「无 gateway 配置」而非宕机
- `api/agent_health.py:_reset_remote_probe_cache_for_tests` — 测试钩子：清空进程内探测缓存，使单测可重置状态
- `api/gateway_chat.py:webui_chat_backend_mode` — 从 env 和 config_data 中读取后端模式，只接受显式 gateway/api_server/api-server 三值，其余 truthy 字符串一律视为 legacy，防意外切换
- `api/gateway_chat.py:gateway_chat_config_status` — 返回脱敏的 gateway 聊天配置状态（是否启用/base_url 是否配置/api_key 是否配置），不泄露原始值
- `api/gateway_chat.py:_gateway_sse_delta` — 从 OpenAI-compatible SSE chunk 中提取 delta.content 文本，兼容 choices[0].delta 和 choices[0].message 两种格式
- `api/gateway_chat.py:_gateway_stream_usage` — 从 SSE payload 提取 usage，兼容 prompt_tokens/input_tokens/completion_tokens/output_tokens/estimated_cost 多命名变体
- `api/gateway_chat.py:_gateway_tool_progress_event` — 把 hermes.tool.progress SSE payload 翻译成 WebUI 内部 tool/tool_complete 事件，含 tid 匹配和 is_error 标记
- `api/gateway_chat.py:_run_gateway_chat_streaming` — 核心 gateway bridge worker：在后台线程内向 Gateway /v1/chat/completions 发 POST，接收 SSE 流，翻译为 WebUI 内部事件，最终写回 session
- `api/gateway_watcher.py:_cheap_change_fingerprint` — 仅扫描 sessions 表（无 messages JOIN）生成变更指纹，同时追加 per-session message 聚合防止 replace_messages 绕过；指纹不变则跳过昂贵投影（#3506 优化）
- `api/gateway_watcher.py:_snapshot_hash` — 对会话列表按 session_id 排序后拼接 id:updated_at:message_count 生成 MD5，用于全量变更检测的第二层比对
- `api/gateway_watcher.py:GatewayWatcher` — 后台守护线程，5 秒轮询 state.db，两阶段（cheap fingerprint → 全量投影）变更检测，通过 subscribe/unsubscribe queue 向 SSE 推送 sessions_changed 事件
  - 💡 慢订阅者队列满时立即移除并发 None sentinel，让 EventSource 自动重连，防止内存泄漏
- `api/gateway_watcher.py:GatewayWatcher.is_alive` — 检查 poll 线程是否存活，供 SSE handler 判断是否返回 503 触发客户端降级轮询
- `api/gateway_watcher.py:start_watcher / stop_watcher / get_watcher` — 模块级单例管理：幂等启动、停止（带 sentinel 唤醒）、获取，用 threading.Lock 保护
- `api/runner_client.py:HttpRunnerClient` — 纯 HTTP JSON 客户端，封装与外部 runner sidecar 的通信（start_run/observe_run/get_run/cancel_run/respond_approval/respond_clarify/queue_message/update_goal），不持有任何进程内状态
  - 💡 自定义 NoRedirect handler 禁止 3xx 重定向，防止 Bearer token 被泄露到攻击者控制的 Location
- `api/runner_client.py:HttpRunnerClient.from_env` — 从 HERMES_WEBUI_RUNNER_BASE_URL / HERMES_WEBUI_RUNNER_API_KEY 构建客户端，验证 scheme 必须为 http/https
- `api/runtime_adapter.py:RuntimeAdapter` — Python Protocol（结构子类型）定义 8 个方法接口：start_run/observe_run/get_run/cancel_run/respond_approval/respond_clarify/queue_message/update_goal，是运行时后端的标准缝合点
- `api/runtime_adapter.py:StartRunRequest / RunStartResult / RunEventStream / RunStatus / ControlResult` — 冻结 dataclass 定义运行时数据结构，是适配器协议的类型契约
- `api/runtime_adapter.py:build_runtime_adapter` — 根据 HERMES_WEBUI_RUNTIME_ADAPTER 环境变量（legacy-direct/legacy-journal/runner-local）工厂函数构建适配器，返回 None 表示走默认直通路径
- `api/runtime_adapter.py:RunnerRuntimeAdapter` — 注入式 client 的协议翻译 facade，不持有 agent 实例/队列/flags，仅把 dict 响应规范化为 dataclass
- `api/runtime_adapter.py:LegacyJournalRuntimeAdapter` — 旧版流执行路径的 facade，通过注入 delegate callable 隔离，observe_run 读 run_journal 文件，get_run 查 live_stream_lookup 判断是否还在运行
- `api/runtime_adapter.py:_active_control_result` — 将 dict/bool/ControlResult 归一化为 ControlResult，统一旧版控制操作响应格式
- `api/route_approvals.py:submit_pending` — 向 per-session 审批队列追加带 uuid4 approval_id 的审批项，在持锁内通知 SSE 订阅者（防止并发时出现乱序通知）
- `api/route_approvals.py:_approval_sse_subscribe / _approval_sse_unsubscribe` — 注册/注销 per-session 审批 SSE 订阅队列（maxsize=16）
- `api/route_approvals.py:_approval_sse_notify_locked` — 在持锁时向所有订阅者 put_nowait 审批事件，payload 包含 head（FIFO 头部条目）而非刚追加的尾部条目
  - 💡 设计关键：多个并发审批时 /api/approval/pending 返回的是 head，SSE 必须与其对齐，所以 notify 也传 head
- `api/clarify.py:_ClarifyEntry` — 单个 pending clarify 的容器：threading.Event（阻塞 agent 等待用户响应）+ data + result + clarify_id
- `api/clarify.py:submit_pending` — 向 per-session gateway_queues 追加 clarify 请求，含语义去重（同 question+choices_offered 则复用旧 entry），在锁内 SSE notify 保证排序
- `api/clarify.py:resolve_clarify / resolve_clarify_by_id` — 解决最老/指定 id 的 clarify：设置 entry.result + event.set() 解除 agent 阻塞，pop 后推 SSE 给下一个 pending 或 None
- `api/clarify.py:register_gateway_notify / unregister_gateway_notify` — 注册/注销 per-session gateway callback，unregister 时批量 event.set() 解除所有等待中 agent 线程的阻塞
- `api/clarify.py:sse_subscribe / sse_unsubscribe` — clarify SSE 订阅管理，与 approvals 对称结构
- `api/compression_anchor.py:is_context_compression_marker` — 检测消息是否为合成的压缩锚点标记（[context compaction...] / [session arc summary...]），这些消息不计入 token 预算显示
- `api/compression_anchor.py:visible_messages_for_anchor` — 返回可用于压缩 UI 元数据锚定的消息子集，通过 auto_compression flag 区分手动压缩（仅 text 类型）和自动压缩（接受 input_text/output_text/thinking/reasoning）两种规则
- `api/request_diagnostics.py:RequestDiagnostics` — 请求慢速诊断工具：记录各阶段耗时（ms），超时时触发 watchdog 捕获全进程线程栈快照并打 warning log
- `api/request_diagnostics.py:RequestDiagnostics.maybe_start` — 仅对 GET /api/sessions 和 POST /api/chat/start 这两个延迟敏感路径启动诊断，其余路径返回 None
- `api/request_diagnostics.py:_thread_stack_snapshot` — 用 sys._current_frames() 采集所有线程的调用栈（最多40帧），按线程名排序，用于定位慢请求卡在哪里
- `api/system_health.py:build_system_health_payload` — 聚合 CPU（/proc/stat 50ms 采样）、内存（/proc/meminfo）、磁盘（shutil.disk_usage）三项指标，返回标准化 payload，任一失败只影响该指标
- `api/system_health.py:_cpu_percent` — 无 psutil 依赖地从 /proc/stat 采样两次 CPU ticks 计算占用百分比，sleep 50ms 取差值

### kanban/goals/metering/workspace-git/worktree  ·  90 项

- `api/kanban_bridge.py:_kb` — 延迟导入 hermes_cli.kanban_db，让 WebUI 在 hermes_cli 未安装时仍能启动（graceful degradation）
- `api/kanban_bridge.py:_resolve_board` — 从 URL query ?board=<slug> 中解析并规范化 board slug，校验存在性
  - 💡 query 优先于 body.board；两个解析路径合并到 _normalise_board_or_raise 确保一致
- `api/kanban_bridge.py:_normalise_board_or_raise` — 调用 kb._normalize_board_slug 并检查 board 是否已在磁盘上存在；default board 允许懒建
- `api/kanban_bridge.py:_conn` — 获取数据库连接：先 init_db(懒建 schema)再 connect，均接受 board=<slug> 参数
- `api/kanban_bridge.py:_task_dict` — 把 task 对象序列化为 dict 并附加 age_seconds/progress 字段
- `api/kanban_bridge.py:_latest_event_id` — 查 task_events 表 MAX(id)，用于 ?since= 轮询短路和 SSE cursor
- `api/kanban_bridge.py:_board_payload` — 构建看板列数据：按 BOARD_COLUMNS 顺序分组、附加 link_counts/comment_counts；支持 ?since= 增量轮询（changed:false 短路）
- `api/kanban_bridge.py:_set_status_direct` — 执行 drag-drop 状态跳转（非 running 列）；离开 running 状态时自动清空 claim_lock/claim_expires/worker_pid 并 _end_run('reclaimed')，防止 phantom-running
  - 💡 专门排除进入 'running' 的情况（强制走 kb.claim_task() 路径），保证 dispatcher 契约
- `api/kanban_bridge.py:_create_task_payload` — 创建任务：支持 parents/skills/workspace_kind/idempotency_key/max_runtime_seconds 等全量字段
- `api/kanban_bridge.py:_patch_task` — 统一任务状态机转换：done→complete_task、blocked→block_task、ready 从 blocked→unblock/否则→_set_status_direct、running→HTTP 400
- `api/kanban_bridge.py:_comment_payload` — 给任务添加评论，调用 kb.add_comment
- `api/kanban_bridge.py:_link_tasks_payload` — 创建或删除任务依赖链接（parent_id/child_id），支持 unlink=True
- `api/kanban_bridge.py:_task_detail_payload` — 单任务详情：task + comments + events + links + runs 一次返回
- `api/kanban_bridge.py:_events_payload` — 批量拉取 task_events，支持 since/limit 游标分页
- `api/kanban_bridge.py:_config_payload` — 返回看板全局配置：columns 顺序、已知 assignees、lane_by_profile 等
- `api/kanban_bridge.py:_stats_payload` — 按 status/assignee 聚合任务计数，优先调用 kb.board_stats 否则 fallback SQL
- `api/kanban_bridge.py:_task_log_payload` — 读取 worker log 文件内容，支持 ?tail= 截断
- `api/kanban_bridge.py:_bulk_tasks_payload` — 批量操作任务（archive/status/assignee/priority），per-task 错误捕获
- `api/kanban_bridge.py:_dispatch_payload` — 触发 kanban dispatcher 一次调度（?dry_run + ?max），调用 kb.dispatch_once
- `api/kanban_bridge.py:_task_action_payload` — 执行单任务结构化动作：block/unblock
- `api/kanban_bridge.py:_board_counts_for_slug` — 给 board switcher 提供 per-board 实时任务计数 badge，新板空 dict 不报错
- `api/kanban_bridge.py:_list_boards_payload` — 列出所有磁盘上的 board + 当前活跃 board；检测 active board 是否已被删除并自动回退 default
- `api/kanban_bridge.py:_create_board_payload` — 创建新 board（幂等）；?switch=true 时立即设为 active
- `api/kanban_bridge.py:_update_board_payload` — 更新 board 展示元数据（name/description/icon/color/archived），slug 本身不可变
- `api/kanban_bridge.py:_delete_board_payload` — 归档或硬删 board；拒绝删除 default board；active board 删除后自动回退 current pointer
- `api/kanban_bridge.py:_switch_board_payload` — 设置活跃 board；磁盘指针是跨 CLI/Gateway/Dashboard/WebUI 的共享 source of truth
- `api/kanban_bridge.py:_kanban_sse_fetch_new` — 轮询 task_events 取 id>cursor 的新事件，board 被删除后跳过查询自愈
- `api/kanban_bridge.py:_handle_events_sse_stream` — SSE 长连接推送 task 事件；支持 Last-Event-ID 断线续传；0.3s 轮询 + 15s keepalive 注释心跳
  - 💡 单向推送用 SSE 而非 WS，原因是 server 是 BaseHTTPServer 同步架构；事件 id: <cursor> 帧让 EventSource 自动重连时携带正确 cursor
- `api/kanban_bridge.py:handle_kanban_get/handle_kanban_post/handle_kanban_patch/handle_kanban_delete` — HTTP 方法级分发器，三值返回（False=未匹配/None=已响应/True=成功），跨 ImportError→503 / LookupError→404 / ValueError→400 / RuntimeError→409 统一错误映射
- `api/goals.py:_ProfileGoalManager` — WebUI 用的 GoalManager 适配层：显式 profile_home 路径持久化而非全局 HERMES_HOME，支持多 profile 并发
- `api/goals.py:_ProfileGoalManager.evaluate_after_turn` — 每轮结束后评估目标达成/预算耗尽；调用 judge_goal LLM judge；返回 should_continue + continuation_prompt
- `api/goals.py:_ProfileGoalManager.set/pause/resume/clear` — 目标生命周期管理：active/paused/done/cleared 四状态转换，每次操作后持久化到 state.db
- `api/goals.py:goal_state_snapshot / restore_goal_state` — 在 stream 创建前快照目标状态，stream 失败后原子回滚，防止 kickoff 半途失败留下错误 active goal
- `api/goals.py:goal_command_payload` — WebUI /goal 命令处理器：status/pause/resume/clear/set 分派；set 时 stream 正在运行则 HTTP 400；返回 kickoff_prompt 让调用方立即触发第一轮
- `api/goals.py:evaluate_goal_after_turn` — 每轮对话后判断目标状态，返回 should_continue/continuation_prompt/verdict 供 streaming 层注入下一轮
- `api/goals.py:_goal_decision_payload / _goal_status_payload` — 给 decision 附加 i18n message_key/message_args，前端按 key 渲染本地化文案
- `api/todo_state.py:_normalize_snapshot` — 规范化 todo 快照为 {todos, summary, version} 标准结构；空列表也是合法快照（latest write wins）
  - 💡 空列表不过滤是关键：防止冷加载时错误回溯到更旧的非空列表，与 agent 侧 _hydrate_todo_store 行为完全对称
- `api/todo_state.py:derive_todo_state` — 从 message history 反向扫描，找最近一条 role=tool 且含 todos 的消息；快速路径 '"todos"' in content 避免 JSON 解析所有 tool 消息
  - 💡 timestamp 丢失时 fallback 到 _max_timestamp_through 确保 cold ts >= 任何更早快照，防止 INFLIGHT 快照因 ts 比较错误赢过 cold-load
- `api/todo_state.py:_redact_snapshot` — SSE 路径的 todo 快照在发出前脱敏（非 redact_session_data 路径），防止 credential 泄漏进 run journal
- `api/todo_state.py:emit_todo_state` — todo 工具调用时发射 SSE todo_state 事件；失败时 swallow error 不破坏主 tool delivery 链路
- `api/todo_state.py:attach_todo_state` — 冷加载 session GET 时附加 todo_state 到响应 payload，mutates payload in-place
- `api/metering.py:GlobalMeter` — 全局线程安全 TPS 计量：per-stream 独立计量 + 全局均值 + 60分钟滚动 HIGH/LOW 历史
- `api/metering.py:GlobalMeter.get_interval` — 有活跃 stream 返回 1.0（1Hz ticker），否则返回 10.0（idle 退出），驱动 streaming ticker 自适应调速
- `api/metering.py:GlobalMeter.get_stats` — 剪枝 stale sessions → 计算 global TPS → 维护 rolling readings → 返回 {tps, tps_available, high, low, active}
- `api/usage.py:prompt_cache_hit_percent` — 计算 prompt cache hit 率（cache_read / total_prompt_tokens），后端统一计算防止前端多处实现漂移
- `api/workspace.py:_profile_state_dir` — 返回当前 active profile 的 webui_state 目录；default profile 回退全局 STATE_DIR
- `api/workspace.py:_profile_default_workspace` — 按优先级读取 profile 默认 workspace：workspace key > default_workspace key > terminal.cwd；remote terminal 跳过 stat
- `api/workspace.py:load_workspaces / save_workspaces` — 读写 workspaces.json；读时自动 clean（过滤跨 profile 条目）并持久化清洁版本
- `api/workspace.py:_clean_workspace_list` — 过滤跨 profile 泄漏的路径；把 name='default' 重命名为 'Home' 防止与 profile name 混淆
- `api/workspace.py:_is_blocked_workspace_path / _is_blocked_system_path` — 拦截系统目录注册（/etc /usr /var /bin 等）；carve-out /var/folders /var/tmp 等合法用户 tmp
- `api/workspace.py:resolve_trusted_workspace` — 三重信任检查：(A)home 子目录 (B)已保存列表 (C)boot DEFAULT_WORKSPACE 子目录；remote terminal 跳过 stat 直接信任
- `api/workspace.py:safe_resolve_ws` — resolve 并验证路径在 workspace root 之下，raise ValueError on traversal
- `api/workspace.py:open_anchored_fd / open_anchored_create_fd / open_anchored_write_fd` — TOCTOU 防御：通过 openat(dir_fd) + O_NOFOLLOW 组件逐段打开，防止 resolve 后 symlink 替换
  - 💡 专门处理 Windows fallback（无 dir_fd 支持时退化为 pathname open）
- `api/workspace.py:make_anchored_dir / unlink_anchored / rmtree_anchored / rename_anchored` — 所有文件系统变更操作的 anchored 版本，防止 symlink race escape
- `api/workspace.py:list_dir` — 目录列举：symlink 过滤（循环/跨 workspace/系统路径）；通过 anchored fd 扫描防 TOCTOU；最多 200 条
- `api/workspace.py:dir_signature` — 目录内容快速 SHA-256 签名（只用 metadata 不读文件内容），供轮询判断目录是否变化
- `api/workspace.py:read_file_content` — 通过 anchored fd 安全读文件内容，MAX_FILE_BYTES 限制
- `api/workspace.py:git_info_for_workspace` — 并行（ThreadPoolExecutor 3线程）执行 git status/ahead/behind，返回简版 git 状态给 workspace picker
  - 💡 三个 git 子进程并行跑减少 50-200ms 串行延迟
- `api/workspace.py:list_workspace_suggestions` — 路径自动补全：只在 trusted roots 下补全，~前缀保留波浪线格式
- `api/workspace_git.py:GitContext` — 冻结 dataclass：workspace/repo_root/workspace_prefix，是所有 git 操作的上下文载体
- `api/workspace_git.py:_git_mutation_lock` — 按 repo_root 串行化写操作（threading.Lock），不同 worktree 用不同锁；acquire timeout=GIT_REMOTE_TIMEOUT 防死锁
- `api/workspace_git.py:_clean_git_env` — 清除 GIT_DIR/GIT_WORK_TREE/GIT_CONFIG_* 等环境变量，防止宿主环境污染 git 子进程
- `api/workspace_git.py:_classify_git_error` — 把 git stderr 文本映射到结构化错误码（timeout/not_a_repo/auth_failed/conflict/dirty_worktree 等），前端按 code 渲染友好提示
- `api/workspace_git.py:resolve_git_context` — 从 workspace 路径找 repo root（rev-parse --show-toplevel）；计算 workspace_prefix 用于 pathspec 隔离
- `api/workspace_git.py:git_status` — 完整 status：porcelain=v2 解析分支/upstream/ahead/behind/files + numstat(+/- 统计) + 噪声过滤（filemode-only/crlf-only 变更不计入 staged）
  - 💡 staged_diff_paths 与 staged_raw_stats 双重校验过滤 CRLF/filemode 假 dirty，提升 Windows 跨平台体验
- `api/workspace_git.py:git_branches` — 列 local + remote 分支，每分支带 ahead/behind/author/subject；通过 --format= NUL 分隔单次 for-each-ref 获取
- `api/workspace_git.py:git_checkout` — 分支切换，支持 local/new/remote/detached 四种 mode；dirty_mode=block 阻止脏工作区切换
- `api/workspace_git.py:git_stash_and_checkout` — 切换前 stash 脏工作区（带 hermes-webui 前缀的 stash message），切换后自动 pop 目标分支匹配的 stash
  - 💡 stash 失败时 pop 回滚，目标分支的历史 stash 自动恢复——完整 undo 语义
- `api/workspace_git.py:git_stage / git_unstage / git_discard` — 文件级 staged/unstaged/discard 操作；discard 前检查 conflict 和 untracked（需 delete_untracked=true）
- `api/workspace_git.py:staged_commit_message_prompt / selected_commit_message_prompt` — 生成 commit message LLM prompt：附加 staged diff + file 统计；selected_commit 用临时 GIT_INDEX_FILE 隔离选中文件
- `api/workspace_git.py:_selected_temp_index_env` — 创建临时 git index 文件（mkstemp），在其中 stage 选中文件，操作后删除，实现无污染 selected-file commit
  - 💡 用 GIT_INDEX_FILE env override 而非 stash/reset，操作原子、零副作用
- `api/workspace_git.py:git_commit / git_commit_selected` — 提交全部 staged 或选中文件；selected 路径通过临时 index 实现，正常 index 不受影响
- `api/workspace_git.py:git_fetch / git_pull / git_push` — 远程操作（timeout=60s）；push 在无 upstream 时自动 -u origin <branch>
- `api/workspace_git.py:COMMIT_MESSAGE_SYSTEM_PROMPT` — commit message 生成的 system prompt 常量：明确禁止 vague subjects、禁止提及 AI 工具
- `api/worktrees.py:worktree_status_for_session` — 会话 worktree 快照：exists/dirty/untracked_count/ahead_behind/locked_by_stream/locked_by_terminal/listed
- `api/worktrees.py:remove_worktree_for_session` — 删除 worktree：先检查 stream/terminal lock、dirty/untracked/unpushed 守卫，再 git worktree remove + prune
- `api/worktrees.py:find_git_repo_root` — 通过 git rev-parse --show-toplevel 找 repo root，支持 nested worktree
- `api/worktrees.py:create_worktree_for_workspace` — 调用 hermes agent 的 _setup_worktree 创建 git worktree，返回 path/branch/repo_root/created_at
- `api/worktrees.py:_worktree_listed` — 调用 git worktree list --porcelain 验证 worktree 是否仍被 git 追踪，false 是 safe fallback
- `api/worktrees.py:_ahead_behind` — worktree 的 upstream ahead/behind 计数，available=false 表示无 upstream
- `api/worktrees.py:_hermes_branch_switch_stashes` — 列出以 'hermes-webui branch switch' 前缀命名的 stash，供自动恢复
- `api/background.py:track_background / complete_background / get_results` — 内存中追踪 background 并发 agent 任务（parent→child session）；get_results 只移除已完成任务，保留 running 防止丢失
- `api/background.py:track_btw / cleanup_btw` — 追踪 /btw 临时 ephemeral session（单一活跃），完成后清理
- `api/rollback.py:list_checkpoints` — 按 mtime 倒序列出 workspace 的所有 shadow-git checkpoint，每个含 commit hash/message/date/file count
- `api/rollback.py:_inspect_checkpoint` — 单个 checkpoint 目录的元数据提取（git log + ls-files），超时/报错返回 None 不影响列表
- `api/rollback.py:get_checkpoint_diff` — 对比 checkpoint 和当前 workspace 文件内容，生成 unified diff；检测 deleted/modified 文件
- `api/rollback.py:restore_checkpoint` — 把 checkpoint 文件复制回 workspace（不删除 checkpoint 后新增的文件），per-file 错误捕获
- `api/rollback.py:_validate_checkpoint_id` — checkpoint id 白名单校验（[A-Za-z0-9_-][A-Za-z0-9_.-]{0,63}），防止路径遍历攻击
- `api/rollback.py:_workspace_hash` — 用 realpath 后的 workspace 路径做 SHA-256[:12] 作为 checkpoint 目录名，与 agent CheckpointManager 完全对齐
- `api/skill_usage.py:read_skill_usage` — 只读取 .usage.json（由 agent 写入），返回 {skill_name: {use_count, view_count}} 给 Insights 页面

### 整体架构(server/mcp/bootstrap/routes/启动/文档)  ·  51 项

- `server.py:main` — 启动入口：打印启动配置、提升 FD soft limit、修权限、startup session recovery、启动 gateway_watcher、加载 plugins、检查端口冲突、创建 QuietHTTPServer + 可选 TLS、serve_forever；关闭时 drain lifecycle commit、停 watcher、输出 shutdown audit log
  - 💡 TLS 封装在 main() 内直接 wrap socket，不依赖任何 WSGI/ASGI 框架
- `server.py:QuietHTTPServer` — ThreadingHTTPServer 子类：daemon_threads=True、queue_size=64、IPv6 感知、Windows SO_EXCLUSIVEADDRUSE 重试 bind、TCP_NODELAY+SO_KEEPALIVE 每连接设置、静默吞掉 BrokenPipe/ConnectionReset 异常
- `server.py:Handler` — BaseHTTPRequestHandler 子类：HTTP/1.1 keep-alive、per-request profile cookie 注入/清理、check_auth 前置、dispatch 到 api/routes.py 的 handle_get/post/put/patch/delete、结构化 JSON access log、CSP-Report-Only 头自动追加
- `server.py:Handler.end_headers` — 在每个响应自动追加 Content-Security-Policy-Report-Only 和 Report-To 头，不需要每个路由手动设置
- `server.py:Handler._handle_write` — 统一处理 POST/PUT/PATCH/DELETE 的公共包装：profile cookie 设置、CSP report 特例豁免(仅 POST)、错误处理，注意 /api/csp-report 的 POST 豁免鉴权
- `server.py:_addr_is_local` — 测试隔离用：识别 127.x/10.x/192.168.x/172.16-31.x/::1/fe80:/fc00:/localhost/.local/.test/.example 等私有地址，阻断测试进程的出站网络
- `server.py:_abort_if_already_serving` — 启动自检：探 GET /health，发现活跃 server 则 sys.exit(1)，防止双实例破坏 DB state
- `server.py:_raise_fd_soft_limit` — 启动时 best-effort 将 RLIMIT_NOFILE soft limit 提升到 4096，避免 macOS launchd 默认 256 导致 FD 泄漏快速触顶
- `server.py:_log_shutdown_audit` — 进程退出时记录 active_sessions 列表(sid/stream_id/pending)，诊断意外 kill
- `server.py:_csp_extra_connect_src / _valid_csp_extra_connect_source` — 解析 HERMES_WEBUI_CSP_CONNECT_EXTRA 环境变量为 CSP connect-src 允许源，带格式校验
- `bootstrap.py:main` — 一次性启动器：平台检测、agent 目录探测、Python 解释器选择、venv 按需创建、supervisor 环境检测、前台(execv)/后台(Popen+log)两条启动路径、健康等待、browser open
- `bootstrap.py:discover_agent_dir` — 多策略探测 hermes-agent 安装目录：env var > HERMES_HOME/hermes-agent > sibling > parent > ~/.hermes/hermes-agent > ~/hermes-agent，最后读 hermes CLI shebang 反向找 venv
- `bootstrap.py:_agent_dir_from_hermes_cli` — 解析 `hermes` CLI 文件 shebang 行，提取解释器路径，向上找包含 run_agent.py 的目录——处理用户自定义 clone 路径
- `bootstrap.py:ensure_python_has_webui_deps` — 验证 Python 能同时 import webui + hermes-agent 依赖，不能则尝试 agent venv > 本地 .venv > 新建 .venv + pip install，最后还失败则报告具体 fix-it 步骤
- `bootstrap.py:_detect_supervisor` — 通过 INVOCATION_ID/JOURNAL_STREAM/NOTIFY_SOCKET/XPC_SERVICE_NAME/SUPERVISOR_ENABLED 等 env var 自动检测 systemd/launchd/supervisord，XPC_SERVICE_NAME 做 noise 过滤(排除 'application.' 前缀和 '0')，决定是否 --foreground
- `bootstrap.py:wait_for_health` — 轮询 /health 直到返回 '"status": "ok"'，25s 超时；先校验 URL scheme 防 file:// 危险注入
- `bootstrap.py:_load_repo_dotenv` — 启动时加载 REPO_ROOT/.env 到 os.environ（无 PRESERVE_ENV 则无条件覆盖），使 python bootstrap.py 与 ./start.sh 行为一致
- `mcp_server.py:main (async)` — 以 stdio_server 协议提供 MCP server，注册 7 个工具(list/create/rename/delete projects, list/rename/move sessions)，直接访问 webui 内部 api.models + api.profiles（绕过 HTTP 层，无网络开销）
- `mcp_server.py:_api_auth` — 懒初始化 + 25 天缓存：对 /api/auth/login POST 取 cookie，供写操作(rename_session/move_session/delete_project unassign)使用
- `mcp_server.py:_api_post` — 带 auth cookie 的 HTTP POST helper，用于需要 session cache 同步的写操作(session rename/move)——解释为什么不直接写文件：绕过 HTTP 会导致 _index.json 与内存 SESSIONS 缓存漂移
- `mcp_server.py:handle_delete_project` — 删除 project 后通过 HTTP API unassign sessions，没有密码时明确拒绝直接写文件并解释原因（index drift 危险）
- `api/routes.py:handle_get / handle_post / handle_put / handle_patch / handle_delete` — 扁平 if/elif 路由分发：无框架、无装饰器，所有业务路由在这5个函数里，维护 CRITICAL ORDERING RULE（/api/upload 必须在 read_body() 之前）
- `api/routes.py:_profiles_match (re-export)` — 从 api.profiles 重导出，让路由层的 profile 过滤无需 import 路径变动
- `api/routes.py:_publish_session_list_changed` — 包装 publish_session_list_changed，容忍历史测试 double 只有1个参数的形式——向下兼容 shim
- `api/startup.py:fix_credential_permissions` — 启动时扫描 HERMES_HOME 中 5 个敏感文件（.env/google_token.json/.signing_key/auth.json 等），按 HERMES_HOME_MODE 声明决定是否保留 group bits，清除 world bits，0600 兜底
- `api/startup.py:auto_install_agent_deps` — 受 HERMES_WEBUI_AUTO_INSTALL=1 控制，在 agent_dir 通过信任检查（非 group/world 可写、uid 匹配）后 pip install requirements.txt/pyproject.toml
- `api/startup.py:_trusted_agent_dir` — 两重信任检查：mode & 0o022（非 group/world 可写）+ uid 匹配当前用户，防止恶意 agent_dir 劫持 pip install
- `api/config.py:_discover_agent_dir` — 8 策略探测 hermes-agent：env > HERMES_HOME > sibling > parent(本身是 agent) > ~/.hermes/hermes-agent > ~/hermes-agent > XDG_DATA_HOME > /opt /usr/local 系统路径
- `api/config.py:get_config / reload_config` — mtime + path 双维缓存的 YAML config 加载；_cfg_has_in_memory_overrides() 检测测试 monkeypatch 覆盖，避免 profile 切换触发误重载；reload 时 bust models disk cache
- `api/config.py:get_config_for_profile_home` — 解决 worker thread 不继承 per-request thread-local profile 上下文的问题：对已知 profile_home 直接读文件，不污染全局缓存
- `api/config.py:resolve_model_provider` — 解析 model_id 的四种格式(@provider:model / provider/model / bare / custom_providers 匹配)为 (model, provider, base_url)，含 local server provider 判断（不剥 namespace）
- `api/config.py:_build_nous_featured_set` — 从 Nous Portal 实时 catalog 挑选 featured 子集：sticky当前选中 > 静态 curated flagship > vendor 优先级 round-robin top-up，extras 用于 /model 自动补全全量覆盖
- `api/config.py:_deduplicate_model_ids` — 多 provider 暴露同 model ID 时 in-place 添加 @provider: 前缀（按 provider_id 字母序，首个不变保持向后兼容）
- `api/config.py:_is_local_server_provider / _base_url_points_at_local_server` — 双层判断本地 inference server（静态名单 + loopback/private IP heuristic），防止剥 namespace 破坏 Ollama/LM Studio 的 HF-style model registry key
- `api/config.py:print_startup_config / verify_hermes_imports` — 启动诊断：彩色输出探测到的路径/版本信息；尝试 import run_agent，失败时给出具体 env var fix-it 指引
- `api/auth.py:check_auth` — 请求鉴权门卫：PUBLIC_PATHS 豁免、cookie 验证、API 路径返 401 JSON、页面路径 302 到 /login?next= (带 percent-encode 防 query pollution)
- `api/auth.py:get_password_hash` — double-checked locking 单次计算 PBKDF2-600k hash：env var 优先、settings.json 次之；进程级缓存，_invalidate_password_hash_cache() 供 save_settings 调用
- `api/auth.py:verify_password` — 透明升级：先用当前 .pbkdf2_key 验，失败再用 legacy .signing_key 验，匹配则自动 re-hash 并持久化（迁移旧会话无感知）
- `api/auth.py:create_session / verify_session` — HMAC-SHA256 签名 token cookie；verify 时兼容 32-char 旧截断签名（30天过期后可删）；懒清理过期会话；持久化到 .sessions.json(0600)
- `api/auth.py:csrf_token_for_session / verify_csrf_token` — CSRF token 派生自 session token 的 HMAC(key, 'csrf:'+token)，绑定到会话生命周期，无需独立存储
- `api/auth.py:_check_login_rate / _record_login_attempt` — 基于 IP 的登录速率限制：5次/60s 窗口，尝试记录持久化到 .login_attempts.json，防暴力破解
- `api/auth.py:_passkey_feature_flag_enabled / are_passkeys_enabled` — Passkey 默认关闭：需 HERMES_WEBUI_PASSKEY=1 或 config.yaml 里 webui_passkey_enabled:true，且 passkeys.json 有注册凭证才启用
- `api/auth.py:set_auth_cookie / _is_secure_context` — Cookie 安全标志多优先级检测：HERMES_WEBUI_SECURE env > TLS socket > HERMES_WEBUI_TRUST_FORWARDED_PROTO opt-in(防 header injection)
- `api/oauth.py:resolve_runtime_provider_with_anthropic_env_lock` — Anthropic onboarding OAuth 与 chat stream 共享 _ENV_LOCK，防止 onboarding clear token 时 chat 看到残留 env var 的竞态
- `api/oauth.py:_normalize_onboarding_oauth_provider` — 归一化 OAuth provider 名：anthropic/claude/claude-code 都映射到 'anthropic'
- `api/passkeys.py:PasskeyError / PasskeyRateLimitError` — 用户可纠错的 WebAuthn 错误分层：基础错误与速率限制错误分开，前端可针对性提示
- `api/passkeys.py:_atomic_write_json` — temp file + os.replace + chmod 0600 原子写 JSON，统一模式：crash-safe + 权限安全
- `api/runtime_adapter.py:StartRunRequest / RunStartResult / RunEventStream / RunStatus / ControlResult` — RuntimeAdapter seam 的不可变 dataclass 协议类型，为 legacy-direct / legacy-journal / runner-local 三种执行模式提供统一接口
- `api/gateway_watcher.py:start_watcher / stop_watcher` — 后台 daemon 线程，每 5s 轮询 state.db(hermes-agent 的 SQLite)，检测 gateway sessions 变化，触发 SSE 推送 session-list-changed 事件
- `api/gateway_watcher.py:_cheap_change_fingerprint` — 只扫 sessions 表（无 messages JOIN）做廉价指纹，避免大型 state.db 每轮 O(n messages) 扫描；仅指纹变化时才做完整 projection
- `api/config.py:_resolve_configured_provider_id` — 双模式 provider 解析：resolve_alias=True 给 badge/picker，False 给 resolve_model_provider 保留原始名以便 _LOCAL_SERVER_PROVIDERS 成员检测（#1625 回归修复）

## 3. 页面元素穷举

### 前端 UI 渲染骨架 ui.js + panels.js

#### Offline Banner（#offlineBanner）
- **元素**：标题 offlineTitle + 详情 offlineDetails（browser/network 两种文案）+ 自动刷新提示 offlineAutorefresh + 「Check Now」按钮（disabled 时文案变 Checking...）
- **交互**：banner 出现时启动 2.5s 定时探针；点 Check Now 手动探针；后台 visibilitychange 触发 refresh；探针成功后软恢复（无 reload）
- **UX 细节**：区分 browser offline（navigator.onLine=false）和 network error（fetch 失败），展示不同文案；软恢复避免 Android PWA 每次后台化都 reload

#### Composer（消息输入区）
- **元素**：textarea #msg、发送/停止按钮 updateSendBtn、model chip #composerModelChip、reasoning chip、toolsets chip、profile chip #profileChipLabel、workspace dropdown chip、上传文件 tray、queue pill、mobile config 折叠按钮、图片粘贴/拖放区
- **交互**：Ctrl+Enter 或 Enter（依设置）发送；busy 时按钮变 Stop（发 /stop）；图片路径粘贴自动转为图片预览；drag 文件到 tray；IME Enter 正确区分
- **UX 细节**：所有 chip dropdown 互斥关闭（toggleModelDropdown 中 close 其他）；mobile 下折叠非必要 chip 到 config 菜单节省空间；queue pill 显示排队消息数

#### Model Dropdown（#composerModelDropdown）
- **元素**：scope 提示文本、搜索框（model-search-input + clear 按钮）、「Custom model ID」输入框 + use 按钮、Configured 分组（有 badge 的模型）+ 其他 provider 分组（每组带 count）、每个 model 行（name + provider chip + badge）、No results 提示
- **交互**：实时搜索过滤（input 事件）、Escape 关闭、Enter 在搜索框中不提交、搜索框自动获焦、custom input Enter 确认、点击外部关闭、resize 时重新定位
- **UX 细节**：Configured 组去重（语义 provider::normalizedModel key，相同模型不同 provider alias 只显示一个），按 badge.role primary/fallback 排序；custom model ID 允许输入任意 ID 直接使用

#### Reasoning Effort Dropdown
- **元素**：chip 显示当前 effort 标签、dropdown 列出 API 返回的支持 effort 列表（none/low/medium/high/auto）、每项带描述
- **交互**：点 chip 切换下拉、选中后写 session + 持久化、Escape 关闭
- **UX 细节**：从 /api/session 或 /api/model/reasoning 拉取支持列表，仅展示支持的选项

#### 消息列表（#messages / #msgInner）
- **元素**：加载更早消息 button（loadOlderIndicator）、日期分隔符、用户 bubble（可编辑、jump-to-question 按钮）、assistant turn（role header + tps + 时间戳）、thinking card（<details>可折叠）、tool card 组（activity group summary/details + 各工具卡片）、status card、compression reference card、preserved task list card、empty state
- **交互**：滚动到底 pin 模式（不在底部时停止跟随）、双击 msg 内联编辑（textarea 替换 DOM）、点 Edit 按钮、copy 按钮、speak 按钮、jump to session start 浮动按钮（右下角）、jump to question（↑ 箭头）、copy session ID
- **UX 细节**：tool card 折叠默认关闭，header 点击展开；diff 块自动着色（+/-/@@）；思考内容和可见回答去重；compression anchor 位置精确对齐压缩摘要

#### Tool Card（.tool-card）
- **元素**：running dot（流式时闪烁）、tool icon、tool name、preview text（arg 关键字摘要，隐藏 secret 参数）、展开 toggle、detail：args 键值对 + result pre（diff 或文本）、Show more/less 按钮
- **交互**：点 header 展开/收起；Show diff/Show more 展开完整 result；diff 展开时用 _colorDiffLines 着色
- **UX 细节**：arg 预览自动跳过 apikey/token/secret 等参数名（归一化 + 子串匹配）；subagent 卡片有特殊样式；data-memory-save/data-skill-update 标注在 data-* 上跨 innerHTML 序列化存活

#### Activity Group（live turn .tool-call-group）
- **元素**：summary 行（chevron + 计数摘要 + elapsed timer）、details 展开内容（agent-activity-status 事件行：run/model/thinking/tool/waiting/done/warning）、duration label
- **交互**：点 summary 展开/收起（状态存 localStorage 按 turn key）、elapsed timer 每秒刷新（timer 跟随 DOM isConnected 自动停止）
- **UX 细节**：事件行按 data-activity-event-id 去重更新，避免重复追加；summary 合并 memory save / skill update 数量为可读摘要

#### Context Indicator（上下文 token 进度条）
- **元素**：progress bar（input+output+cache 分层颜色）、token 数字、compress 按钮（_setCtxCompressButton）
- **交互**：接近上限时变红；点 compress 发 /api/session/compress
- **UX 细节**：_mergeUsageForCtxIndicator 用 latest 和 fallback 两个 usage 对象合并，处理流式中间状态

#### 图片 Lightbox（.img-lightbox）
- **元素**：全屏 overlay、img 元素、close (×) 按钮、prev/next 箭头按钮、键盘左右 ESC
- **交互**：点图片打开、键盘导航、点 overlay 关闭、Escape 关闭
- **UX 细节**：多图时支持相册导航（index 追踪）；close 按钮绑定在 overlay 上防止误触

#### Toast 通知
- **元素**：toast div（.show 类控制）、复制文本按钮
- **交互**：默认 2.8s 消失（error 型 20s）；点 copy 复制 toast 内容
- **UX 细节**：type 参数影响颜色（error/info）；clearToastDismissTimer 允许 hover 保持

#### Confirm / Prompt Dialog（app dialog）
- **元素**：modal overlay、title、message、cancel 按钮、confirm 按钮（danger 时红色）、prompt 时 input 框
- **交互**：Escape dismiss、Enter confirm、focusCancel 选项改变初始焦点
- **UX 细节**：Promise 化 API；_isAppDialogOpen 保证同时只有一个 dialog；焦点在按钮间 tab trap

#### 文件树（#workspaceFiles / .file-tree）
- **元素**：面包屑导航（每段可点击 + drag-drop 目标）、目录 expand toggle（▸/▾）、文件图标（按扩展名）、文件名（单击 debounce / 双击 rename）、文件大小、delete (×) 按钮、kebab prefs menu（Show hidden files toggle）
- **交互**：单击目录展开/收起（S._expandedDirs Set）、单击文件打开预览、双击文件 rename（input 替换 span，Enter/Escape/blur 确认/取消）、右键 context menu（Rename/Copy path/Download/Delete）、拖拽移动文件/目录、OS 文件拖入上传
- **UX 细节**：click vs dblclick 300ms debounce 精确区分；S._dirCache 缓存子目录内容；tree depth 以 paddingLeft 缩进（8+depth*16px）

#### Kanban 看板面板
- **元素**：board 切换菜单（board 名 + color dot）、列头（列名 + 任务数）、看板卡片（id/priority badge/title/body/assignee/comment count/age/staleness）、侧边栏（列统计）、过滤栏（assignee/tenant/archived/onlyMine select）、任务详情面板（右侧）、任务创建/编辑 modal、stats bar、read-only banner
- **交互**：卡片拖拽换列（HTML5 DnD）、点卡片打开详情、双击编辑、board 切换持久化到 localStorage、SSE 实时更新（失败 fallback 30s 轮询）、bulk select + dispatch
- **UX 细节**：staleness class 按 age 分级（新/正常/旧）；拖拽和点击区分（300ms suppress）；kanban board 支持 profile-lane 视图（按 assignee profile 分行）

#### Tasks（Cron）面板
- **元素**：左列：cron job 列表（name/status badge/next run/new dot）、「+」创建按钮；右侧：detail card（status/schedule/next/last/deliver/mode/profile/skills/prompt）+ 运行历史列表；header 按钮（Run/Pause/Resume/Edit/Duplicate/Delete/Cancel/Save）
- **交互**：点列表项打开 detail + active highlight + 清除未读 dot；展开/收起 prompt 和 run output；点 run 条目加载 content；detail header 按钮按 mode 切换显隐
- **UX 细节**：运行中 job 自动注入 running indicator 并轮询；后台完成时 toast + nav tab badge；badge 按 job 逐个清除（非批量）

#### Insights 面板
- **元素**：系统健康卡（CPU/mem/disk progress）、LLM wiki 状态卡、Skill 调用统计、Overview 统计格（sessions/messages/tokens/cost）、每日 token 柱状图（input/output 分层）、模型用量表（model/sessions/tokens/cost/share%）、星期活跃度条形图、时段（小时）活跃度条形图
- **交互**：日期范围 select 切换（7d/30d/90d 等）、柱 hover 显示 tooltip（title attr）
- **UX 细节**：日期分桶：数据点过多时合并为周/月，避免横向溢出；每日图 input 叠加在 output 上方（stacked bar）

#### Skills 面板
- **元素**：搜索框、skill 列表（按 category 分组，每组 header 可折叠）、每个 skill 行（enable toggle/name/description）、右侧 detail（markdown 渲染）、create/edit 表单（name/category/content）
- **交互**：搜索实时过滤、category collapse（_collapsedCats Set 持久）、enable toggle 即时更新、detail 带 linked files 链接
- **UX 细节**：YAML frontmatter 在 detail 中剥除；_enhanceSkillMarkdown 将文件引用转超链接

#### Memory 面板
- **元素**：section 列表（core/project/profile）、当前 section detail（mtime + markdown 内容）、edit 区（textarea）、外部 notes source 选择（Notion/Obsidian）
- **交互**：点 section 选中 + 加载 detail、edit 按钮切换编辑/读取、save/cancel
- **UX 细节**：外部 notes source 选中后显示连接状态，数据通过 /api/memory/external 路由

#### Workspaces 面板
- **元素**：workspace 列表（name/path/default badge）、detail card（path/description/actions）、create/edit 表单（name/path，path 有 autocomplete suggestion）、workspace dropdown（在 composer 中共用）
- **交互**：选中 workspace 加载 detail、path 输入 debounce 300ms 拉 suggestions、上下键 + Enter 导航建议列表
- **UX 细节**：suggestion 列表 fixed 定位对齐输入框；workspace dropdown 在 composer 和 panel 两处用同一个 renderWorkspaceDropdownInto 函数

#### Profiles 面板
- **元素**：profile 列表（name/active badge）、概念帮助卡（首次引导）、detail（model/instructions/skills/active state）、create 表单、profile dropdown（在 composer chip 中渲染）
- **交互**：点 profile 开 detail、switch profile（/api/profile/switch）、create form 提交
- **UX 细节**：_refreshProfileSwitchBackground 用 generation counter 防止切换竞态

#### Settings 面板（主内容区）
- **元素**：侧边 section menu（Conversation/Appearance/Preferences/Providers/Plugins/System）+ mobile select dropdown、各 pane（conversation/appearance/preferences/providers/plugins/system）、unsaved changes bar（Discard/Save 按钮）、tab visibility chips（可拖排序，role=switch）
- **交互**：section 切换时 lazy load providers/plugins；dirty 时拦截 panel 切换并显示 bar；Discard 还原；chip drag reorder；chip toggle 显隐
- **UX 细节**：appearance 和 preferences 各自独立 autosave（1s debounce + 5s retry）；plugins pane 隐藏时 deep link 自动 fallback 到 conversation

#### Providers 设置子页
- **元素**：provider card 列表（name/meta/badge/API key 输入/test 按钮/remove 按钮）、quota card（status badge/pool 展开/used/limit/reset 时间/refresh 按钮）
- **交互**：API key 明文输入后 save；test 触发 /api/provider/test；quota refresh 请求 /api/provider/quota?refresh=1
- **UX 细节**：quota pool 展开状态存 localStorage；exhausted 状态显示 retry-after 时间；last checked 时间 toLocaleString

#### Plugins 设置子页 + Plugin Page（iframe）
- **元素**：plugin card 列表（name/version/key/description/hooks badge/activation badge/enable toggle/open link）、plugin page container（sandboxed iframe）
- **交互**：enable toggle → PATCH /api/plugins/:key；open link 载入 iframe；iframe sandbox=allow-scripts,allow-forms,allow-popups
- **UX 细节**：onclick 绑定分离（后绑定闭包）防 plugin key/path 注入；provider plugin 不显示 lifecycle hooks（仅显示 provider 解释）

#### MCP 管理子页（Settings → System）
- **元素**：server 列表行（name/transport badge/status badge/tool count/enable toggle）、tools 搜索框 + per-page select + 分页 + tool 行（name/server/status/description/schema summary）
- **交互**：toggle server → PATCH /api/mcp/servers/:name；搜索 filter 实时重渲染；分页 prev/next
- **UX 细节**：schema summary 用 * 标注 required 参数；tools per page 选项 5/10/20/40；搜索跨 name+server+description 联合匹配

#### Logs 面板
- **元素**：文件选择 select（log files）、tail 行数 select、severity 过滤 select（all/debug/info/warn/error）、wrap toggle、log 内容区（行着色）
- **交互**：30s 自动刷新（panel 激活时）；参数变化立即刷新
- **UX 细节**：severity 按正则检测行内容（error/warn/info/debug 关键字）；wrap toggle 控制 pre 的 white-space

#### Tab 可见性 Chips（Settings → Appearance）
- **元素**：每个非固定 tab 对应一个 chip（label text），可拖排序、点击 toggle 显隐
- **交互**：drag-start/drag-over/drop 重排（操作 .rail 和 .sidebar-nav 两处 DOM）；toggle 存 localStorage；拖拽后 suppress 250ms 防误点
- **UX 细节**：role=switch + aria-checked 比 aria-pressed 语义更准确（on/off 而非 pressed/not-pressed）；ALWAYS_VISIBLE_TABS 白名单保护 chat/settings 不被隐藏

#### Background Error Banner（#bgErrorBanner）
- **元素**：警告图标 + 消息文案（单条/多条）、View 按钮、Dismiss 按钮
- **交互**：FIFO 队列：View 跳转到最老的出错 session 并 shift 队列；Dismiss 清空所有错误
- **UX 细节**：只追踪非当前 session 的错误；sticky 插入 messages 区域上方

### 会话列表 + 消息渲染 (sessions.js + messages.js)

#### 会话列表（#sessionList）
- **元素**：每行：会话标题文本、最后活动时间戳（相对格式）、流式进行中 spinner、未读圆点、手动状态图标（todo/in-progress/done）、三点操作按钮（.session-actions-trigger）、长按/右键上下文菜单、复选框（batch select 模式）、左滑露出「archive」、右滑露出「delete」的 swipe affordance
- **交互**：点击行 → loadSession；三点按钮 → openSessionActionMenu；左/右滑动 → archive/delete（阈值 128px）；长按（400ms）→ 上下文菜单；双击标题 → 内联重命名；Ctrl/Cmd+A → 全选；Escape → 关闭菜单；勾选复选框 → 批量选择
- **UX 细节**：FLIP 动画：列表重排时各行平滑滑动到新位置（--session-reflow-offset CSS 变量），respects prefers-reduced-motion；流式进行中行有旋转 spinner；后台完成时列表项自动出现未读圆点（无需用户刷新）；切换 session 时当前行立即高亮，列表不重渲染

#### 会话上下文菜单（.session-action-menu）
- **元素**：固定顺序菜单项：复制链接 / 重命名 / 置顶・取消置顶 / 移至项目 / 归档・还原 / 复制会话 / 停止响应（仅流式中显示）/ 重新生成标题 / 手动状态（todo/in-progress/done） / 删除 worktree / 删除
- **交互**：菜单项 button.onclick，危险操作（删除/worktree）有 danger CSS 类；点击菜单外或 Escape 关闭；window.resize 时自动重新定位；短边剪辑超高菜单时添加 max-height + overflow-y:scroll
- **UX 细节**：入场动画（Web Animations API：opacity 0→1 + translate3d -4px→0 + scale .985→1，450ms cubic-bezier(.2,.8,.2,1)）；meta 描述降级为 hover tooltip 而非常驻文字，保持菜单紧凑；外部 session（CLI/messaging）菜单只有「复制链接 + 隐藏」

#### 批量操作栏（#batchActionBar）
- **元素**：已选数量徽章、批量归档按钮、批量移至项目（内联 project-picker）、批量删除按钮（danger）；project-picker 内联 dropdown 含「无项目」和所有项目列表（含颜色圆点）
- **交互**：选择数量 > 0 时显示（flex），= 0 时隐藏；批量操作前弹 confirm dialog（含 worktree 会话特殊文案），Promise.all 并发执行
- **UX 细节**：批量删除当前会话时自动加载下一条会话或显示空状态，不留用户在已删 session

#### composer 输入区
- **元素**：#msg textarea（自动高度，max 200px）、发送按钮（send/stop 切换）、模型选择 chip、文件 tray（pendingFiles）、YOLO 徽章（#yoloPill）、队列数徽章（#bgBadge）、composerStatus 状态文本
- **交互**：输入 → autoResize()；Enter 发送（若有 shift 换行）；斜杠命令补全（/cmd）；文本选中时浮出「Reply with selection」浮动按钮；focus → 暂停 TTS；blur → 恢复 TTS
- **UX 细节**：busy 时发送不被丢弃而是走 queue/steer/interrupt 三种模式（busy_input_mode 配置）；发送前自动保存草稿；切换 session 时 draft 保存到服务器；stale busy 状态在发送前自动检测并清除

#### 消息流（#msgInner）
- **元素**：user 气泡（含附件列表）、assistant 气泡（含 Thinking 折叠卡片、reasoning 文本、live tool card 列表、markdown/代码块/KaTeX/mermaid 渲染）、tool 结果行（name + preview + snippet + error 状态）、live assistant segment（.assistant-segment[data-live-assistant=1]）、handoff summary 卡片（pseudo tool 消息）、btw 旁路气泡（.msg-row-btw）、Empty state
- **交互**：滚动到顶部 → 自动加载更多消息（_loadOlderMessages）；「Jump to start」胶囊 pill；选中文字 → 浮出 Reply 按钮；代码块 → 复制按钮 + 语法高亮；文件路径 → 图片预览 lightbox；```html/svg/mermaid → 沙箱预览
- **UX 细节**：流式输出时 smd 逐字符增量 DOM 构建（非 innerHTML 全量），配合 rAF 66ms 节流；stream fade 模式下每词分 <span> 淡入错开；thinking 卡片：流式中显示动态内容，done 后折叠为静态摘要；tool 卡片 preview/snippet 区分进度文本和结果文本；INFLIGHT 恢复时 assistantBody 已有内容，smd 清空重建

#### 工具审批卡（#approvalCard）
- **元素**：命令预览（#approvalCmd）、描述（#approvalDesc）、四个响应按钮（Once/Session/Always/Deny）、「N of M pending」计数、折叠/展开 chevron 按钮
- **交互**：Once/Session/Always/Deny 按钮点击 → respondApproval(choice)，发送前立刻 disable 所有按钮防双提；折叠按钮 → toggleApprovalCardCollapsed；最短可见 30s（防误操作后消失）；SSE 推送立即显示，降级 1.5s 轮询
- **UX 细节**：卡片高度变化时动态设置 --approval-card-height CSS 变量，消息列表添加 padding-bottom 确保不被遮挡；展开/折叠时滚动跟随（near-bottom 检测）；切换 session 后自动清除；多个待审批时显示「1 of N pending」

#### Clarification 卡（#clarifyCard，动态创建）
- **元素**：问题文本（#clarifyQuestion）、选项按钮列表（含编号徽章 + Other 按钮）、自由输入框（#clarifyInput）、Submit 按钮（#clarifySubmit）、倒计时（#clarifyCountdown，秒级，<10s 变 urgent 样式）、折叠 chevron
- **交互**：点击选项 → respondClarify(choice)；Enter 键提交；倒计时到期或 terminal 时 draft 自动移入 composer（_stashClarifyDraft）；SSE 推送 + 60s 静默重连 + 降级 3s 轮询
- **UX 细节**：聚焦时锁定 composer（lockComposerForClarify）；关闭时解锁；draft 迁移到 composer 防止用户输入丢失；倒计时 10s 以下切换 urgent 文字颜色；Same question 不重置 input（sameClarify 守卫）

#### Handoff 提示条（#handoffHintBar，dock 底部固定）
- **元素**：绿色圆点、渠道标签（WeChat/Telegram 等）+ 轮次数量文本、「View summary」按钮、「Close」关闭按钮
- **交互**：点击条体 → _generateHandoffSummary；Close → _dismissHandoffHint（写入 dismissed_at）；发送消息自动 dismiss；--handoff-dock-height 动态调整消息区域底部空间
- **UX 细节**：only shown for messaging session（WeChat/Telegram/Discord/Slack/Email/WeCom）且超过 10 轮且用户未手动关闭；摘要以假 tool 消息插入，不污染上下文；生成中显示 running 状态，失败显示 retry 状态

#### 会话搜索框（#sessionSearch）
- **元素**：input 文本框、搜索命中高亮（span.session-search-hit）、content 搜索预览片段
- **交互**：输入防抖搜索 → 过滤 _allSessions；content 搜索调后端 /api/sessions/search，结果以 match_preview 展示；选中结果后隐藏预览（_hideSearchPreviewsAfterSelect）
- **UX 细节**：命中词高亮支持多词分段匹配、URL token 清洗、session ID 候选提取；content 搜索结果与 title 搜索合并展示

### 启动引导 + 命令面板 + 工作区 (boot/commands/workspace)

#### 工作区面板 (rightpanel) / workspace panel
- **元素**：Files tab / Artifacts tab 切换标签；文件树 (#fileTree)；面包屑导航 (#breadcrumbBar)；git 徽章 (#gitBadge，显示 branch/dirty△/behind↓/ahead↑)；工具栏（上级目录、新建文件、新建文件夹、刷新、折叠）；文件预览区 (#previewArea，含 code/md/image/html/pdf/audio/video 六种模式）；内联编辑 textarea (#previewEditArea)；编辑/保存按钮 (#btnEditFile)；大 markdown 强制渲染按钮 (#btnRenderMarkdownAnyway)；在浏览器中打开按钮 (#btnOpenInBrowser)；Artifacts tab 文件列表（file-path + source 来源）；OS 文件拖拽上传目标（file-item[data-ws-type=dir] + fileTree 本身）
- **交互**：Files/Artifacts tab 切换；文件树节点点击预览/展开目录；面包屑段点击 loadDir；git 徽章显示状态（dirty 时加 .dirty class）；工具栏按钮；预览区 Prism 高亮；编辑模式 Escape 取消/Ctrl+S 暂无（通过 updateEditBtn 切换状态）；Artifacts 点击打开文件；拖拽 OS 文件到目录节点或 fileTree 触发上传
- **UX 细节**：previewCurrentPath 在 loadDir 时不清零（keepPanelOpen 选项）防止文件预览因目录刷新而消失；大 markdown 文件（>256KB/5000行）降级纯文本并提供强制渲染按钮，避免卡顿；binary 文件服务端 flag 后直接触发 download，不尝试渲染

#### 工作区下拉选择器 (wsDropdown / composerWsDropdown)
- **元素**：搜索输入框（前端实时 filter by name+path）；清除按钮；按 name 排序的工作区列表（active 高亮）；分隔线；「新建 worktree 对话」操作项；「手动输入路径」操作项；「管理工作区」操作项
- **交互**：topbar chip 点击（toggleWsDropdown）或 composer workspace chip 点击（toggleComposerWsDropdown）；列表项点击 switchToWorkspace；footer 操作项点击；点击外部关闭；window resize 时重新计算 composerWsDropdown left 定位
- **UX 细节**：两个 dropdown 实例（topbar + composer）共享 renderWorkspaceDropdownInto 逻辑，但定位逻辑独立；composerWsDropdown 用 chipRect.left - footerRect.left 实现相对 composer footer 的对齐

#### 工作区管理面板 (workspacesPanel sidebar panel)
- **元素**：工作区行列表（name + path，active badge，drag handle）；详情/空 placeholder；详情视图（name/path/status/checkpoint 列表）；Activate/Edit/Delete/Cancel/Save 按钮（按状态显隐）；新建/编辑表单（name 输入 + path 输入 + 路径 suggestion dropdown + 错误提示）
- **交互**：行点击展开详情；drag handle 拖拽排序（dragstart/dragover/drop）；POST /api/workspaces/reorder 持久化；Activate 按钮调 switchToWorkspace；Edit 进入表单模式（path 禁用）；Delete confirm dialog；新建表单 path 输入时 /api/workspaces/suggest 路径补全（ArrowUp/Down/Enter/Tab/Escape）
- **UX 细节**：isDefault 的工作区不显示 Delete 按钮；edit 模式 path 只读防止误操作；cancelWorkspaceForm 如有 _workspacePreFormDetail 快照则恢复原详情，否则回到 empty 状态（两路取消）

#### 斜杠命令补全下拉框 (cmdDropdown)
- **元素**：命令条目（/name [arg] + desc）；skill 来源有 badge；subarg 条目显示「/parent subarg」格式；path 类型条目显示路径值；键盘选中高亮（.selected class）
- **交互**：输入 / 触发；ArrowUp/Down 导航；Tab/Enter 选中；Escape 关闭；path 类型选中后原地 token 替换并追加 /；subarg 选中后填 /cmd value；无 arg 的命令选中后不加空格，有 arg 的加空格并触发子参数补全
- **UX 细节**：navigateCmdDropdown 调 scrollIntoView({block:'nearest'}) 确保键盘导航时选中项可见；六种来源差异渲染（badge/class/格式）帮助用户识别条目类型

#### Composer 输入区及配套控件
- **元素**：消息 textarea (#msg)；发送按钮 (#btnSend)；附件按钮 (#btnAttach)；麦克风按钮 (#btnMic，含录音状态动画）；语音模式切换按钮 (#btnVoiceMode，条件显示）；语音状态栏 (#voiceModeBar，listening/thinking/speaking 状态）；语音模式状态标签 (#voiceModeLabel)；Steer 注入指示器 (.steer-indicator)；composer workspace chip (#composerWorkspaceChip)；model select (#modelSelect)
- **交互**：Enter/Ctrl+Enter 发送（可设置）；移动端触屏判断逻辑（pointer:coarse + visualViewport shrink → Enter 默认换行）；粘贴图片自动检测 kind=file 拦截转 addFiles；/开头触发命令补全、~/ 触发路径补全；composer draft 防抖存 server；麦克风点击 → SpeechRecognition 或 MediaRecorder 录音 → 转录或 raw audio；voice mode 链式 STT→send→TTS→STT
- **UX 细节**：IME 输入（东亚输入法）三重 guard：isComposing + keyCode===229 + _imeComposing flag + blur 重置，防 Safari 的 compositionend 后跟随的 Enter keydown 误触发发送 (#1443)；物理键盘检测 (any-pointer:fine) 防止平板蓝牙键盘被误判为软键盘

#### 主题/皮肤选择器 (settings panel)
- **元素**：主题选择 Grid（Light/Dark/System 三个按钮，每个有三色预览点）；皮肤选择 Grid（16 种色包，每个有三色点 + 名称）；字体大小选择 Grid
- **交互**：点击按钮调 _pickTheme / _pickSkin；同步 localStorage + server /api/settings + CSS data-theme/data-skin + meta theme-color；legacy 主题名映射（monokai/slate/nord）
- **UX 细节**：system 主题通过 matchMedia prefers-color-scheme 监听切换，移除旧 listener 再注册新 listener 防内存泄漏；meta theme-color 从 CSS var(--sidebar) 读取，支持所有自定义 skin 颜色正确反映到移动端状态栏

### 终端 + 引导 + 登录 + PWA

#### 终端面板（composerTerminalPanel）
- **元素**：顶部 toolbar（workspace 名称标签、collapse/expand 按钮、restart 按钮、clear 按钮、copy 按钮、close 按钮）；拖拽高度调节 handle（terminalResizeHandle）；xterm.js 渲染区（terminalSurface）；展开/折叠 toggle 按钮（btnTerminalToggle，在 composer toolbar 上）
- **交互**：点击 toggle → open/collapse 切换；拖拽 handle → 调整高度（pointerdown/move/up + pointer capture）；键盘 ArrowUp/Down/PageUp/Down/Home/End 调整高度（无障碍）；输入 exit/quit/logout/close 回车 → 关闭面板；xterm 内容输出时若处于 collapsed 态可自动展开
- **UX 细节**：三态：closed（面板隐藏）/ expanded（全高）/ collapsed（dock 条，仅显示 workspace 名）。消息列表底部 padding 用 CSS 变量动态补偿，保持底部对齐。折叠不断开 PTY，避免 shell 状态丢失。

#### 新手引导覆盖层（onboardingOverlay）
- **元素**：左侧步骤指示器（步骤号 + 标题 + 描述，done/active/pending 三种样式）；右侧主体（每步不同内容）；Back/Continue 按钮；Skip 链接
- **交互**：系统检查步骤：只读展示 hermes/provider/password 三项状态卡片；Provider 设置步骤：provider <select>（分 category optgroup）+ API key 密码输入 + 可选 base_url 输入 + Test 按钮 + probe 状态 banner + Claude Code OAuth 卡片；workspace 步骤：workspace <select> + 手动路径 input + model <select/input>；password 步骤：password 密码 input；finish 步骤：只读摘要；Continue 在 setup 步会同步执行 probe，失败则显示错误 notice 并阻断
- **UX 细节**：probe banner 三态：probing（旋转提示）/ ok（绿色：N models available）/ error（红色：本地化错误+detail）。key_optional provider 的 API key 字段显示 '(optional)' 且空值放行，避免 Ollama 用户被迫输入假 key。

#### OAuth 流程卡片（内嵌于引导 setup 步）
- **元素**：Codex OAuth：verification_uri 链接 + user_code 大字显示（可 copy）+ Copy code 按钮 + Cancel 按钮 + 轮询状态文字；Anthropic/Claude Code OAuth：'run claude setup-token' 命令展示 + Cancel + 轮询等待；终态卡片（success/expired/cancelled/error）
- **交互**：点击 'Login with Claude Code' / Codex 按钮触发 OAuth 启动；copy code 一键复制；cancel 取消并通知后端 /api/onboarding/oauth/cancel；3s 轮询自动检测完成；终态 success 自动 reload onboarding 状态
- **UX 细节**：user_code 用大字体（18px）+ letter-spacing + 浅色背景块，方便移动端手动输入；轮询期间按钮禁用显示省略号防双击。

#### 登录页（/login route）
- **元素**：密码 input（id=pw）；登录 button；Passkey 登录按钮（id=passkey-login，按需显示）；错误信息区（id=err）
- **交互**：password input 回车 = 点提交（keydown Enter 拦截）；Passkey 按钮：POST /api/auth/passkey/options → navigator.credentials.get → POST /api/auth/passkey/login；页面加载时探 /health，不可达则禁用表单并 3s 重试
- **UX 细节**：Passkey 按钮仅在 PublicKeyCredential 可用且 /api/auth/status 返回 passkeys_enabled 时显示；i18n 字符串通过 data-invalid-pw / data-conn-failed 属性从 HTML 传入，不需要服务端注入 JS 字面量。

#### PWA 安装提示（body class 控制，非独立页面）
- **元素**：pwa-installable class 存在时可显示'安装到主屏'按钮（由主 UI 根据 class 决定是否渲染）；pwa-installed/pwa-standalone/pwa-browser/pwa-ios/pwa-offline 等状态 class
- **交互**：点击安装按钮 → HermesPWA.promptInstall() 调起浏览器原生安装弹窗；PWA shortcuts（manifest.json）支持'New conversation'快捷方式（?action=new-chat）
- **UX 细节**：manifest.json 声明 display_override:[window-controls-overlay,standalone,minimal-ui]，支持最新 Window Controls Overlay API；theme_color 与 background_color 统一为 #0D0D1A（深夜蓝），installed 态标题栏配色一致。

### 页面 DOM 骨架 + 主题/设计 token (hermes-webui)

#### 顶部标题栏 (.app-titlebar)
- **元素**：汉堡菜单按钮(btnHamburger)、Hermes 品牌 logo（内联 SVG 金色渐变）、标题文字(appTitlebarTitle)、副标题(appTitlebarSub 默认 hidden)、右侧弹性占位、重载按钮(btnReload)
- **交互**：汉堡 -> toggleMobileSidebar()；重载 -> window.location.reload()
- **UX 细节**：logo 用内联 SVG linearGradient 实现金色渐变，不依赖图片资源；副标题默认 hidden，panel 切换时 syncAppTitlebar() 动态更新

#### 左侧 Rail（桌面竖向导航 .rail）
- **元素**：11 个图标按钮（chat/tasks/kanban/skills/memory/workspaces/profiles/todos/insights/dashboard/logs）+ rail-spacer + 设置按钮；每个按钮有 data-panel 属性 + data-tooltip
- **交互**：点击 -> switchPanel(name, {fromRailClick:true})；active 态在按钮左侧用 ::before 伪元素画 3px 竖条；dashboard 按钮条件隐藏（auto-detect 模式）
- **UX 细节**：rail 宽 48px，按钮 36x36，gap 4px；media min-width 641px 才显示；tooltip 在右侧 8px offset 处出现，150ms 延迟；dashboard 按钮右上角有外部链接徽章

#### 左侧 Sidebar（.sidebar）含 panel 系统
- **元素**：sidebar-nav（移动端横向 tab）、11 个 .panel-view（#panelChat/#panelTasks/#panelKanban/#panelSkills/#panelMemory/#panelTodos/#panelInsights/#panelWorkspaces/#panelProfiles/#panelLogs/#panelSettings）、拖拽调宽 .resize-handle、session 搜索框
- **交互**：switchPanel() 控制 panel-view active 状态；resize-handle 支持拖拽改宽；session 搜索框 oninput 调 filterSessions()
- **UX 细节**：sidebar 默认 300px，transition: width .24s cubic-bezier(.22,1,.36,1)；折叠时 width=0 + opacity=0；panel-head 有统一标题 + 右侧操作按钮区；Settings panel 是纯菜单列表，点击在 .main 区渲染 settings-pane

#### Chat 侧边栏面板 (#panelChat)
- **元素**：panel-head（Chat 标题+新建按钮）、session 搜索框（id=sessionSearch + 清空按钮）、会话列表（#sessionList 动态渲染）
- **交互**：新建按钮 -> btnNewChat；搜索 -> oninput:filterSessions()；会话列表项点击加载对话
- **UX 细节**：搜索图标用绝对定位放在 input 左侧 22px 处；clear 按钮默认 hidden，有内容时出现

#### Scheduled Jobs 面板 (#panelTasks / #mainTasks)
- **元素**：侧边栏: panel-head（刷新+新建）+ #cronGatewayNotice + #cronList；主区: 任务详情视图（title + run/pause/resume/edit/duplicate/delete/cancel/save 按钮 + 详情 body）
- **交互**：新建 -> openCronCreate()；运行 -> runCurrentCron()；暂停/恢复 -> pause/resumeCurrentCron()；cron 表达式校验实时提示
- **UX 细节**：cron 详情包含运行历史列表，可展开查看 agent 输出内容；gateway notice 当 Telegram/Discord 等渠道未配置时显示警告

#### Kanban 面板 (#panelKanban / #mainKanban)
- **元素**：侧边栏: 搜索框+分配人下拉+租户下拉+include-archived/only-mine checkbox+bulk 操作栏+快速新建输入+kanban-list；主区: 看板列（triage/todo/ready/blocked/done/archived）+board 切换下拉+dispatcher 按钮
- **交互**：dispatcher dry-run -> nudgeKanbanDispatcher()；正式运行 -> runKanbanDispatcher()；bulk 状态更新 -> bulkUpdateKanban()
- **UX 细节**：board 切换下拉带 emoji 图标+颜色标识；kanban-task-preview 在主区顶部展示选中任务详情；看板卡片支持拖拽调整状态（通过 dispatcher 认领）

#### 主聊天区 (#mainChat)
- **元素**：messages-shell（含 session-jump-btn Start/End）、#messages（含 .empty-state 空状态 + .messages-inner + #liveCompressionCards + #liveToolCards）、reconnect-banner、offline-banner、agent-health-banner、更新横幅(#updateBanner)
- **交互**：jumpToSessionStart()/scrollToBottom()；empty-state 有 3 个 suggestion 按钮快速触发预设 prompt；reconnect-banner 可 Dismiss 或 Reload
- **UX 细节**：empty-state 展示完整 Hermes caduceus SVG 金色 logo（80x80）；suggestion-grid 是 CSS Grid 3 列；live-tool-cards 在 streaming 时展示工具调用进度

#### Composer 输入区 (.composer-wrap / .composer-box)
- **元素**：queue-card（排队消息卡片）、approval-card（权限审批卡: once/session/always/deny/YOLO 5 按钮）、clarify-card（AI 澄清卡: 多选项+自由输入）、composer-terminal-panel（内嵌终端）、handoff-hint-container、queue-pill、composer-box（主输入框+dropzone）、attach-tray、mic-status、voice-mode-bar、textarea#msg、composer-footer
- **交互**：approval: respondApproval('once'/'session'/'always'/'deny')；clarify: respondClarify()；mic: btnMic(dictate)/btnVoiceMode；attach: btnAttach->fileInput；composer-terminal: 全屏/收起/清除/重启/复制
- **UX 细节**：approval-card 和 clarify-card 从 composer 下方向上滑出（flyout 模式）；approval-card 支持折叠(collapse)；YOLO pill 在激活时 composer 左侧展示；terminal resize handle 支持上下拖拽调整高度

#### Composer Footer（工具栏）
- **元素**：左侧: 文件附件(btnAttach)、麦克风(btnMic)、声音模式(btnVoiceMode)、分隔线、YOLO 模式 pill、profile-chip（当前 profile+切换）、workspace-chip（文件夹图标+名称+切换）、移动端配置按钮(ctx-ring SVG)、model-chip（型号+切换）、provider-quota-chip（余额）、reasoning-chip（推理等级）、toolsets-chip（工具集）；右侧: composer-status 文字、ctx-indicator（context 环形进度）、bg-badge（后台任务数）、send-btn
- **交互**：profile/workspace/model/reasoning/toolsets 各有 dropdown；ctx-indicator 展开 tooltip 显示用量详情 + compress 按钮；ctx-ring 是移动端紧凑版（SVG arc + 数字）
- **UX 细节**：ctx-indicator 是 SVG 环形进度条（圆弧 stroke-dasharray 动画），中心显示百分比；model-chip 下拉按 OpenAI/Anthropic/Other 分组；reasoning 下拉有 none/minimal/low/medium/high/xhigh/max 7 档；provider-quota-chip 仅 >=1400px 且用户开启时可见

#### 右侧 Workspace Panel (.rightpanel)
- **元素**：panel-header（标题+hidden-indicator+git-badge+panel-actions: up-dir/new-file/new-folder/refresh/upload/prefs/close-preview）、workspace-panel-tabs（Files/Artifacts）、breadcrumb-bar、#fileTree、#workspaceArtifacts、#wsEmptyState、preview-area（preview-path+preview-code/img/media/pdf/md/html/edit-area 多种预览模式）
- **交互**：tab 切换 -> switchWorkspacePanelTab('files'/'artifacts')；navigateUp()；promptNewFile/Folder()；loadDir()；preview 支持代码/图片/视频/PDF/Markdown/HTML iframe(sandbox)；edit 模式内联编辑保存
- **UX 细节**：git-badge 展示仓库当前分支；hidden-files indicator 点击弹出 workspace prefs 菜单；preview-path 右侧有 open-in-browser/download/edit 按钮；HTML preview 用 sandbox='allow-scripts allow-popups' iframe 隔离；workspace panel 宽 300px，transition cubic-bezier 展开动画

#### Settings 主面板 (#mainSettings / .settings-main)
- **元素**：5 个 settings-pane：Conversation（transcript 下载/JSON 导出导入/清空）、Appearance（theme 4 格 picker + skin 网格 picker + font-size 4 格 picker + tab 可见性 chips）、Preferences（model/send-key/language/RTL/TTS/通知等 30+ 个选项）、Providers（API Key 输入+quota 卡片）、System（version 徽章+password+passkey+dashboard link+gateway 状态+MCP 服务器+MCP 工具搜索）
- **交互**：skin picker 实时预览；tab visibility chips 拖拽排序（drag-and-drop）；TTS rate/pitch 用 range input；Providers: 动态刷新模型列表 + quota 状态颜色；System: registerPasskey/goPasswordless/shutdownServer/checkUpdatesNow
- **UX 细节**：Appearance picker 每个选项有小预览块（40px 高，展示实际背景色/图标）+ 文字标签；skin picker 用网格布局一次展示所有肤色；autosave-status 用 aria-live='polite' 实时反馈保存结果

#### Onboarding 覆盖层 (.onboarding-overlay)
- **元素**：onboarding-card（双栏: sidebar 有 steps 步骤指示器+标题+lead 文字；main 有 notice/body/actions 三段）；Back/Skip/Continue 按钮
- **交互**：nextOnboardingStep()/prevOnboardingStep()/skipOnboarding()；步骤引导：安装检测->工作区选择->模型选择->可选密码设置
- **UX 细节**：首次运行时显示；onboarding-steps 是可视步骤 stepper；modal 语义用 role=dialog aria-modal=true

#### 通用对话框 (.app-dialog)
- **元素**：app-dialog-overlay、app-dialog（role=dialog）、header（title+close）、desc、可选 input、actions（Cancel+Confirm）
- **交互**：全局 showDialog()/closeDialog() 调用；Confirm 按钮绑定 resolve 回调
- **UX 细节**：取代 window.confirm/prompt，保持主题一致性；backdrop 点击/Escape 关闭

#### Toast (#toast)
- **元素**：单个 #toast div，动态 innerHTML
- **交互**：showToast(msg, type) 弹出后 3s 自动淡出
- **UX 细节**：全局唯一实例，z-index 最高；不堆叠

#### Kanban 模态框 (#kanbanBoardModal / #kanbanTaskModal)
- **元素**：board modal: name/slug/desc/icon emoji 输入/color picker；task modal: title/description/status select/priority number/assignee select/tenant datalist
- **交互**：点击 overlay 背景关闭；form 提交 submitKanbanBoardModal()/submitKanbanTaskModal()；assignee 列表动态从 profiles 填充
- **UX 细节**：kanban-modal-row-inline 两列布局用于 icon+color 配对；tenant 支持 datalist 自动补全

#### Insights 面板 (#panelInsights / #mainInsights)
- **元素**：侧边栏: period select（7/30/90/365 天）+刷新按钮；主区: insights-card wiki-status-card + 动态统计卡片
- **交互**：period change -> loadInsights()；刷新 -> loadInsights(true)
- **UX 细节**：统计卡片结构：图标+数字+delta 变化量；这是 swarmx 最想借鉴的 Usage/Cost 可观测功能

#### docs/ui-ux/index.html（UI 组件库文档页）
- **元素**：doc-header（theme/skin 切换按钮组）、doc-main（分 section 展示所有消息类型）；复用真实 static/style.css，不额外维护 CSS
- **交互**：theme/skin 按钮通过 JS 改 html.class/data-skin 实时预览
- **UX 细节**：这是设计系统活文档：每个 UI 元素有 doc-label 标注 class 名，doc-card 包裹真实 DOM 结构；为开发者提供 visual regression 基准

## 4. swarmx 借鉴小结（本 repo）

| 优先级 | 借鉴点 | 价值 | 工作量 | 落到 swarmx 哪里 |
|---|---|---|---|---|
| P0 | Usage/Cost 可观测面板（仿 Insights） | 当前 swarmx 完全没有 token 消耗和费用可见性，无法感知哪个 worker/orchestrator 最费钱；hermes 的 Insights 有每日 token 柱状图（input/output 分层）+ 模型用量表 + 总 cost，已验证 UX 可行 | 后端：在 transcript.rs/JSONL tail 中解析 usage 字段并写 SQLite；前端：复用 hermes 的 _renderInsights + _bucketDailyTokensForChart 模板，适配 swarmx 的 per-agent/per-direction 维度 | 挂在现有 Insights/黑板页或新增独立面板；后端 AgentActivity 事件已有 token 字段基础（transcript.rs），只需聚合存库；前端纯 HTML 图表无框架依赖，可直接移植 |
| P0 | 后台 cron/定时编排（仿 hermes Tasks 面板） | swarmx 没有定时触发能力；hermes 的 cron 支持任意 cron expr + skills + profile + deliver 模式 + no-agent script，UI 有运行历史/展开 output/badge 未读数 | 后端：Rust tokio-cron-scheduler 或 cron crate，cron job 表存 SQLite，触发时 spawn worker（与现有 orchestrator 复用 spawn 逻辑）；前端：搬 panels.js 的 _renderCronForm/_renderCronDetail/_startCronWatch/updateCronBadge 约 800 行 | 新增 Cron 页面（左侧导航）；spawn 复用 swarmx 的 PTY spawn 流程；skill 对应 swarmx 角色注册表中的 role |
| P0 | ephemeral 字段携带（_carryForwardEphemeralTurnFields） | swarmx 当前 force-reload 会话时，活动行展示的 tool 名称、token 用量、耗时等 client-side 字段会丢失。hermes 的 _carryForwardEphemeralTurnFields 通过 (role\|timestamp\|content前160字符) 作 identity key，在任何 S.messages wholesale-replace 前把这些字段迁移到新数组，解决了相同问题。 | 低（纯前端逻辑，不需要 Rust 改动，swarmx 成员栏/活动 tab 的 JS 里加一个迁移函数即可） | chat.js 或 swarm-ui.js 的 loadSession / refreshMessages 路径，在替换 S.messages 前调用 carryForward，保留活动行的 tool/usage 字段。 |
| P0 | 竞态守卫：generation 计数 + loadingSessionId 令牌 | swarmx 多方向快速切换时，旧会话的 fetch 响应可能覆盖新 session 的数据，导致消息列表闪烁或错乱。hermes 的两层守卫（_loadingSessionId !== sid 短路、_renderSessionListGen 版本号）保证最新请求胜出。 | 低（纯 JS 逻辑，加两个模块级变量和每次 await 后的 guard check） | swarmx chat.js 的 loadSession / fetchMessages，以及方向列表的 renderDirectionList，加 generation + loading-id 两层守卫。 |
| P0 | 斜杠命令声明式注册表 + noEcho + fall-through 协议 | swarmx 现有命令面板是 MCP 入口菜单，没有 CLI 风格的斜杠命令系统。hermes 的 COMMANDS 数组设计极简却功能完整：声明式注册、noEcho 控制回显、handler 返回 false fall-through。swarmx 的聊天 composer 若加入 /compress /workspace /model /status 等斜杠命令，能大幅提升键盘效率，且不引入新的 UI 表面。 | 中：前端 command registry + parseCommand + Rust 后端对应 /api/commands endpoint（或复用现有 API） | 落在 chat/composer 输入层；Rust 后端不需改 PTY 逻辑，命令由前端拦截执行或透传给 agent PTY 输入（/steer 可直接向 PTY 注入，/compress 调现有 context 压缩接口） |
| P0 | 补全下拉框：命令名 + 子参数 + 文件路径三路聚合 + 六种来源差异渲染 | swarmx 目前的命令面板（Ctrl+K）不支持 composer 内行内补全。hermes 把「/开头」和「~/开头路径」做成两套独立补全，都汇聚到同一个 cmdDropdown UI 渲染，且 skill/agent/path 等来源有 badge 区分。这对 swarmx 的 workspace 路径输入和命令输入都极有价值。 | 中：前端补全 UI（token 就地替换逻辑值得直接移植）；后端 /api/workspaces/suggest 已有类似能力 | 落在 swarmx 前端 composer 组件；路径 token 就地替换逻辑可直接参照 _findComposerPathToken；补全来源扩展到 swarmx 的 role/skill/blackboard key |
| P0 | PTY env allowlist 策略（剥离 server API token） | swarmx 进程持有 ANTHROPIC_API_KEY / OPENAI_API_KEY 等，如果内嵌终端直接继承 process env，用户可 printenv 拿到所有 key。allowlist 是 security-by-default 设计，新加的 env var 不透传。 | 低：实现内嵌终端时跟随加上，仅在 spawn shell 时构造 safe_env HashMap 替换 std::env::vars() | src/terminal.rs:start_terminal 的 env 构造逻辑，SAFE_ENV_KEYS 同 hermes（PATH/HOME/USER/SHELL/LANG/TZ/TERM/COLORTERM 等）。 |
| P0 | Usage/Cost 可观测面板：token 用量 + cost + session 数分时段统计卡片 | hermes 的 Insights 面板（period=7/30/90/365 天 + 统计卡片 delta）是 swarmx 当前最大空白。worker 跑一次要多少 token/多少钱，用户完全看不到。对标 hermes loadInsights() + _statusCardHtml() | 中：后端加 /api/insights 汇总接口（读现有 SQLite agent_sessions/tool_calls 表）；前端加新 panel（参照 hermes insights-card CSS）；token 数据 swarmx transcript.rs 已在读，有原料 | 前端新增 panelInsights + mainInsights，复用 swarmx 现有 panel 框架；后端 routes/insights.rs 聚合 agents 表按时间窗口的 token/cost，cost 需知道 per-CLI 模型定价（可配置） |
| P0 | 闪屏预防 IIFE：在 head 内联脚本同步恢复 theme/font-size/panel 状态 | swarmx 冷启动有 theme flicker——先白屏再变暗色。hermes 用 6 段内联 script 在首绘前写好 html class/dataset，零 FOUC。简单但效果显著 | 低：把 swarmx index.html head 补几行内联脚本读 localStorage 写 document.documentElement，不依赖任何外部文件 | frontend/index.html 或 Tauri webview 的 initScript（Tauri 有 initScript 钩子，在页面脚本执行前注入） |
| P0 | Run journal（SSE 事件 append-only JSONL + seq cursor）替代/增强现有 JSONL tail 广播 | swarmx 现在通过 tail 读 claude/codex 会话 JSONL 广播 AgentActivity。run_journal 模式提供额外价值：(1) seq 编号让前端做游标增量拉取而非全量重推；(2) terminal 事件触发 fsync 保证崩溃后事件不丢；(3) worker 消失时合成 synthetic apperror 填 gap，前端可展示「worker 已断，是否重试」 | 中等：新建 Rust struct RunJournalWriter，每次 SSE put_event 时 append；stale_interrupted_event 逻辑在 WS 广播时注入 | 落在 transcript.rs（现有 JSONL tail）和 WS broadcaster。RunJournalWriter per stream_id 写 _run_journal/{session_id}/{stream_id}.jsonl；frontend poll /api/run-events?after_seq=N 做增量拉取，解决现有 tail 全量推送的带宽浪费 |
| P0 | Turn journal（用户意图边界 JSONL + flock + 目录 fsync） | swarmx 无 turn 级 crash recovery。Turn journal 记录 submitted/completed/interrupted 事件，重启后可知哪些 turn 提交了但未完成，配合 session_recovery 可重建 pending turn 状态 | 低：turn journal 是纯文件 append，Rust 实现 flock + fsync 很直接；关键是在 PTY spawn 前写 submitted，turn 完成后写 completed | 落在 worker spawn 路径（pty.rs 或 agent.rs）。per-session _turn_journal/{sid}~{pid}.jsonl，目录 fsync 保证可见性。配合现有 WakeCoordinator 可在 worker hang/crash 后发现 pending turn 并重派 |
| P0 | Stale worker 检测 + synthetic error 事件注入 | swarmx 有 .error fallback（agent kill 后写 <signal>.error）但 hang 场景无兜底（已知盲区）。run_journal 的 stale_interrupted_event：定期检查 active stream 是否有 journal 但 worker 进程已消失，若是则合成 synthetic terminal event 推送给前端展示恢复提示 | 中等：需要 run_journal（P0 已做）+ 定期检查任务（已有 WakeCoordinator 基础）+ synthetic event 路径 | 落在 WakeCoordinator 或新 HealthMonitor 任务。检查 ACTIVE_RUNS 中所有 non-terminal stream，若 PTY 进程已退出但 journal 无 terminal 事件，向对应 WS channel 推 synthetic apperror，触发前端「worker 已断」提示 |
| P0 | Worker 健康三态（alive/down/unknown） | PTY worker 的健康状态不是二值的：agent 可能 hang（进程存在但无输出），可能已退出（error fallback），也可能根本没配置。当前 swarmx 缺少 hang 检测。引入 alive=None 语义后 UI 可区分「卡死」和「退出」，触发不同的 UX（静默重试 vs 报错） | 中：在 agent_registry 中加 last_activity_at 心跳 + 两个阈值（warn_s/dead_s），WS 广播时带 health 字段 | src/agent_registry.rs + WS 广播层：加 health: Alive\|Stale\|Unknown 枚举，蜂群成员栏用颜色区分三态 |
| P0 | Kanban 看板：任务台账从只读黑板升级为可写 CRUD 控制面板 | swarmx 黑板目前是 agent 间共享区，缺少 UI 创建/编辑任务、拖拽列、手动 block/unblock 的能力；hermes kanban 模型完整（6列状态机、依赖链、comment、bulk 操作、multi-board） | 高：需要定义 kanban_task 表（SQLite）、状态机、REST + WS 事件推送、前端看板列UI | 可在现有 SQLite(agents.db) 加 kanban_tasks/task_events 表；状态转换 done/blocked/ready 映射到 swarmx orchestrator 派发逻辑；WS 广播替代 SSE（swarmx 已有 WS 基础设施）；typed handoff key 可作为 task 的 workspace_kind |
| P1 | inflight state 持久化（浏览器 reload 恢复） | swarmx 当前刷新页面会丢失所有 live tool cards 和 activity group；hermes 的 saveInflightState 把最近 N 条消息+toolCalls 紧凑化存 localStorage，reload 后 restoreLiveTurnHtmlForSession 恢复 DOM | 前端：移植 _compactInflightState + _writeInflightStateMap（QuotaExceeded 降级）+ restoreLiveTurnHtmlForSession；与 swarmx 现有 WS 重连 + AgentActivity tail 逻辑兼容 | swarmx 前端 chat.js/聊天模块；WS 重连时先 restore snapshot，再用实时 AgentActivity 事件增量更新 |
| P1 | renderMd stash-token pipeline 的 diff/csv/json/yaml 特判 | swarmx 当前 ChatMarkdown 做了 markdown 渲染，但 diff 块无语法着色、json/yaml 无 tree view、csv 无表格，影响 worker 输出可读性 | 前端：从 hermes renderMd 剪出 diff/csv/json/yaml/mermaid 的 stash 分支（约 100 行），集成进现有 ChatMarkdown 渲染；_buildTreeDOM 树视图约 50 行独立函数，可直接搬 | ChatMarkdown 组件（/src/components/ChatMarkdown.tsx 或等效）；json/yaml tree view 对 worker 输出的结构化数据（blackboard JSON、harness-check 结果）尤其有价值 |
| P1 | Kanban 任务面板（仿 hermes Kanban） | swarmx 当前无 kanban，需要在多 direction/worker 场景下可视化任务状态；hermes kanban 有 SSE 实时更新 + SSE→polling fallback + 拖拽换列 + 多 board + 任务详情（events/runs/comments/links） | 后端：Rust kanban_db 模块（SQLite），/api/kanban/board + /api/kanban/events/stream；worker 完成时写入 task 状态；前端：panels.js kanban 区块约 1800 行，可分批移植 | 新增 Kanban 页面；task 对应 swarmx 的 blackboard typed handoff key；status 列 = pending/in_progress/completed 三态；SSE stream 复用 swarmx 现有 WS broadcast 机制 |
| P1 | 消息渲染两级缓存（per-message renderMd cache + per-session innerHTML cache） | swarmx 长会话（orchestrator 派多个 worker）消息多，每次 WS 事件都全量 re-render 会卡帧；hermes 的双缓存策略可大幅降低重渲成本 | 前端：_renderCache（Map，上限 300 条，key=role+长度+prefix+suffix）+ _sessionHtmlCache（Map，key=sessionId，guard streaming 时跳过）；约 60 行，逻辑简单 | 聊天渲染模块；streaming 时跳过 session cache 的 guard 尤其重要（swarmx 有实时 AgentActivity 注入 DOM） |
| P1 | 两阶段懒加载（metadata=0 → messages=1 分离） | 当前 swarmx loadSession 可能一次拉所有消息，长会话切换时有明显等待。hermes 把元数据（<1KB）和消息体拆开，Phase1 立刻渲染 topbar/模型标签，Phase2 后台拉消息体，视觉上切换瞬间完成。 | 中（需要后端 Rust API 支持 messages=0/1 参数，前端分两次 fetch） | routes.rs + chat handler，GET /api/agent 加 messages=0 快捷路径返回纯 meta；前端 loadSession 拆成两阶段。 |
| P1 | composer 草稿跨会话持久化到服务端 | swarmx 用户切换方向时，当前 composer 内容会丢失。hermes 在切换前 await _saveComposerDraftNow() 写服务器，恢复时从 session.composer_draft 还原。支持多客户端共享和页面刷新恢复。 | 中（Rust 需加 /api/session/draft POST 端点，db 加字段；前端切换前 save + 加载后 restore） | storage.rs 加 composer_draft 字段，routes.rs 加 draft 端点；前端 loadSession / session-switch hook。 |
| P1 | 多维未读状态机（viewed-count + completion-unread 双轨） | swarmx 当前未读通知标记机制依赖 WS 广播 + 轮询，后台完成会话的未读圆点可能延迟出现。hermes 用 completion-unread（流结束时打标）+ viewed-count（已读时清标）双轨，localStorage 持久化，不依赖服务端，立即响应。 | 低（纯前端 localStorage，无需 Rust 改动） | swarmx 成员栏的未读标记逻辑（notification-store.ts 或 swarm-ui.js），把现有「消息计数对比」改造为双轨方案。 |
| P1 | smd streaming-markdown 增量解析 + rAF 节流渲染 | swarmx 当前聊天区的流式输出用 innerHTML = renderMd(全文) 方式，每个 token 触发全量替换，长回复时频繁 reflow。hermes 用 smd.min.js parser_write 增量构建 DOM + 66ms rAF 节流，性能显著更好。 | 中（引入 smd.min.js，替换流式渲染路径；PTY worker 的流式输出已有 JSONL tail，差接收端） | chat.js 的 appendToken 路径，引入 smd parser 替换现有全量 innerHTML；swarmx 的流式来源是 transcript.rs 的 JSONL tail，已有 WS 广播 AgentActivity，直接对接。 |
| P1 | Steer 中途注入（不中断 turn）+ queue/interrupt/steer 三模式策略 | swarmx worker 在 PTY 内运行，目前 busy 时只能 kill 或等待。hermes 的 /steer 通过 /api/chat/steer 注入 agent 内部 tool-result，实现不中断当前 turn 的方向修正。swarmx 可以在 PTY 层实现类似能力：向 worker PTY 注入换行符或特定标记文本（类似 WakeCoordinator 的 PTY inject）作为 steer hint，fallback 到 cancel+requeue（类似 cmdInterrupt）。 | 高：需在 Rust PTY 层设计 steer 注入协议；claude CLI 的 /steer 可能需要通过 --message 或 PTY input 实现 | 落在 agent_manager.rs + PTY 层；steer hint 可参照 WakeCoordinator 的 PTY inject `\x15…\r` 机制；前端 _showSteerIndicator 的临时徽章（不进 messages）可直接移植 |
| P1 | 工作区面板三态状态机 (closed\|browse\|preview) + bfcache pageshow 恢复 | swarmx 工作区面板目前是简单的 open/close 布尔；hermes 三态状态机明确区分了「用户主动浏览」和「文件预览触发打开」，防止预览关闭时面板错误收起。bfcache pageshow 处理解决了浏览器后退缓存场景下 SSE 失效 + 搜索残留问题，swarmx 也会遇到相同问题。 | 低：前端状态机逻辑；bfcache listener 约 30 行 | 落在 swarmx 前端 workspace panel 状态管理；Rust 后端无需改动；pageshow SSE 重连对 swarmx 的 WS 连接也适用 |
| P1 | Artifact 自动收集（工具调用路径提取 + turn 级变更追踪 + 预览自动刷新） | swarmx 已有 worker 实时活动（读 JSONL），但没有「本次对话修改了哪些文件」的 Artifacts 视图。hermes 从 tool_calls 参数中提取文件路径（支持 OpenAI/Anthropic 双格式），渲染为可点击的 Artifacts tab。turn 结束后自动 bustCache 刷新已开预览。swarmx 的 worker 工具调用通过 transcript.rs 可拿到，适合做类似追踪。 | 中：前端 _artifactCandidatesFromToolCall 逻辑可直接参照（纯 JS）；Rust 后端需在 transcript 解析时记录 mutation tool 参数 | 落在 transcript.rs（提取 tool_call paths）+ 前端 workspace panel（Artifacts tab）；ARTIFACT_MUTATION_TOOLS 集合需扩展 claude/codex 实际使用的工具名 |
| P1 | API 封装：相对路径 + timeout + 3次 TypeError 重试 + 401 自动跳登录 | swarmx 前端 fetch 封装目前不统一（有些直接 fetch、有些通过 api helper）。hermes 的 api() 函数处理了：subpath 部署的 baseURI 拼接、30s timeout + AbortController 级联、只重试 network error（不重试 HTTP 错误）、401 重定向、JSON error body 解析。这些是生产环境必备的健壮性。 | 低：直接搬运 workspace.js api() 函数（~90 行），适配 swarmx 的 auth 机制 | 落在 swarmx 前端公共 utils；timeout 和 upstream signal 合一的 AbortController 链接设计对长 SSE 连接尤其有价值 |
| P1 | 内嵌 PTY 终端面板（workspace 感知） | 用户在 swarmx 聊天/蜂群页调试 agent 输出时需要随时能开一个 shell 在 workspace cwd，现在必须切到系统终端。内嵌终端可大幅减少上下文切换，尤其是查看 agent 修改的文件、运行测试、手动 git 操作。 | 中：后端 Rust 侧用 pty crate（portable-pty/nix::pty::openpty）+ 独立线程 reader，API 层加 /api/terminal/start\|input\|output(SSE)\|resize\|close；前端 xterm.js + 三态状态机可直接参照 hermes 实现。注意 swarmx 是 Rust 不是 Python，spawn supervisor 思路相同但用 tokio::task::spawn_blocking 包裹 Popen 即可。env allowlist 同样需要，swarmx server 持有多个 CLI 的 API token。 | 落点：routes/terminal.rs 新模块 + src/terminal.rs 核心逻辑；前端 chat 页 composer toolbar 增加'终端'toggle 按钮，面板嵌在 composer 下方，复用 hermes 的三态 CSS 方案；workspace 感知（per-workspace cwd）天然契合 swarmx 多 workspace 设计。 |
| P1 | PWA 安装支持（manifest.json + Service Worker shell 缓存） | swarmx 目前是纯 Web 应用，没有 PWA 能力。加上后可安装到 macOS Dock / 手机桌面，脱离浏览器标签页，体验接近原生桌面应用（不需要 Tauri 路径也能有 standalone 窗口）。shortcuts 支持'新建 workspace'等快捷入口。 | 低：静态文件 manifest.json + sw.js（缓存策略参照 hermes，注意 /api/* 不缓存、/login 不缓存、navigate network-first）；pwa-startup.js 的早期检测逻辑可几乎原样移植；Axum 侧加 /sw.js 路由（注入版本号）和 /manifest.json 路由。 | static/ 目录加 manifest.json/sw.js/pwa-startup.js；routes/static_files.rs 增加这两个路由；Tauri 壳和 PWA 互不干扰（Tauri 不注册 SW）。 |
| P1 | SW 缓存版本号注入（git commit hash 自动 bump） | hermes 在 /sw.js 路由处动态替换 __WEBUI_VERSION__ 为 git hash，静态资源 URL 带 ?v=<hash>，无需手动维护版本号。swarmx 每次 cargo build 更新 binary，SW 缓存自动失效，避免用户看到 stale UI。 | 低：build.rs 里读 git rev-parse HEAD 写入 CARGO_PKG_VERSION 或自定义 env，routes 层替换 sw.js 模板字符串 | build.rs + routes/static_files.rs（/sw.js 动态 handler）；静态 JS/CSS <script>/<link> src 加 ?v={GIT_HASH}。 |
| P1 | 首次启动引导向导（wizard，检查 CLI 工具 + API key 配置） | 新用户 clone swarmx 后需要手动配置 claude/codex CLI 路径、API key、默认 workspace；没有引导容易卡在'为什么 agent spawn 失败'。hermes 的 system check（hermes_found/imports_ok）+ provider setup + workspace 选择 + 密码设置五步向导是很好的首次体验起点。 | 中：后端 /api/onboarding/status（检查 claude/codex binary 是否在 PATH、env var 是否配置）+ /api/onboarding/setup + /api/onboarding/complete；前端五步向导 DOM 模板；probe 逻辑对 swarmx 不适用（无 base_url provider）可简化 | routes/onboarding.rs 新模块；前端引导 overlay 在 index.html；检查点：claude --version/codex --version 可执行 + ANTHROPIC_API_KEY 环境变量存在 + 至少一个 workspace 目录。 |
| P1 | 双轴主题系统（theme x skin 正交）：轻量 accent 皮肤只覆盖 5 个 CSS 变量 | swarmx 现在 dark/light 已有，但没有 accent 皮肤层。hermes 的设计证明：加皮肤等于只加一段 CSS，零 JS 改动，完全向后兼容。开发者用户欣赏能改颜色的工具 | 低：swarmx style.css 已有 --accent 族变量；只需添加 [data-skin=X] 选择器，settings 里加皮肤 picker，boot 时读 localStorage 写 html dataset.skin | style.css 新增皮肤 CSS block；前端 settings/appearance 面板加皮肤 grid picker（参照 hermes _buildSkinPicker）；Rust boot 不需改 |
| P1 | 纯 CSS data-tooltip 自定义 tooltip（替代原生 title 属性） | swarmx 目前用原生 title，有 1.5s 延迟。hermes 的 .has-tooltip::after + opacity transition + 150ms delay 方案零 JS、低开销，VS Code/Linear 都用这个套路。对程序员用户来说快速 tooltip 是基本体验 | 低：在 swarmx style.css 加 .has-tooltip 相关规则（约 20 行 CSS）；把现有 title= 换成 data-tooltip= + class=has-tooltip；三个方向变体（右/下/左）覆盖所有场景 | style.css 新增 tooltip CSS；更新 Rust 侧 HTML 模板和前端组件中的 button 属性 |
| P1 | Context window 环形进度指示器（SVG arc + 百分比 + compress 按钮） | swarmx 已有 context 相关功能但 UI 无可视化。hermes 的 ctx-ring（SVG stroke-dasharray 动画 + 中心数字）在 composer 角落直观展示 context 占用；填满时出现 compress 按钮。对 swarmx 的 orchestrator 场景，context 管理尤为重要 | 中：SVG 环形进度 CSS + JS（约 50 行）；后端 GET /api/session/{id}/context 返回 token 计数（claude/codex 会话 JSONL 里有 usage）；compress 按钮触发现有压缩流程 | swarmx 聊天页 composer 右侧；后端 transcript.rs 已读 JSONL usage 字段，补一个 REST 端点暴露即可 |
| P1 | Reconnect banner + Offline monitor（断线自动探测 + soft reload） | swarmx 断线后用户体验不明确。hermes 的 initOfflineMonitor 监听 navigator.onLine 变化 + 定时 /health probe，恢复时 soft reload 不全页刷新，断线时 banner 提示。本地工具网络更稳但 WS 断线场景（server restart）同样存在 | 低：ui.js/boot.js 加 30 行 offline monitor；CSS 加 offline-banner 样式；Rust 侧已有 /health 端点 | swarmx frontend boot.js + style.css；WS 断线重连逻辑可与此联动 |
| P1 | UI 活文档页（docs/ui-ux/index.html 模式）：复用真实 CSS 展示所有 UI 状态 | hermes 用一个 HTML 文件加载真实 style.css，穷举所有 message/component 状态，开发时 visual regression 一眼可见。swarmx 迭代很快，这能防止新 CSS 改动破坏已有状态，配合 harness-check 形成 UI 层断言 | 低：新建 docs/ui-inventory.html，引用 swarmx style.css，手写各组件 HTML 骨架（不需要 JS）；可加 theme/skin 切换按钮 | docs/ 目录下新文件；无需 Rust 改动；CI 里可用 playwright 截图对比做 golden test |
| P1 | Coalescing bounded-queue 事件总线替代简单 WS broadcast | swarmx 当前 WS 广播是 fan-out 到所有连接。如果 tab 很多或事件频繁（如 AgentActivity 高频），会浪费带宽。session_events 的 maxsize=1 + coalesce 模式：多次 sessions_changed 在 tab 还未消费时自动合并，保证最终一致性同时减少推送量 | 低：在 WS session list 变更通知路径上加 coalesce 逻辑；per-workspace 作用域类比 per-profile 作用域 | 落在 ws_broadcaster.rs 或 routes/sessions.rs。新增 version 字段（单调递增），前端携带 last_version 做 ETag-style 去重 |
| P1 | 四源交叉 session discoverability 审计（audit + repair） | swarmx 有 DB（SQLite）+ 方向（git worktree）+ agents 内存状态三个数据源，可能出现类似的不一致（agent 在 DB 但 worktree 已删）。discoverability 审计模式：定期或 on-demand 交叉检查，输出 repairable/unsafe 分类，可加 CLI 命令 swarmx audit 供用户检查 | 中等：读 SQLite + 检查 worktree 目录 + 比对 agents 内存状态；分类逻辑参考 audit_session_discoverability | 新 CLI 子命令 swarmx audit 或 /api/audit endpoint。重点检查：DB 有 agent 但进程已死、worktree 已删但 DB 有 direction、blackboard key 孤儿 |
| P1 | Source normalization 稳定契约（normalize_agent_session_source 模式） | swarmx 有 claude/codex 两种 CLI，未来可能增加第三种。agent source 类型散布在 DB 和前端代码中。规范化到 {cli_type: 'claude'\|'codex', session_source: 'worker'\|'orchestrator'} 契约，后续路由/UI 只对枚举判断，不做字符串 match | 低：在 models_config.rs 或新 agent_meta.rs 加 normalize 函数，已有 CLI enum 的基础上扩展 | 落在 agent.rs / models_config.rs。CLI 类型已有 enum，加 session_source（worker/orchestrator/merger）维度，给 WS 消息和 API response 统一附带规范化 meta |
| P1 | Explicit profile/workspace threading（state_sync 防 TLS 泄漏模式） | swarmx worker 是多线程（PTY + WakeCoordinator + WS），不同 agent 有不同 workspace。如果用 thread-local 存 workspace/session 上下文，worker 线程中可能读到错误 context 并写入错误 DB row。显式传参模式：所有修改 session 状态的函数签名必须显式带 agent_id/workspace，不依赖 thread-local | 低：Rust 无 TLS 惯用法问题（Rust 传参是主流），主要是 code review 层面的约束规则 | harness-check 规则：扫描 session 写入路径确保都显式携带 agent_id；新 MEMORY.md 条目记录此约束 |
| P1 | session_status / session_usage slash command 对标（/status + /usage 语义） | swarmx 聊天里无 /status /usage 命令。session_ops 展示了一个清晰接口：/status 返回 token 计数+cost+agent_running；/usage 返回用量摘要。这是 Usage/Cost 可观测的 CLI slash command 层 | 低：已有 session DB token 字段，加 slash command handler 调 /api/session/:id/status 或直接读 DB | 落在 chat slash command 处理（前端 ComposerInput + 后端 routes/session.rs）。已有的 /api/session 路由扩展 status/usage 子路由 |
| P1 | RuntimeAdapter Protocol / 缝合点设计 | swarmx 当前 streaming 逻辑和 PTY spawn 逻辑耦合在一起，难以替换后端。引入类似的 Trait 缝合点（start_run/observe_run/cancel_run/respond_approval）可以在不改浏览器协议的情况下支持：PTY-legacy / runner-sidecar / 未来 native-API 三条路 | 大：需定义 Rust trait + 至少两个实现（PTY 直通、未来 runner），但可分阶段—先定义 trait 不改路由 | src/runtime_adapter.rs（新文件）：定义 RuntimeAdapter trait，PtyRuntimeAdapter 实现，routes/chat.rs 通过 Arc<dyn RuntimeAdapter> 调用 |
| P1 | 审批/澄清 FIFO 队列 + SSE 推送 | swarmx 已有 PTY inject 唤醒机制，但缺少结构化的用户介入点（工具调用审批、澄清问题）。hermes 的 FIFO + threading.Event 阻塞 + SSE 推送模式可直接映射到 Rust：tokio::sync::oneshot（agent 阻塞等）+ broadcast channel（SSE 订阅），支持并发多个 pending approval | 中：Rust 侧 per-agent pending_approvals: VecDeque<Approval>，路由 POST /api/agents/:id/approvals/:approval_id/respond，WS 推送 | src/approvals.rs（新）+ routes/approvals.rs：与现有 PTY inject 层对接，用 oneshot::Sender 阻塞 worker task，用 WS broadcast 推给前端审批 UI |
| P1 | Request watchdog：阶段计时 + 超时全线程栈 | swarmx 缺少对慢请求的可观测性。request_diagnostics 的 stage()/finish() + Timer 快照全线程栈 可以非侵入性地挂在 axum middleware 层，对延迟敏感路径（spawn、chat/start）自动打 warning log | 小：axum middleware 在 request_id 层包装，超时用 tokio::time::sleep + 打 WARN log，Rust 侧栈抓取用 std::backtrace | src/middleware/slow_request.rs（新）：axum middleware，stage 通过 request extensions 传递，超时触发 tracing::warn! 带 stages JSON |
| P1 | 压缩锚点标记识别 + visible_messages_for_anchor | swarmx 读 JSONL tail 做实时进度，但 context compaction 消息会混入活动展示。引入 is_context_compression_marker 过滤逻辑，在 transcript.rs tail 处过滤合成 compaction 消息，避免 drawer 活动 tab 把压缩标记显示为「工具调用」 | 极小：在 transcript.rs 的消息过滤处加 is_compression_marker() 函数，匹配 [context compaction 前缀 | src/transcript.rs：filter_compression_markers()，AgentActivity 广播前先过滤，同时 compression 消息可独立推 CompressionEvent 类型供 UI 展示进度 |
| P1 | HTTP client 安全基线：禁用重定向 + scheme 白名单 | swarmx 目前调用外部 runner/MCP server 可能走 reqwest 默认配置（允许重定向）。对 operator 配置的内部服务 URL 应拒绝重定向（防 token 泄露到 Location）并只允许 http/https scheme | 极小：reqwest::ClientBuilder::redirect(Policy::none()) + URL scheme 检查，加在 runner_client.rs 或 mcp_admin 的 HTTP 调用点 | src/http_client.rs（工具函数）：build_internal_client() 返回禁重定向 + scheme 校验的 reqwest::Client，所有内部 HTTP 调用复用 |
| P1 | SSE/WS cursor + Last-Event-ID 断线续传模式 | swarmx 现有 WS 广播无法在重连时从断点续传，导致客户端重连后看不到中间事件 | 中：WS 层加 event_id 序列号，客户端重连时发 last_seen_id，服务端补发 replay | 在 swarmx Axum WS handler 中维护 event_log Vec（内存或 SQLite）；客户端 WS 握手 header 带 Last-Event-ID；现有 WsMessage 加 seq 字段 |
| P1 | TPS 实时计量 + HIGH/LOW 60分钟滚动窗口 | swarmx 已有 worker 活动 JSONL tail，但没有 token throughput 可视化；用户无法感知 claude/codex worker 的输出速度异常 | 低：读 JSONL 中 output_tokens 字段，per-worker 计算 TPS，通过 WS 推送 metering 事件 | transcript.rs tail 已有 token 字段；新增 MeterState per agent_id；AgentActivity SSE 可扩展 tps 字段；前端成员栏活动行加 TPS chip |
| P1 | Workspace 路径信任模型：三重检查（home / saved list / boot default）+ 系统目录黑名单 | swarmx 允许任意 workspace 路径，存在路径遍历风险；hermes 有完整的分层信任+系统目录黑名单 | 中：Rust 实现 is_trusted_workspace() + block list，在 spawn/file API 入口校验 | workspace.rs 新增 resolve_trusted_workspace()；spawn 参数 cwd 必须通过信任检查；文件 API（/api/file）做同样校验 |
| P1 | TOCTOU 防御：文件读写用 anchored fd（openat + O_NOFOLLOW） | swarmx /api/file 端点在 resolve 后重新按路径打开，存在 symlink 竞态窗口 | 中：Rust nix/libc openat + O_NOFOLLOW 组件逐段打开 | workspace.rs safe_open_file()；现有 GET /api/file 和 PUT /api/file 替换 std::fs::read/write |
| P1 | Git 操作套件：env scrub + repo-root 互斥锁 + 结构化错误码 | swarmx 现有 git 操作（list_workspaces 分支计算）无环境清洗、无并发保护、错误直接字符串 | 中：Command::env_clear + insert_required + Mutex<HashMap<repo_root, Lock>>；错误 enum 对应前端 i18n key | workspace_git.rs 新模块；现有 sidebar branch-forward 计算迁移过来；git_stage/commit/push 支持方向合并 UI |
| P1 | stash-and-checkout + 按分支 stash 前缀自动恢复 | swarmx 多方向切换时担心脏工作区阻塞切换；hermes 模式：stash保存→切换→自动pop | 中：git stash push -m 'swarmx direction switch to X' + 切换后 stash list + pop 匹配 | directions 切换（sidebar_branch_forward）集成；前端「切换方向」按钮触发 POST /api/workspace/checkout?dirty_mode=stash |
| P1 | Worktree 删除多重守卫：active stream / active terminal / dirty / unpushed commits | swarmx 方向 worktree 删除时只有基本 dirty 检查，缺 stream 锁、unpushed 检查 | 低：worktree status 增加 locked_by_agent（有 in-progress 任务）、ahead 检查；force=true bypass | ExitWorktree handler；directions API DELETE /api/directions/:id 前调用 worktree_status |
| P1 | 启动健壮化：端口双实例检测 + FD limit 提升 + 进程退出 audit log | swarmx 目前启动检查简单，双实例会破坏 SQLite in-memory 状态（已有文档说明）。参考 hermes 的 _abort_if_already_serving + _raise_fd_soft_limit + _log_shutdown_audit，Rust 对应是 std::net::TcpListener::bind 失败时给出清晰 '已有实例在运行' 错误，signal::ctrl_c 时记录 active agents 数 | 低，几十行 Rust | src/main.rs 或 server/mod.rs 的启动序列，在 bind 之前 probe /health 或 /api/health，失败则 eprintln 并 process::exit(1) |
| P1 | PBKDF2 + HMAC 签名 cookie 鉴权（零外部依赖） | swarmx 无鉴权是已知短板。hermes 的方案：PBKDF2-600k hash 进程内缓存 + 签名 cookie + CSRF = HMAC(key, 'csrf:'+token) 无需存储 + IP 速率限制 + Passkey 默认 off。Rust 可用 ring/hmac + pbkdf2 crate，整套 stdlib 级别实现 | 中，3-4天 | 新建 src/auth/ 模块：password_hash.rs(PBKDF2) + session.rs(签名cookie) + csrf.rs(派生token) + rate_limit.rs(IP计数)；axum middleware 注入；settings.json 存 password_hash |
| P1 | Profile-scoped 配置：worker thread 显式传 profile_home 而非依赖 thread-local | swarmx 的多 CLI + 多方向场景比 hermes 复杂：orchestrator 和 worker 在不同 PTY，配置必须 snapshot 到 spawn 时。参考 hermes get_config_for_profile_home()：把 session 的 profile 路径在 spawn 时序列化进 worker 的 system_prompt/env，而非依赖全局状态 | 低，设计决策已有雏形 | spawn_worker 时把 agent_config 快照（model tier、MCP config 路径、hermes home 等）序列化到 AgentMeta，worker 启动时从 AgentMeta 而非进程全局读配置 |
| P1 | 原子文件写入模式推广到所有持久化文件 | swarmx 的 settings.json/blackboard 写入是否原子？hermes 每个文件都用 tempfile + os.replace + chmod。Rust 的 tempfile crate 提供完全一致的 atomicwrite 语义 | 低，一个 utility 函数 | src/storage/atomic.rs: pub fn atomic_write(path: &Path, content: &[u8]) -> Result<()> { 用 tempfile::NamedTempFile + persist() }；推广到 settings.rs / blackboard.rs |
| P2 | SSE→polling fallback 模式（失败 N 次自动降级） | swarmx 目前用 WS，部分代理环境下 WS 也会被剥离；hermes 的 SSE 配 fallback 提供了更好的网络兼容性，且 SSE 在 HTTP/2 下可多路复用 | 后端：/api/events/stream SSE 端点（已有 WS，改为支持双协议）；前端：_kanbanStartEventStream 模式，_kanbanEventSourceFailures 计数器，超 3 次切换 setInterval polling | swarmx WS 广播层（broadcast.rs）扩展 SSE 端点；前端 EventSource 客户端与现有 WS 并存，先尝试 WS，fallback SSE，再 polling |
| P2 | data-* 属性作为 DOM 序列化边界（跨 innerHTML 存活的分类标志） | swarmx 聊天中工具卡片在 session 切换时 snapshotLiveTurnHtml → restore，JS 属性会丢失；hermes 用 data-memory-save/data-skill-update 镜像到 data-* 使分类存活 | 前端：在 buildToolCard 中凡需跨 snapshot 持久的状态，mirror 到 data-* attribute；约 5 行改动，但思维模式需全员知晓 | swarmx 聊天工具卡片渲染；AgentActivity 事件 id 去重逻辑也应用 data-activity-event-id 而非 JS WeakMap |
| P2 | inert markup + 后绑定事件（防插件/动态内容 XSS） | swarmx MCP 管理页 / 角色注册页中如果用 innerHTML 插入动态 key/path，存在注入风险；hermes 的分离绑定模式是正确解法 | 前端：审查所有 innerHTML 中含 onclick 的地方，把动态值改为 querySelector + addEventListener 后绑定；约 2-3 处（MCP admin、role 列表） | swarmx 前端 MCP 管理页（mcp_admin.rs 对应的前端部分）和任意动态 key 插入的 onclick |
| P2 | Tab 可见性 + 拖拽排序（仿 hermes Settings → Appearance） | swarmx 侧边栏面板固定，用户无法隐藏不常用页面（如 MCP/黑板）；hermes 的 chip 拖排序 + localStorage 持久化体验好 | 前端：移植 _getHiddenTabs/_setHiddenTabs/_getTabOrder/_applyTabOrder/_renderTabVisibilityChips/_wireTabChipDrag 约 200 行；无后端改动 | swarmx 左侧导航（rail/sidebar）；ALWAYS_VISIBLE_TABS 白名单保护蜂群/聊天不被隐藏 |
| P2 | Provider Quota 可视化（exhausted/available/retry-after） | swarmx 当前 model 配给页无 quota 状态；当 codex 503/claude 限流时用户无法感知配额状态；hermes 的 quota card 有 pool breakdown + retry-after 时间 | 后端：/api/provider/quota 端点读取 Claude/Codex 的速率限制响应头并聚合；前端：_buildProviderQuotaCard 约 120 行 | swarmx 设置→模型配给页（models_config.rs 对应的前端）；codex 503 诊断 playbook 中提到的空跑问题可通过 quota 可视化提前发现 |
| P2 | SSE + HTTP polling 双通道降级架构（approval/clarify/sessions） | swarmx 的 approval / clarify 等交互当前用 WS 实现，在代理或网络不稳定时可能断连。hermes 的 SSE+fallback 模式（SSE 失败自动切 HTTP 轮询，指数退避重连）更健壮。 | 高（需要 Rust 端加 SSE 路由，前端替换 WS 监听逻辑；但可分模块逐步替换） | routes.rs 加 /api/approval/stream、/api/sessions/events SSE 端点；前端 approval/clarify 模块用 EventSource 替换轮询。 |
| P2 | stream fade 渐显动画（EWMA 词速 + 词级 span 淡入） | swarmx 当前流式输出 token 是直接追加，快速模型下会出现整段文字突现的廉价感。hermes 的 stream fade 用 EWMA 估算词速、每帧揭示 2-3 词，每词 <span class='stream-fade-word'> + CSS animation，视觉上更流畅专业。 | 中（纯前端，需配合 smd renderer 包装，加 CSS 动画定义；不需要 Rust 改动） | chat.js 的 smd renderer wrapper，在 add_text 中按词分 span；style.css 加 @keyframes stream-fade-word；开关由配置 window._fadeTextEffect 控制。 |
| P2 | 会话列表 FLIP 动画（位移平滑过渡） | swarmx 方向列表更新时行跳变，新方向突现。hermes 的 FLIP 动画在数据更新前快照 top，更新后计算 delta，用 CSS transform 驱动平滑位移，用户感知「滑动」而非「闪现」。 | 低（纯前端，capturePositions + playReflow 两个函数，200 行左右） | swarm-ui.js 的 renderDirectionList / renderAgentList，在 innerHTML 更新前后调用 FLIP 工具函数。 |
| P2 | 「Reply with selection」：选中消息内容快速引用到 composer | swarmx 用户无法对已有消息片段进行引用回复。hermes 检测 selectionchange 事件，在聊天区选中文字时弹出浮动按钮，点击将选中文本格式化为 markdown > 引用插入 composer，高效且无 UI 常驻开销。 | 低（纯前端，约 80 行，无需 Rust 改动） | chat.js 注册 selectionchange + mouseup 事件，动态创建 #selectedTextReplyBtn 按钮，检查 selection 是否在 #msgInner 内。 |
| P2 | 「手动状态」（todo/in-progress/done）循环切换 | swarmx 方向/任务没有用户可控的完成状态标记。hermes 在 session 行提供三态手动状态（localStorage 存储），配合上下文菜单一键切换，适合多会话并行时做任务管理。 | 低（纯前端，localStorage + 渲染 badge；无需 Rust 改动） | swarmx 方向列表每行加 todo/in-progress/done 状态图标，数据写 localStorage[directionId]；与现有 active/completed 语义做映射。 |
| P2 | Markdown 文件预览大文件降级 + 强制渲染按钮 + 缓存路径 guard | swarmx 聊天中已有 markdown 渲染（ChatMarkdown），但文件预览没有大文件保护。hermes 的 256KB/5000行 阈值降级 + 强制渲染按钮 + _previewRawContentPath 缓存 guard（防 #3378 用旧缓存渲染错文件）是经过生产验证的策略。 | 低：前端 shouldRenderMarkdownPreviewAsPlainText + setLargeMarkdownForceRenderVisible + _previewRawContentPath guard 约 30 行 | 落在 swarmx 工作区文件预览模块；Rust /api/file 端点无需改动 |
| P2 | git 状态徽章（branch/dirty/ahead/behind）在文件树头部实时显示 | swarmx 有 git merge 功能但没有工作区层面的 git 状态可视化。hermes 的 _refreshGitBadge 在 loadDir 根目录时非阻塞拉取 /api/git-info，显示 branch·N△↓M↑K，对 swarmx 多方向/worktree 用户非常有用。 | 低：Rust 后端 /api/git-info 用 git2 crate 读 repo 状态（已有类似能力在 list_workspaces）；前端 _refreshGitBadge 约 30 行 | 落在 workspace panel 文件树头部；可复用 sidebar 分支计算逻辑（Sidebar branch-forward redesign 已做） |
| P2 | 工作区展开目录状态按 workspace path 分别持久化 | swarmx 文件树没有展开状态持久化。hermes 用 localStorage key = 'hermes-webui-expanded:' + workspace_path 为每个工作区独立保存展开状态，切换工作区时自动恢复。配合 loadDir 的 Promise.all 并行预取展开目录，体验流畅。 | 低：纯前端 localStorage 约 20 行 + loadDir 预取逻辑 | 落在 swarmx 前端文件树组件；无需 Rust 后端改动 |
| P2 | spawn supervisor 线程（单线程串行 Popen） | 虽然 tokio 是异步，但 swarmx worker spawn 也可能在请求上下文中调 std::process::Command，同样有 PR_SET_PDEATHSIG per-thread 问题（如果未来用到）。更重要的是 supervisor 模式提供了 5s spawn 超时 + 超时后自动 SIGHUP/SIGKILL 清理孤儿的结构，对 PTY shell spawn 很有用。 | 低（如果做终端 P1 时顺带）：一个 mpsc channel + tokio::task::spawn_blocking 串行处理 spawn 请求 | src/terminal.rs，仅影响终端模块，不影响现有 agent PTY 驱动（agent PTY 已有自己的生命周期管理）。 |
| P2 | 登录页连通性探测（区分 VPN 断开与未授权） | swarmx 目前无鉴权，但 MEMORY 里有 bypass-permissions 和 auto-trust 设计，未来可能加密码。hermes 的 /health 轮询 + 'VPN 断了请检查连接' + auto-reload 是非常实用的 UX 细节，开发者远程使用时必备。 | 极低：纯前端 IIFE，复制 checkConnectivity 代码，指向 swarmx 的 /health endpoint | static/login.js（若 swarmx 加登录页时）；/health 路由 swarmx 已有。 |
| P2 | WebAuthn Passkey 支持（无密码登录） | 如果 swarmx 加认证，Passkey 比密码安全（phishing-resistant）且对开发者友好（Touch ID/Face ID/硬件 key）。hermes 的零依赖 CBOR parser + ES256 实现可在 Rust 侧用 webauthn-rs crate 替代，前端代码可原样复用。 | 中：Rust 侧用 webauthn-rs crate 处理注册/验证；challenge 存 SQLite（swarmx 已有 DB）；前端 b64u 转换 + navigator.credentials.get 逻辑可直接搬 | routes/auth.rs + src/passkeys.rs；challenge 表加到 SQLite；前端登录页加 passkey 按钮（按 /api/auth/status 按需显示）。 |
| P2 | 引导向导的 blocking probe-gate 模式（Continue 前强制验证端点可达） | swarmx 的设置页当前可以填写无效的 API endpoint/model tier 并保存，之后 spawn 失败才报错，错误在 worker 侧。probe-gate 可以前移到设置保存时，比如验证 codex base_url 或 claude API key 有效性。 | 低：后端 /api/settings/probe 接口（发一个最小 models list 请求）；前端设置页 provider 字段旁加 Test 按钮 + 状态 banner | routes/settings.rs 加 /api/settings/probe；前端 settings.js 的 per-CLI model 配置区加 probe 逻辑，类似 hermes onboarding_probe_banner 三态渲染。 |
| P2 | 自托管图标字典 + li() 工厂函数（无 CDN、无 sprite） | swarmx 已部分用内联 SVG，但不统一。hermes 的 LI_PATHS + li() 模式把 50+ 图标集中管理，更新/新增有明确位置，调用侧简洁（li('folder', 16)）。CSP 严格环境下 CDN SVG sprite 会有 integrity 问题 | 低：新建 icons.js（或 icons.ts），把现有散落的内联 SVG path 迁移进去；全局替换 innerHTML 中的 SVG 字符串 | swarmx frontend/src/icons.js；Rust 侧动态 HTML 生成也可以用同格式常量加 format!() 宏 |
| P2 | 字体大小分档用户偏好（small/default/large/xlarge） | swarmx 目标用户是程序员，有人用小字看更多代码，有人用大字防疲劳。hermes 证明：用 data-font-size attribute + 分档 CSS override 比改 CSS var 更稳健（不破坏 px 值组件） | 中：style.css 加 :root[data-font-size] 覆写规则（session-item/msg-body/各级标题/code/table/textarea/file-item 等关键区域）；settings 加 font-size picker；boot 时 localStorage 同步 | swarmx style.css + settings/appearance section（已有 theme picker，扩展即可） |
| P2 | Sidebar tab 可见性/顺序用户自定义（drag-to-reorder + 隐藏 chip） | swarmx 侧边栏 tab 是固定的。hermes 允许用户拖拽重排、隐藏不需要的 tab，chat/settings 固定不可隐藏。这对 swarmx 的程序员用户不用所有面板的场景有实际价值 | 中：前端加拖拽排序逻辑（用 HTML5 draggable API）；localStorage 持久化；boot 时 IIFE 恢复（防 flash）；settings/appearance 加 chip 编辑器 | swarmx 前端 tab 控制逻辑 + settings/appearance；方向(thread) tab 天然也受益于此机制 |
| P2 | Monotonic generation counter + segment ownership 的 memory commit 序列化 | swarmx 的 WakeCoordinator 控制 worker 唤醒，但无 memory extraction 生命周期管理。如果未来加入记忆/总结类 provider（如 context 压缩），需要精确的 commit 序列化防双提交 | 高：需要引入 generation counter + in_flight guard 数据结构，与现有 session DB 集成 | 落在 session_lifecycle 模块（新建）。当前 swarmx 无 memory extraction，此模式在加入 context 压缩/summarization 功能时再落地 |
| P2 | 压缩链 lineage 折叠 + last_activity 排序覆盖（_project_agent_session_rows） | swarmx 多方向功能中，一个 workspace 有多个 direction，每个 direction 可能有多次编排（压缩/重开）。sidebar 应把同一 direction 的连续编排显示为一个条目，以最新活动时间排序 | 中等：需要在 DB 记录 direction lineage，sidebar 列表时做折叠投影 | 落在 directions 列表 API（/api/workspaces/:id/directions）。给 direction 加 parent_direction_id + end_reason 字段，sidebar 调用时做折叠投影，tip 的 last_activity 覆盖 root 行排序键 |
| P2 | 两阶段变更检测（cheap fingerprint → expensive query） | swarmx gateway_watcher 模式适用于黑板轮询场景：目前 blackboard 变更用 WS 广播是实时的，但如果加历史回放/归档查询，可用 cheap fingerprint（仅扫 agent_id+updated_at）做 guard，避免每次都跑全量 JOIN | 小：在现有 SQLite 查询层加一个 fingerprint 辅助函数，在高频路径（如 /api/sessions list）调用 | src/db.rs：add_fingerprint_query()，在 list_agents handler 前做 ETag/Last-Modified 比对，命中则 304 |
| P2 | 系统资源监控面板（CPU/内存/磁盘），无 psutil | swarmx 已有 MCP 管理页和设置页，缺少 worker 宿主机资源监控。hermes 纯 /proc 实现可移植到 Rust（read_to_string + parse），嵌入 system_health 端点供设置页或蜂群页展示 | 小：sysinfo crate 或手写 /proc/stat 解析，一个 GET /api/system/health 路由，设置页加一个资源 card | src/routes/system_health.rs（新）+ 前端 settings 页 SystemHealthCard 组件 |
| P2 | Gateway chat bridge 模式：保持浏览器 SSE 协议不变，后台翻译外部 API 响应 | swarmx 将来可能需要支持原生 API 模式（非 PTY），gateway_chat 的设计证明可以在不改前端协议的情况下切换后端：/api/chat/start 返回相同 stream_id，后台 worker 把 OpenAI SSE 翻译为 swarmx 内部事件（token/tool/done） | 大：需要 native API 适配层，现阶段 PTY 是核心约束，但可作为架构预留 | src/gateway_chat.rs（预留）：当 worker CLI 为 api 模式时，spawn 一个 HTTP SSE consumer task 替代 PTY，向同一 WS channel 推事件 |
| P2 | prompt_cache_hit_percent 后端统一计算 | cache hit 率计算逻辑容易在前端多处分叉；统一在后端计算并返回 | 极低：一个纯函数，加到 token 统计 API | 在 /api/agents/:id/usage 响应中加 cache_hit_percent 字段；读 claude JSONL 的 cache_read_input_tokens 和 input_tokens |
| P2 | 临时 GIT_INDEX_FILE 实现 selected-file commit | swarmx git 合并时需要精确提交特定文件，现在靠 agent 自己 git add，不稳定 | 低：tempfile::NamedTempFile + GIT_INDEX_FILE env，Command stage selected → commit → cleanup | git_merge_closure 的合并预览/提交路径；merge-resolver worker 可调用此 API 做精确提交 |
| P2 | Checkpoint（shadow git）列表 + diff + restore | swarmx worker 改坏文件后无快照回滚能力；hermes 用 shadow git repo 做 per-commit checkpoint | 高：需要 CheckpointManager（在 worker spawn 前/后自动 snapshot）+ REST API + 前端 diff viewer | spawn.rs 在 worker 开始前触发 checkpoint；rollback.rs Rust 实现（subprocess git clone --mirror 到 ~/.swarmx/checkpoints/<ws_hash>/<id>/）；diff viewer 复用现有代码块预览 |
| P2 | Goal 系统：持久化目标 + turn budget + LLM judge + 自动 continuation prompt | swarmx orchestrator 有任务派发但缺乏「目标是否达成」的自动评判和 turn budget 管理 | 高：Goal 状态机（active/paused/done）持久化到 SQLite；每轮 worker 输出后 claude -p headless 调用 judge；budget 耗尽 pause 而非 kill | orchestrator.rs 的任务完成检测；黑板 done key 写入时附加 goal_verdict；类比 /goal 命令作为 UI 控制入口（聊天 /goal set <text>） |
| P2 | Background task tracking（parent→child session 关联） | swarmx 已有 orchestrator 派发 worker，但 UI 对 background 子任务的映射（哪个任务属于哪次 orchestrator 派发）不清晰 | 低：在 spawn metadata 加 parent_agent_id；台账按 parent 分组展示子 worker | 现有 agents 表加 parent_agent_id 列；蜂群成员栏按树状展示；台账 parent_task_id 外键 |
| P2 | Skill usage 只读统计（agent 写、WebUI 读） | swarmx 无 skill/role 使用频次统计，难以评估哪些角色注册表条目被频繁调用 | 极低：在 role_registry 加 use_count 字段；spawn 时 +1；/api/roles GET 附带 use_count | roles_config.rs + roles table；设置页「角色注册表」展示使用频次 |
| P2 | MCP server 直接 import 内部模块而非走 HTTP | swarmx mcp_server 目前应该走 HTTP，hermes 的 insight：只读操作直接访问数据层、写操作走 HTTP 保证 cache 同步。对 swarmx：MCP 工具里 list_agents/list_blackboard 可直接读 SQLite，写 blackboard 必须走 REST API 保证 WakeCoordinator 感知 | 低，架构决策 | mcp_server 模块：区分 read-only(直查 SQLite) vs write(POST /api/blackboard) 两条路径，参考 hermes mcp_server.py 的 _api_post vs 直接 load_projects() |
| P2 | 廉价指纹 + 昂贵 projection 双层轮询（实时进度优化） | swarmx transcript.rs 当前每 N ms 读所有活跃 agent 的 JSONL tail，大并发时 I/O 线性增长。参考 hermes gateway_watcher 的 cheap fingerprint（只看 sessions 表元数据）→ 完整 projection 双层：先 md5(session_id+updated_at)，变化才读 messages | 中 | src/transcript.rs 的 TailWatcher：先 fstat() 对比 mtime/size，不变则跳过 read，变化才 seek+read 新行；广播 AgentActivity。相比 hermes 更简单因为直接读文件不查数据库 |
| P2 | Supervisor 自动检测 + foreground/detached 双模启动 | swarmx 有 ctl.sh 但没有自动检测 systemd/launchd 的逻辑，导致 launchd plist 里需要特殊配置。参考 hermes 的 _detect_supervisor：读 INVOCATION_ID/JOURNAL_STREAM/XPC_SERVICE_NAME，自动选 foreground 模式 | 低 | bootstrap 脚本或 Tauri sidecar 启动脚本：检测 supervisor env var，决定是否 execv/直接运行 vs 后台 spawn + 健康等待 |
| P2 | 测试时 socket-level 网络隔离（monkey-patch create_connection） | swarmx 测试里 agent spawn 会真实调用 claude/codex CLI，危险且慢。hermes 在 server 进程级别 patch socket，只允许 loopback。Rust 对应可用 HERMES_TEST_NETWORK_BLOCK=1 env var 触发，在 tokio::net::TcpStream::connect 前检查地址 | 中，需要 mock 框架配合 | tests/harness_check 或 e2e 测试：用 cfg(test) feature flag + 全局 AtomicBool，connect 时检查地址是否 loopback，非测试环境零开销 |

