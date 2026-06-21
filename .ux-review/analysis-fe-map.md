# 前端信息架构分析 — swarmx 工作空间（代码深读）

依据：web/src 实码（commit 95c9fa5）+ .ux-review/firsthand-findings.md 实测记录。

---

## 一、完整 IA 地图

### 1.1 路由总表（App.tsx）

```
/                              → redirect /chat
/chat                          ChatHome（Welcome 屏 + 左侧工作空间列表；有空间即自动跳第一个）
/chat/:wsId                    WorkspaceShell（layout route）
  ├─ (index)                   ChatView（聊天）
  ├─ /dag                      DagView（协作图）
  ├─ /ledger                   LedgerView（工作台账）
  ├─ /replays                  ReplaysView（录像库）
  ├─ /context                  → redirect ../ledger（老书签兼容；丢失 ?key 参数，见 3.1）
  └─ /t/:threadSlug/…          同上 5 条，按「方向」作用域
/chat/:wsId/replays/:recId     全屏录像播放器（脱离 AppShell）
/mcp /files /terminal /cron /goals /tasks /usage /notifications /settings(/:section)  全局工具页
/debug                         调试面板（DEBUG_ENABLED 时）
?agent=<id>                    任意房间视图打开右侧 AgentDrawer；?tab= 控制抽屉内 tab
```

### 1.2 视觉层级（一个房间内用户面对的 chrome 栈，桌面端）

```
AppShell 顶栏 (44px)：品牌 logo(→/chat) · ⌘K 搜索按钮 · 通知铃 popover
├─ McpActivityBar 全局左栏 (184px / 收起 56px)：MCP·文件·终端·定时·目标·任务·用量 ＋ 设置 ＋ 收起开关
├─ WorkspaceList 工作空间栏 (264px, <md 隐藏改 Sheet)：
│    标题行「工作空间」＋ 小[+]（新建入口①）
│    每个空间行：accent 圆点 + 名称 + dirty 黄点 + 分支 caption + 目录名 + 成员数 + hover[挂载源码|删除]
│    激活空间展开卡片：
│      · WorkspaceHealthLine 三统计 chip（N agent / N 方向 / N 源码）
│      · 「工作方向」section：主线(Home图标)/各方向行（worktree=紫分支图标、degraded=断链、preparing=spinner、
│         dirty 黄点、↑N↓M、hover[×删方向]）＋ [+]开新方向
│      · 「源码上下文」section：根树（主项目+「AI 终端在此」Home 标、对等项目、依赖/工具 chip）＋ [+]管理挂载
│    底部通栏大按钮「新建工作空间」（入口②，与顶部小+重复；空列表时隐藏）
├─ 主区：
│    WorkspaceToolbar：tab 栏 聊天|协作图|工作台账|录像(⌘1-4，<lg 收成纯图标) ＋ [合并到主线(条件)] [LIVE] [未读→跳转]
│    ChatView：
│      · WorkspaceStatusStrip 状态条：空间名/方向名 · N 个 AI 在线|AI 未在线 · N 个源码上下文 · 分支 ·
│        「AI 引擎就绪：…」；0 成员时[唤醒调度]，无 CLI 时[安装 AI 引擎→/settings/plugins]
│      · MessagesPanel（消息流+输入框：@对象、模型/思考强度(改了会杀掉重启 orchestrator)、优化输入、图片、
│        TaskActivity 浮条「✨正在启动 N 个 agent」）
│      · OnboardingTour（4 步浮层，localStorage v1 一次性）
└─ 右成员栏 (320px，仅 ≥2xl=1536px 显示)：成员行(头像+角色+「调度」徽章+状态点/typing+活动行+未读+⚡唤醒)
     ＋「近况」worker 心跳列表（blackboard *.progress.md）
```

关键事实：一屏最多 4 条竖栏 + 2 条横 bar；右成员栏 <1536px 整体消失（Chat.tsx:776，注释自认 R2-004），
而成员状态/唤醒的主要入口就在这条栏。

### 1.3 用户路径全集（按钮→去向）

