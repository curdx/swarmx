# swarmx 工作空间 — 实测第一手记录（2026-06-09，浏览器实操）

测试方式：chrome-devtools 驱动真实浏览器，后端 + vite dev 真实运行，创建了真实工作空间（/tmp/swarmx-ux-test 迷你项目）。截图在本目录 01-07。

## 实测走过的完整流程

1. **空状态首页** (`/chat`)：营销式 hero（"让 Claude / Codex 在你的项目里协同工作"）+ 3 步引导卡 + 3 个功能卡片 + 「新建工作空间」。
2. **创建对话框**（CreateWizard）：两步合一的单弹窗 — ①命名 + 5 色标识色；②主项目路径（浏览器模式只能手动粘贴绝对路径，文件夹选择器 disabled，tooltip 说"桌面 app 内可直接选文件夹"）。可展开「高级源码上下文」accordion 挂载更多仓库。底部文案："点击「开始」后，AI 会先了解一下你的项目"。
3. **点击开始**：弹窗变成等待态："AI 正在了解你的项目… 已等待 Ns · 通常 20-40s"，附「直接进入房间」按钮。
4. **房间内** (`/chat/<id>`)：自动弹出 4 步新手引导 tour：
   - 1/4 "左边是你的工作空间"：每个工作空间 = 一个项目 + 一支 AI 团队；可开多条「方向」并行推进。
   - 2/4 "中间有 4 个视角"：聊天 / 协作图 / 工作台账 / 录像，⌘1-4 切换。
   - 3/4 "底部输入框"：自然语言派任务，调度 agent 自动拆解派工。
   - 4/4 "⌘K 命令面板"。
5. **房间布局**：三栏 — 全局导航（MCP/文件/终端/定时/目标/任务/用量/设置）｜工作空间侧栏（空间列表 + 工作方向 + 源码上下文）｜主区（4 个 tab + 聊天流 + 输入框）+ 右侧成员栏（orchestrator，"调度"徽章，claude · a635ec7c，「唤醒」按钮）。
6. **协作图**：单个 orchestrator 节点 + 图例（已满足/等待中/派生）+「全停」「全恢复」按钮。空状态基本没信息量。
7. **工作台账**：左「任务台账（目标·假设·计划DAG）」右「进展状态」，均为空："还没写。orchestrator 第一次 wake 时会自动建立。"；底部「近况」worker 心跳区。
8. **点成员行**：右侧抽屉打开，含该 agent 的实时 xterm 终端（URL 变 ?agent=claude-a635ec7c）。

## 实测命中的关键问题（按严重度）

### P0 静默失败 / 状态撒谎
- 创建流程承诺"AI 会扫描项目…20-40 秒后在聊天中打招呼"。实际 orchestrator 的 claude CLI 未登录（本次是沙箱环境 Keychain 不可达所致，但**任何** bootstrap 失败——限流、CLI 崩溃、hook 卡死——表现完全一样）：聊天永远"暂无消息"，**没有任何错误提示、没有超时兜底、没有诊断入口**。
- 与此同时成员面板显示**绿色在线点**、房间顶部显示"1 个 AI 在线"、"AI 引擎就绪：Claude Code / Codex CLI"——状态全在撒谎。绿点的语义是"PTY 进程活着"，不是"agent 能干活"。
- 服务端日志里其实有完整线索（bootstrap injected 等），但 UI 不消费这些事实。
- 用户唯一能发现真相的途径：点成员行 → 抽屉 → 肉眼读终端里的红字 "Not logged in · Please run /login"。

### P1 概念过载（一个新用户进门要建立 ~9 个心智概念）
工作空间、方向（= git worktree）、源码上下文（挂载仓库）、agent/成员、orchestrator/调度、4 个视角（聊天/协作图/工作台账/录像）、blackboard 台账、唤醒、spell/编排。全局导航还有另外 8 项（MCP/文件/终端/定时/目标/任务/用量/设置）。
- "方向"这个名字非常抽象；"主线 main" 同时显示中文名+分支名。
- "工作台账"、"唤醒"、"wake"、"派生" 等系统内部行话直接漏到 UI 文案里。
- 全局"任务"页与空间内"工作台账"的关系不清楚；全局"终端"页（人用的 shell）与 agent 终端（藏在成员抽屉里）也容易混淆。

### P1 布局 / 信息层级
- 三条左侧竖栏（全局导航 + 工作空间栏）+ 右成员栏，留给内容的空间被切碎；工作空间侧栏里"1 agent / 1 方向 / 1 源码"三个统计 chip 信息价值低但占据黄金位置。
- 侧栏顶部和底部各有一个「新建工作空间」按钮（重复）。
- 删除工作空间按钮直接放在空间行上（一次误点风险）。
- 空间列表项信息密：名称 + main 徽章 + 目录名 + 未读数 + 删除钮挤在一行。

### P2 空状态浪费
- 协作图/工作台账的空状态只说"还没写"，没有引导（例如：示例图、第一步建议、或者干脆引导回聊天）。
- 聊天空状态"暂无消息"——但创建流程明明承诺 AI 会打招呼，空状态没有衔接这个预期（哪怕一句"orchestrator 正在初始化…"）。

### P2 浏览器模式劣化
- 文件夹选择器在浏览器里禁用，必须手动粘绝对路径（提示文案把责任推给"浏览器调试模式"）。路径输错没有即时校验反馈（未测完整，待查代码确认）。

## 关键代码路径（给后续分析用）
- 创建向导：web/src/components/workspace/CreateWizard.tsx
- 房间外壳/侧栏：web/src/routes/workspace/Shell.tsx, WorkspaceSidebar.tsx, useWorkspaceShellData.ts
- 四视图：web/src/routes/workspace/views/（Chat.tsx, Ledger.tsx, dag, replays）
- 首页空态：web/src/routes/chat/Home.tsx, components/Welcome.tsx
- 成员抽屉：web/src/components/agent/AgentDrawer.tsx
- 新手引导：web/src/components/OnboardingTour.tsx
- 工作空间状态：web/src/lib/workspace.ts, lib/activeWorkspace.ts
- 后端：crates/swarmx-server/src/（routes, spawn）