1. **首次创建**：/chat Welcome[新建工作空间] → CreateWizard（①名称+5 标识色；②主项目绝对路径
   （浏览器模式选文件夹 disabled）+「高级源码上下文」accordion(对等项目/依赖/工具+挂到 parent)）
   → [开始] → POST /workspaces → 逐条 POST /roots → runSpell("init") → 等待视图（假进度条，
   CreateWizard.tsx:835 注释自认"心理安抚条，不跟真实进度挂钩"；60s 超时静默进房）→ 收到
   project.summary.* blackboard 事件即进房 → OnboardingTour → 聊天空态。
2. **发第一条消息**（无活 orchestrator）：sendBootstrappingOrchestrator = runSpell(init)+sendMessage+wake
   （Chat.tsx:495）。"发消息即上线"，UI 未解释此行为。
3. **唤醒调度**：状态条按钮 / 成员栏空态按钮 → runSpell(init)（同一 spell 复用为复活）。
4. **开新方向**：侧栏[+] → 对话框（留空名=AI 自动命名；可选已有分支；非 git 警告）→ createThread →
   navigate → 前端 spawn orchestrator（仅 state≠preparing；命名方向由后端在 worktree 里 spawn，Shell.tsx:224）。
5. **合并方向**：toolbar[合并到主线]（仅非 main+worktree+ready）→ MergeDialog → 冲突时"AI 正在协调——回聊天看进度" → 清理方向。
6. **删空间**：行 hover 垃圾桶 → 确认框 → killAgent 全部 → soft delete → 跳下一个空间。
7. **管挂载**：行 hover 文件夹+ / 源码 section[+] → ManageRootsDialog（manifest 建议 chip + 手填路径+角色+挂到）。
8. **查 agent**：点成员行 / DAG 节点[打开 Agent Drawer] → ?agent= 抽屉：终端|活动|录像|消息|上下文 5 tab
   + 注入 prompt 条 + SPAWN/TURN/TOKEN/TOOLS/PTY/HOOK 状态条 + 唤醒/暂停。死 agent 默认落录像 tab。
9. **协作图**：图例(已满足/等待中/派生) + 角色过滤 + [全停][全恢复]（确认框）+ 节点详情(CLI/状态/handoff/depends_on)。
10. **工作台账**：任务台账卡 + 进展状态卡（只读 markdown）+ 近况通栏 + [压缩](小模型压台账) [刷新]。
11. **全局工具页**：/tasks 看板（每 worker=一卡，状态自动推导+人工覆盖）、/goals 目标（目标+验收标准+token 预算，
    按方向过滤）、/files、/terminal（人用 shell）、/cron（定时投提示词给"编排器"）、/usage、/mcp。
    这些页用 lib/activeWorkspace.ts 的 localStorage「上次看的空间」做默认作用域——用户不可见的隐式联动。

### 1.4 概念清单（新用户在房间内会遇到的名词）

工作空间 / 方向（=thread=git worktree）/ 主线 / 隔离·隔离中·隔离失败 / 源码上下文·主项目·对等项目·依赖源码·
工具项目·挂载·挂到 / 成员·agent·worker / 调度·orchestrator·编排器（同一物三名）/ 唤醒·暂停·恢复·全停·全恢复 /
4 视角（聊天·协作图·工作台账·录像）/ 任务台账·进展状态·近况·心跳 / 共享区（=blackboard，又名"上下文"）/
派生·handoff·depends_on·已满足·等待中 / 合并到主线·清理方向 / 标识色 / AI 引擎（=CLI 插件）/ 模型·思考强度 /
编排·spell / 注入 prompt / 全局：MCP·文件·终端·定时·目标·任务·用量。
**计 ≥25 个独立概念**，房间内必遇 ~12 个。

---

## 二、文案盘点（暴露给用户的术语 × 行话标注）

| 术语（zh 实际文案） | 出处 | 判定 |
|---|---|---|
| 方向 / 主线 / 未命名方向 / 主方向(goals 页) | chat.workspaceDirections, chat.mainDirection, goals.mainDirection | **行话**（=git worktree/thread）；zh 同物三名：方向/主线/主方向 |
| 工作台账 / 任务台账 / 进展状态 | chat.tabs.ledger, ledger.* | **行话**（Magentic-One ledger 直译） |
| 近况 / 心跳 / "worker 们最近的心跳" | Ledger.tsx:330（硬编码中文）, chat.breadcrumbsTitle | **行话**（系统机制名词） |
| 唤醒 / 唤醒调度 / 唤醒中… | chat.wake, chat.reviveOrchestrator | **行话**（wake 机制直译） |
| 调度（badge）/ orchestrator（ledger 空态、tasks 空态英文直出）/ 编排器（cron 页） | chat.pmBadge; ledger.taskEmpty; cron.subtitle | **同一角色三个名字** |
| 派生 / handoff / depends_on / 已满足 / 等待中 | dag.spawn, dag.handoff, dag.dependsOn | **行话**：DAG 详情直接展示英文字段名 |
| 共享区 vs 上下文（抽屉 tab、孤儿 Context 视图）vs 上下文窗口（usage） | notifications.tabs.blackboard, agent.tabs.context, usage.ctx | **行话+一词多义**：blackboard 两译名，"上下文"撞 LLM context |
| 源码上下文 / 挂载 / 挂到 / 对等项目 | chat.workspaceSources, wizard.role* | **行话**（mount/root 直译） |
| 隔离中… / 隔离失败，正与主线共用同一目录 | chat.directionPreparing/Degraded | **行话**（worktree isolation） |
| shim / --dangerously-skip-permissions / mailbox / blackboard | settings.privacy.approvalModeHint; agent.confirm.wake.desc | **裸内核术语进对话框**（"推动它继续读取 mailbox / blackboard"） |
| PTY·HOOK·SPAWN·TURN·READY·STARTING·EXITED | agent.stat.*, agent.status.*, dag.ready | **行话**，全英文大写 |
| ● live / ○ completed / ▶ recording… / ✓ ready to replay | replays.*（zh 语言包里就是英文） | zh 残留英文 |
| 编排 / 声明式编排 / Spell choreography(en) / 运行编排(死键) | welcome.highlight.spell; chat.runSpell† | **已失效产品概念**：spell 启动器只剩 /debug |
| AI 引擎就绪：{{names}} | chat.workspaceStripCliReady | **语义撒谎**：仅指 CLI 二进制已安装 |
| 注入 prompt | agent.injectLabel | **行话** |
| 工作空间 vs 工作区 | chat.backToCurrent"回到当前工作区"、mcp.pageSubtitle"所有工作区" | 同物两名混用 |
| 任务（全局看板）vs 任务台账（房间内）vs 目标（全局页 + 台账内"目标·假设·计划"） | nav.tasks / chat.tabs.ledger / nav.goals | **三套任务/目标体系无互链无解释**（见 3.3） |
| "3 steps" 徽章 | Welcome.tsx:62 硬编码英文 | zh 界面夹生英文 |
| "后端未加载 \`init\` spell — 请重启 swarmx-server…" | CreateWizard.tsx:316 硬编码 | 开发者口吻错误文案直达用户 |

†死键（locales 有、代码零引用，grep 验证）：chat.runSpell、cmdk.runSpell、wizard.aiNamed、chat.attachSource、
chat.noActiveAgents、chat.emptyStateTitle/Hint、chat.backToChat、chat.globalScopeHint、chat.noConversation、
chat.agentCountWs、chat.copyPath、chat.workspaceViews、nav.context、nav.dag、nav.replays、context.title。
——一整层被废弃的 IA 以文案化石形式留存。

---

## 三、代码可见的未完成 / 不一致

### 3.1 孤儿视图与断链
- **views/Context.tsx（558 行 blackboard 浏览器）已无路由**：App.tsx:84 把 /context 重定向到 ../ledger，
  且 Navigate 丢弃 search 参数。但 **AgentDrawer.tsx:792 的「上下文」tab 仍生成 …/context?key=<path> 链接**
  → 点击落到台账页、选中 key 丢失。完整功能死代码 + 活的坏链接。
- CommandPalette 注释（:91）自述曾发生 ⌘ 提示 off-by-one 的同源事故——context→ledger 迁移是半途态。

### 3.2 状态语义谎言（实测 P0 的代码根源）
- 成员绿点 = shim_ready（PTY 起来了），label "在线"；「N 个 AI 在线」「AI 引擎就绪」同理只看进程/插件安装位。
  **没有任何代码消费"CLI 未登录/bootstrap 失败"类事实**（swarm 事件无此类型）。stalled/noResponse 推导只基于活动时间窗。
- CreateWizard 等待视图：进度条按 30s 线性吸到 95%（:822，注释承认假）；60s 超时**静默**进房（:203 注释称
  "让用户能看到失败状态、自己处理"——但房间里没有任何失败状态可看）。
- 错误处理不对称：createWorkspace 后 runSpell 失败会回滚删空间（:384），但 spawn 成功后 scout 失败（登录、
  限流）无任何兜底——正是实测踩中的洞。

### 3.3 三套任务体系并存、互不相认
- 房间内「工作台账」= orchestrator 写的自由 markdown（blackboard，WS 推送刷新）；
- 全局 /tasks「任务看板」= 每个 worker agent 一张卡，状态由生命周期推导（4s 轮询，tasks.tsx 头注释）；
- 全局 /goals「目标」= 目标+验收标准+token 预算，可按"方向"过滤（goals.tsx 自称"task board 之上的持久层"）。
  数据源、刷新机制、入口层级全不同，UI 零互链、零解释。tour 第 2 步还把台账描述成"看任务进度与共享区"。

### 3.4 双份实现 / 数据层割裂
- ChatHome 自带一份 workspace 组装 + 本地复制 splitWorkspacePath（Home.tsx:35，lib 已有同名）+ 独立
  handleDeleteWorkspace（:130），与 useWorkspaceShellData.deleteWorkspace 几乎逐行重复。
- Shell 注释宣称"单一 /ws/swarm 订阅"（useWorkspaceShellData.ts:10），实际 Chat breadcrumbs、Ledger、Dag、
  Replays、Home、CreateWizard 各自再开 useSwarmFeed + 独立 listAgents/listBlackboard 重拉。
- 未读数来自 listMessages limit 200 的近似计算（recomputeUnread）。

### 3.5 入口与布局冗余/风险（对应实测 P1 的代码位）
- 「新建工作空间」双入口：WorkspaceSidebar.tsx:507（顶部小+）与 :823（底部通栏大按钮）。
- WorkspaceHealthLine 三 chip（:151）占激活卡片首行，信息密度极低。
- 删除按钮 hover 直挂空间行（:216），与挂载按钮相邻。
- 成员栏 hidden 2xl:flex（Chat.tsx:776）：1536px 以下唯一的成员/唤醒面板消失；tab 文字 <lg 收成图标——
  OnboardingTour（无锚点、纯文字宣讲）描述的布局在常见笔记本宽度下与实际不符。
- 切换方向模型 = 杀掉 orchestrator 再重启（Chat.tsx setDirectionModel），副作用只在注释里，UI 仅"切换中，调度重启…"。

### 3.6 文案与产品模型脱节
- Welcome 三卡之一"声明式编排/Spell choreography"宣传的 spell 体系已退役（SpellsLauncher 仅 /debug 引用；
  WorkspaceSidebar.tsx:819 注释明说"没有 spell 选择器了"）。
- 16 个死 i18n 键（§2†）记录了至少三轮 IA 改版（全局 dag/context 页时代 → Shell 化 → ledger 化）的残骸。
- accent 持久化 id 是 legacy 角色名（peach/frontend/backend/test/critic），lib/workspace.ts:50 注释承认
  "peach 实际渲染蓝色"，已用 nameKey 打补丁。
- zh/en 术语映射不一致：ledger=工作台账、dispatcher=调度、direction=方向，zh 侧又各有别名（§2）。

### 3.7 其它
- FULLSTACK_INTERNAL_KEYS 清键是全局的，双空间并发互踩（lib/workspace.ts:91 "KNOWN LIMITATION"）。
- 浏览器模式文件夹选择 disabled + tooltip 甩锅"浏览器调试模式"（wizard.pickFolderUnavailable）；路径即时校验
  其实已实现（350ms debounce + filesList 探测），实测未见提示的原因值得复查展示位置。
- lib/activeWorkspace.ts 的隐式"最后活跃空间"联动无任何 UI 表征，工具页默认作用域像随机的。
