Confirmed: AgentDrawer is 872 lines with five tabs (terminal/activity/recordings/messages/context) — the cockpit reuse claim is solid. I now have all load-bearing facts verified. Writing the synthesis.

---

# flockmux 聊天窗口重设计 · 最终推荐方案

> 合成方法：以评分最高的方向为骨架，嫁接其它方向被评委点名的最佳点子。两位评委分属不同视角——新手视角（conversation-first 88 / thread 80 / cockpit 64）和工程视角（cockpit 78 / conversation 71 / thread 64），结论相反。本方案不二选一，而是**用 conversation-first 的"单声道主轴"心智作为默认体验骨架**（这是新手视角与已批准的 final-redesign.md 都站的一边），**嫁接 cockpit-split 那套被工程评委称为"复用度最高、风险最低工程支点"的右面板/钻取机制作为可选的进阶层**，再从 thread-as-workspace 取"diff 进流的行级评审 + 合并闸门"这一个被两位评委都点名为直觉的收口动作。所有结论已对照真实代码验证（锚点见下）。

---

## 1. 诊断：当前聊天面本身为什么不好用

聚焦聊天面，区别于工作空间级 final-redesign.md：

1. **队长气泡近乎隐形，AI 输出不是视觉主角。** 当前用户蓝气泡是全屏最强元素，队长（真正的 payload）反而弱。这违反"AI 输出是 payload，必须最显眼"的基本盘。证据：`web/src/components/MessagesPanel.tsx:1178` 起的气泡渲染，用户态 accent 实底而队长态低对比。

2. **等待期撒谎：PendingBubble 在成员静默死亡后仍挂 60s。** 后端的真相是 `AgentState` 有 `Error`/`Exited` 七态（`crates/flockmux-protocol/src/ws_swarm.rs:102-118`），且 90s 看门狗真的会 `record_agent_error("watchdog")`（`crates/flockmux-server/src/routes/rest.rs:33,711-730`）——但前端 typing 气泡没绑这些事实，绿点/spinner 在 agent 已死后继续转。证据：watchdog 常量与 `record_agent_error` 调用确证后端已知道"它死了"，是前端没读。

3. **"现在发生什么"在重连/冷加载后变空白。** `GET /api/agent/:id/activity` 的注释明确写它"served from the transcript tailer's in-memory ring"（`rest.rs:1125-1135`），广播通道"drops frames under flood"（`ws_swarm.rs:78-80`）。结果：刷新页面或断线重连后，活动信息归零，比不显示更显眼地撒谎。

4. **firehose 风险结构性存在。** `AgentActivity` 只有 `tool` / `system` 两种 kind（`ws_swarm.rs:74-97`），worker 的 token 流若直接进气泡，N 个并行成员就是 N 路交织。当前没有"成员不发气泡、只升格交付/求助"的闸。

5. **空状态浪费最佳教学时机。** 裸"暂无消息"居中漂浮，是新用户唯一一次"我该说什么"最需要脚手架的时刻，却给了零引导。

6. **失败信息双份、措辞不一致、且被截断。** 失败既进失败卡又进成员栏徽章，关键错误文字被截断成"请在终… 0:00"。单一真相源缺失。

---

## 2. 设计原则

1. **诚实状态（事实律）**：任何状态只描述服务器证实的事实；首个真实输出前绝不出现绿色/"工作中"。*为什么*：后端 `AgentState` 七态 + 90s 看门狗已提供事实背书，前端撒谎纯属没接线。

2. **入流律**：所有人需要知道的状态变更（派工/交付/失败/模型切换/引擎回退/隔离）必须以可重放的事件进流，不能只活在 broadcast。*为什么*：广播 drops frames、ring 重启即没——不持久化则重连后流内卡片全空（rAutonomous 的 replay timeline 前提）。

3. **单声道主轴**：屏幕中央永远只有一条人能读完的叙事轴——你、队长、少数"真的发生了"的系统卡。*为什么*：这是新手视角与 final-redesign.md 共同的骨架，直接治概念过载与 firehose（rMultiAgent：子 agent 在隔离上下文干活，只把凝练结果冒泡给人）。

4. **对话与活动分离的边界**：对话区只承载"人能读完的结论"，高频机器输出（raw 终端、完整 diff、逐 token 流、活动明细）退到可选的右面板/抽屉。*为什么*：cockpit 的双区分离被工程评委评为咬合最紧——但本方案让右面板**默认收起**，把它从 cockpit 的"常驻税"降级为"按需进阶层"，兼顾新手的安静主轴。

5. **渐进披露（altitude control）**：顶层（流）只看队长发声 + 交付/审批/失败卡；中层（卡内展开）看派工的成员动作 + 小团队树；底层（抽屉/面板）看单成员 PTY/活动原始流。*为什么*：三层折叠是三个方向的共识，N 并行时流里只多几行卡，永不 firehose。

6. **可打断可操控**：course-correction 永不被静默排队。`Enter=排队`（pending chip 可见）/ `⌘Enter=打断`（路由队长可中止成员重规划）两态显式区分；每个在跑成员有独立 Stop。*为什么*：rCoding 头号论坛投诉就是 redirect 被静默排队。且后端 `interrupt`/`resume`/`interrupt_all`/`wake_agent` 端点已存在（`rest.rs:1103,1806,1824,1861`），这是被低估的现成可行性，应直接接线而非重写。

7. **稳定身份**：每个成员用角色派生名 + 固定色（复用 `--color-agent-*`），在派工卡、活态行、抽屉、diff 归属处复用同一色同一名，禁止 `worker_7` 裸 ID。*为什么*：rMultiAgent——N>3 时归属是承重设计。

---

## 3. 推荐方案：整屏布局

骨架 = conversation-first 的两栏（会话栏 + 对话主轴），但右侧抽屉升级为 cockpit 的"工作面板"语义——**默认收起，用户决定是否常驻**。这样默认是安静的单声道（新手友好），需要俯瞰时一键召出全队驾驶舱（power user 友好）。

### 1280px 档（默认，对话独占，右面板收为脉搏条）

```
┌──────────┬─────────────────────────────────────────────────┬──┐
│ 会话栏    │ ● 退款流程  工作中·3名成员   [对话] 变更+3   ▣  │脉│ ← 单条 36px 状态行
│ 200px    ├─────────────────────────────────────────────────┤搏│
│          │            （对话主轴 max-w≈720 居中）            │条│
│ 收件箱    │                                                 │64│
│ ⚠2需要你  │              你：把校验抽成独立函数              │px│
│          │   ┌队┐ 队长 · 02:56                              │  │
│ ●退款流程 │   └──┘ 好，我拆两步…                              │●队│
│ ●对账定时 │   ┌─ 计划 1/3 ──────────────────────┐           │长│
│ ✓webhook │   │ ✓ 抽出 validateRefundAmount      │           │●测│ ← 脉搏条:
│          │   │ ◐ 补失败用例 · 测试成员           │           │试│   彩色态点
│ + 新会话  │   │ ○ 跑全套测试                      │           │  │   + 数字徽章
│          │   └──────────────────────────────────┘           │  │   点击展开
│          │  ▏派给 测试成员：补失败用例   ●进行中 ▾          │  │   为右面板
│          │  ┌测┐ 测试成员 正在 写 refund.test.ts ⋯ 14s·3文件 │  │
│          ├─────────────────────────────────────────────────┤  │
│          │ ┌ 发消息给队长，或 @成员… ┐  opus·中 @ ✦ 📎  ↑  │  │ ← Composer
│          │ └────────────────────────┘  Enter发送            │  │
└──────────┴─────────────────────────────────────────────────┴──┘
```

- 对话主轴居中限宽（max-w≈720），即使两条短消息也不贴顶留大片空白（治诊断 5）。
- 右侧默认是 **64px 脉搏条**（cockpit 的窄屏降级被提为默认）：只显成员彩色态点 + 数字徽章。**单成员简单任务时它就是这条窄条**——这正是工程评委给 cockpit 扣的"常驻税"，本方案用"默认收起"消解。
- 点脉搏条任一成员 / 点流内派工卡 → 右面板**滑出覆盖**为该成员焦点（终端/活动/变更），看完收回。

### 1536px+ 档（右面板可常驻，仍由用户决定）

```
┌──────────┬───────────────────────────────┬──────────────────────┐
│ 会话栏    │ ● 退款流程 工作中·3名成员 对话 ▣│  工作面板  [全部▾] ⤢  │
│ 220px    ├───────────────────────────────┤  ────────────────────│
│          │        （对话主轴居中限宽）      │ ●队长 ●测试·写测试    │ ← 乐队栏(横向chip)
│ 收件箱    │   你：把校验抽成独立函数         │ ◐后端·启动 ⚠前端·等你 │
│          │   队长：好，我拆两步…            │  ────────────────────│
│ 退款流程  │   [计划卡][派工卡][活态行]      │ 现在(live顶置):       │
│ 对账定时  │                               │ ●测试 写 refund.test  │ ← two-signal:
│          │   [Composer]                  │   14s·上次2s前 ⌃▣     │   动词+计时
│ + 新会话  │                               │  ────────────────────│
│          │                               │ [活动][变更3][终端]    │ ← 深度tab
│          │                               │ ✓队长 派工→测试       │   (复用AgentDrawer
│          │                               │ ✓测试 编辑 refund +12 │    五tab)
└──────────┴───────────────────────────────┴──────────────────────┘
```

- 右面板 = `AgentDrawer.tsx`（872 行，terminal/activity/recordings/messages/context 五 tab 已齐，`web/src/components/agent/AgentDrawer.tsx:82-92`）**从 880px 抽屉原地搬成右面板**。这是工程评委一致认定的"整套重设计里复用度最高、风险最低的工程支点"——钻取焦点 = 换面板内容，不新建。
- **取舍**：1536+ 不把对话拉到全宽（过宽伤行长可读性，对话 capped 居中），右侧空间给工作面板。对话永远在左不动，`⌘1/2/3` 切深度 tab。

**取舍说明**：相比纯 cockpit，本方案在 1280px **不强制 58/42 双区**（避开 cockpit 被扣的"对话压成 52ch 窄带 + 单成员时面板稀疏"），把双区作为 1536+ 与"用户主动召出"时的能力，而非默认。相比纯 conversation-first，本方案保留 cockpit 的"右面板是机器细节的家"语义，避免"全部下沉抽屉、重并行可观测性藏太深"（conversation 被扣的点）。

---

## 4. 逐组件规格

### 4.1 消息流：4 类渲染物

只有前两类是气泡，后两类是系统卡/活态行——这是控制 firehose 的物理隔离。

| 类型 | 形态 | 复用 / 新建 |
|---|---|---|
| **你的消息** | 右对齐 accent 实底气泡，无头像 | 复用 `MessagesPanel.tsx:1178`。**改**：移除 user→user 的"回复"入口（无语义，诊断遗留），改为"引用任意流内卡片"作为下条上下文 |
| **队长消息** | 左对齐、**强对比正文**（`--color-foreground-primary` 深色 + 蓝头像「队」），名字+时间在上方一行；`▸思考摘要`降级为暗色折叠块默认收起 | 复用 `ChatMarkdown.tsx`（GFM/代码/预览全保留）+ `MessagesPanel.tsx:1606` 的折叠块。**改**：提对比度（治诊断 1），思考块不混入结论（rPrinciples） |
| **成员消息** | **默认不进流**。成员通过队长说话，只在三时刻浮上来：①队长引用 ②交付 ③求助。`@成员`定向对话时升格为成员色独立气泡（临时内联子线程，用完收拢） | 复用 `@autocomplete`（`MessagesPanel.tsx:728-766`） |
| **系统事件卡** | 居中/带左色条，形状明显区别于气泡（派工/交付/计划/失败/模型切换/回退/隔离） | 见下分项 |

**系统事件卡解剖（统一语法）**：左侧事件类型徽章（非头像）+ 一行原因 + 可选行动条 + 可选 `▾` 展开。每张卡服务器持久化、刷新/重连可完整重放（见 §6 P0 后端）。

**时间分隔/未读**：复用 `MessagesPanel.tsx` 的 >5min 时间线与"N 条新消息"分隔。**改**：未读计数改用精确端点 `GROUP BY from,thread WHERE to=user AND read_at IS NULL`，重连即拉，不把系统噪声数进徽章（治诊断 6）。

### 4.2「现在发生什么」机制（填 doomscrolling gap）

三个组件，全部 two-signal 诚实降级（这是两位评委都点名保留的最佳点子）：

**① 队长 typing 行（入流律最小实现）**：队长收到消息 **1-2s 内**必须出现 `队长正在 思考… → 派工… → 合并结果…`，来自 `thought_trace.status` + 首个 `AgentActivity`。proof of life 永远先于 spinner（治诊断 2 根因）。

**② 成员心跳活态行（非气泡）**：
```
┌测┐ 测试成员 正在 写 refund.test.ts  ⋯  14s · 上次输出 2s 前
```
- 色头像 + "正在 + 用户动词"（把 `AgentActivity.label` 翻成"写/跑测试/读文件"，**绝不**泄漏 worktree/blackboard/wake/spell/PTY）+ 三点 + **两个会动的数字**（计时 + 文件数/上次输出）。
- **45s 无输出 → 降级**为 `⚠ 已 50s 无活动`（灰），用 `last_activity_at`（`store.rs:438-444` 的 `touch_agent_activity` 已有此字段）。
- **绑 `AgentState`，agent 死即移除**——直接修 PendingBubble 静默挂 60s 的根因（诊断 2）。

**③ 派工卡展开 = 该成员活动时间线**：折叠态一行；展开态按时间线排 `installing/editing X/running tests`，raw CLI 流再下一层进抽屉终端。

**首卡占位即时落地（两评委都点名）**：spawn 时**不等 700ms tailer**，同步落一张 `◐启动中` 占位事件，保证 1-2s 首卡，堵住"盯绿点"空窗。

**断线兜底**：活态行/派工卡在 WS 重连时先打 `GET /api/agent/:id/activity` 回填再接流——但**前提是该端点不再只读 in-memory ring**（见 §6 P1，这是诊断 3 的硬依赖）。

### 4.3 计划/派工/交付/审批/diff 的呈现与操控

| 事件 | 呈现 | 在哪 | 可操控 | 复用/新建 |
|---|---|---|---|---|
| **计划** | 置顶粘性卡，活 checklist `✓/◐/○`，每项标拥有成员；卡头显队长健康度 | 入流（轴顶钉住） | 派工前 `[批准计划][改]` 闸门（可选自治级）；**渐进增强降级**：队长首条不符计划结构则当普通消息渲染，卡区提示"尚未给出计划"，不阻塞流 | 复用台账 markdown；**新建**置顶卡容器 |
| **派工** | 派工卡（左色条 + ●进行中），`▾` 展开成小团队树（队长→成员→依赖） | 入流（折叠一行） | 点成员名→过滤/跳焦点；展开看子步骤。**取代独立 DAG 视图** | DAG 字段 `depends_on/produces/consumes` 已存在（`web/src/api/types.ts:92` + `dagEdgeDerivation.ts`）。**注意**：内联小团队树是从 843 行 `Dag.tsx` 的**重写**，不是白捡——需在排期承认 |
| **交付** | 交付卡：`N 文件 +x/−y` + 测试输出折叠 + `[查看变更]` | 入流 | `[查看变更]`跳变更 tab | **新建**卡，数据来自 worktree diff + thread `dirty/ahead/behind` |
| **审批/需要你** | 三要素卡：①做什么 ②为什么 ③预期结果 + `[批准][拒绝][看终端]` | 入流（红/琥珀，永不消失）+ 收件箱 + 桌面通知 | 禁裸 APPROVE/DENY | **新建**卡 |
| **diff/变更** | **破例进流的收口动作**（thread 取此点）：1280px 点`[查看变更]`→流内就地展开为大卡（file list + 逐文件折叠，虚拟化）；1536+→右面板变更 tab，对话不被推走 | 流内/面板 | 行级评论（点行→"让队长改这里"→带 file:line 发成员）；**合并闸**：所有文件已看勾选 + rebase✓ + 测试○ + 评论已解决○ 才点亮 `[合并回 main]`，per-file accept 可挑拣 | **新建** review 面（flockmux 首个）；门从"写前"移到"合并前"（rCoding 共识） |

### 4.4 Composer（含打断/排队/@/模型/优化/附件/键盘）

复用 `MessagesPanel.tsx` 的整套 Composer 骨架，重排为单卡并补 HITL 控制：

```
┌─────────────────────────────────────────────────────────┐
│ [📎 login.png ×]  ← 附件缩略图(上传失败→红框「未上传」)    │
│ 发消息给队长，或 @成员 直接对话…                          │
│ opus·中▾  @  ✦优化  📎      Enter发送  [打断▾]      ↑    │
└─────────────────────────────────────────────────────────┘
```

- **默认收件人=队长**；`@成员`内联 autocomplete 定向对话（复用 `MessagesPanel.tsx:728-766`）；`@`也能选文件给行级上下文。
- **打断 vs 排队（原则 6）**：`Enter=排队`（灰 pending chip 显示在 Composer 上方）/`⌘Enter=打断`（路由队长，中止成员重规划，点击前弹一行确认）。每个在跑成员有独立 `■停`。**直接接** `interrupt`/`resume`/`interrupt_all` 端点（`rest.rs:1806,1824,1861`），不重写。
- **模型选择**：`opus·high▾` pill（复用 `ModelPicker.tsx`）。**改（治诊断遗留）**：切换前弹"换模型会重启队长、打断在跑回复 [取消][确认]"，执行后入流一张"模型已切换"系统卡。`ModelPicker.tsx:12` 自己的注释确证它"restarts the live orchestrator"，所以加确认弹窗是小改而非新建。
- **优化**：复用 `/api/optimizePrompt` 的 ✦ 就地改写 + undo。
- **附件**：粘贴/拖拽/路径。**改**：上传失败的缩略图变红框 + "未上传 重试"并禁用发送，不让用户误以为带了图（治诊断遗留）。
- **键盘**：桌面 `Enter发送 / Shift+Enter换行`；移动端 `Enter换行 + 显式发送键`（复用现状）。Composer 下方常驻 `Enter发送` 提示 + 三态 placeholder 加 `aria-describedby` 说明为何不可发。
- **草稿持久化**：每（会话,收件人）keyed（复用 `MessagesPanel.tsx:263`）。**改**：切到无活队长会话时显式提示"发送将启动队长"而非静默 bootstrap。

### 4.5 成员/活动面板（右面板）

- **乐队栏**（cockpit 取此点，被评委称"成员=活动主体，不是静态名牌"）：横向 chip，每个 = 角色色 + 五态徽章 + 一行"正在<动作>"小字。点 chip → 面板进单成员焦点模式。
- **现在 live 区**（面板顶置常驻）：每个在跑成员一行 two-signal 卡 + `⌃▣` 一键看终端。
- **深度 tab**：`[活动][变更][终端]` = `AgentDrawer` 五 tab 内容原地换，不再 880px 抢屏抽屉。
- **复用**：整个 `AgentDrawer.tsx`（终端 `XtermPane` PTY、活动 tab 的"finished turns 折一行"逻辑、录像、消息、上下文）。**新建**：乐队栏 + live 顶置区的容器；焦点模式的"全部↔单成员"切换。

### 4.6 空/启动/失败状态（诚实，无假绿点）

**空状态（治诊断 5）**：对话主轴居中能力揭示问候 + 3 个可点 starter prompt（"重构这个函数/补测试/查这个 bug"），点击填入 Composer；右面板空态自我说明"这里会实时显示每个成员在做什么"（教会用户右边是看活动的地方）。引擎预检诚实显示（✓Claude 已登录 / ✕Codex 未装）。

**启动中（事实律 + 90s 看门狗）**：发首条后轴里出现**启动清单卡**，逐步消费真实 bootstrap 事件：
```
✓ 隔离到分支 main↘退款流程
✓ Claude Code 已登录
◐ 启动队长引擎…  8s
○ 等待首次响应
```
徽章 `◐启动中`（非绿）。任一步失败或 90s 看门狗触发（`FIRST_RESPONSE_WATCHDOG_MS=90_000`，`rest.rs:33`，真的 fire `record_agent_error("watchdog")`）→ 清单卡**原地翻转**为失败卡，不换视图、不静默。**复用** `TaskActivity.tsx` 状态机，**新建** `failed` 态。

**失败状态（失败卡永不消失）**：
```
┌ ✕ 队长还没法开始 ──────────────────────────┐
│ Claude Code 未登录                          │
│ claude /login          [复制]               │
│ [打开终端登录][换 Codex 重试][重试][看日志] │
└─────────────────────────────────────────────┘
```
- **单一真相源（治诊断 6）**：失败只在流内卡片讲一次（权威），乐队栏只显 `✕卡住` 点 + tooltip，**不重复整段错误、不截断**。
- **永不消失纪律**：会话栏保持红 ✕、收件箱保持钉住、可操作直到处理。
- **自动重试封顶 2-3 次**再翻"需要你/换引擎"（rAutonomous 防 silent fix-loop），每次重试都入流。`换引擎`复用 `fallback_from`，是有界恢复动作。
- **接管/交还**：失败卡可 `[看终端]`→面板终端 `[接管]`（暂停成员给键盘去修登录）→`[交还]`（注入"我改了什么"为消息）。
- **复用** `OrchestratorFailureCard.tsx`（在 `web/src/components/workspace/`），泛化为 rate-limit/crash/timeout 各一张 + 加"看日志"第四键。

---

## 5. 被否决的方案与原因

**thread-as-workspace（会话即编排台）——否决为骨架，仅取一点。** 理念最完整（边看边操控、握方向盘），但**后端新建量最大、性能风险最集中**，作为增量首期不可落地。它独有的负担：approve-before-run 需"会话级自治级字段 + spawn 挂起逻辑"（现状 `spawn_bootstrap_inject` 是即时注入，插入审批 gate 是非平凡改动）；QUEUE/INTERRUPT 需"轮次结束才送达"的 queued message 状态机（现状 send 直接投递）；diff 进流在 1280px 就地展开是它自己承认的"真风险点"——大 diff 撑爆滚动。它**牺牲了**对话纯净度（派工/交付/审批/diff 全塞一条流，长会话 20+ 派工会很重）和并行的空间分离。**但它的"diff 进流行级评审 + 合并闸门"被两位评委都评为直觉的收口动作**，已嫁接进 §4.3。

**cockpit-split（驾驶舱/透明优先）——否决为骨架，但其工程内核被吸收为进阶层。** 工程评委给它最高分（78），它与现有架构咬合最紧（AgentDrawer 原地搬面板）——这点完全正确，本方案的右面板就是它。但新手评委给它最低分（64），因为**双区分离 1280px 把对话压到 52ch 窄带、单成员简单任务时面板大半时间稀疏**（与现状 P1"对话被压成窄带"同质，只是换了方向），且"读左看右"的信息分裂对新手是认知负担。它**牺牲了**安静的单声道与新手的低心智门槛。**本方案的处理**：吸收它的右面板/钻取/乐队栏/two-signal/首卡占位/P0-P1-P2 分层（全部已嫁接），但把双区从"默认常驻"降级为"1280px 默认收为脉搏条、用户主动召出、1536+ 才常驻"——消解它唯一被扣的"常驻税"。

---

## 6. 分期落地计划

三个方向共担两条已验证的硬依赖，且**有一个三者都漏掉的省事实**：`messages` 表已有 `kind` + `meta` 列（`store.rs:1070` 的 INSERT 含 `kind,meta`），系统事件可直接落 `kind=system + 结构化 meta`——**砍掉三个方向都估的"半天 spike：messages 表 vs swarm_events 表"这个伪 spike**。

### P0（最低成本最高收益，不需大改后端 — 接现有信号 + 前端诚实化）

| 项 | 前端量级 | 后端量级 | 依赖 |
|---|---|---|---|
| 队长气泡提对比度 + 思考块降级折叠（治诊断 1） | S | 0 | 复用 ChatMarkdown |
| two-signal 活态行 + 绑 AgentState 死即移除（治诊断 2） | M | 0 | `AgentState`/`last_activity_at` 已有 |
| 首卡占位即时落地（spawn 同步落 `◐启动中`，不等 tailer） | S | S | 一个同步事件发射点 |
| 启动清单卡 + 90s 看门狗原地翻失败卡 | M | 0 | 看门狗已 fire |
| 失败卡单一真相源 + 不截断 + 第四键（治诊断 6） | M | 0 | 复用 OrchestratorFailureCard |
| 空状态 3 starter prompts + 引擎预检（治诊断 5） | S | 0 | — |
| Composer：打断/排队两态 + 模型切换确认 + 附件失败回滚 | M | 0 | interrupt/resume 端点已有 |
| 右面板默认收为脉搏条 + 1536+ 召出 AgentDrawer 为面板 | M | 0 | AgentDrawer 五 tab 已齐 |

P0 把"不撒谎 + 不 firehose"立住，**不依赖新表**。

### P1（补持久化 — 兑现冷加载与重放，把面板从 live-only 升到"重连不空"）

| 项 | 前端 | 后端 | 依赖 |
|---|---|---|---|
| 系统事件持久化 + 入流（派工/交付/失败/模型切换/回退/隔离）落 `messages kind=system + meta` | M | M | **复用 messages 已有 kind+meta**（砍伪 spike） |
| `AgentActivity` 持久化 + `GET /api/agent/:id/activity` 走 DB 而非 in-memory ring（治诊断 3） | S | M | 这是入流律/replay 的硬前提 |
| agent 状态转换审计（thinking→idle→error 历史） | S | M | 冷加载需要 |
| 精确未读端点 + 重连即拉 | S | S | — |
| 计划置顶卡 + 渐进增强降级 | M | S | 台账 markdown |

### P2（深度操控 — 性能与后端改动更重）

| 项 | 前端 | 后端 | 依赖 |
|---|---|---|---|
| diff 进流/面板：行级评论 + 合并闸 + per-file accept | L | M | worktree diff 端点已有；评论→消息桥新建 |
| 派工小团队树（**承认是 843 行 Dag.tsx 的内联重写**） | L | S | DAG 字段已有 |
| approve-before-run 闸门（spawn 挂起逻辑） | S | L | 会话级自治级字段新建 |
| 结构化事件 grammar（BootstrapStage/Dispatch/Handoff）覆盖率 | M | L | **day-1 spike 验证 AgentActivity 粒度是否够**；不足处降级文本系统卡 |

---

## 7. 关键未决问题（需用户拍板）

1. **1280px 默认：脉搏条 vs 直接 58/42 双区？** 本方案押"默认收为脉搏条、用户召出"以保新手安静主轴；若你的真实用户多是同时盯 3+ 成员的运维者，应反过来默认常驻双区（接受对话压窄）。这是新手视角 vs power-user 视角的分叉点。

2. **diff review：流内内联（thread）vs 纯右面板 tab（cockpit/base doc）?** 已批准的 final-redesign.md 倾向"变更"独立 tab；本方案折中（1280 流内就地展开 + 1536 面板 tab）。流内更直觉但有大 diff 滚动性能风险；纯 tab 更稳但多一次跨区跳转。是否引入流内 diff review 需你拍板，它决定 P2 的前端量级（L vs M）。

3. **approve-before-run 闸门：要不要做、默认开还是默认关？** 它治"错计划浪费整轮"，但需会话级自治级字段 + spawn 挂起的非平凡后端改动（P2 唯一 L 级后端）。多 agent 并行跑错代价高时值得；轻量单成员任务会显得碍事。是否做、做了默认哪个自治级，需你定。

4. **成员"单声道"的强度：worker 完全不发气泡（conversation）vs 允许 @定向时升格独立气泡（本方案）?** 本方案默认不发、@时升格为临时子线程。若你担心"队长不综合直接转发就 firehose"这个 prompt 合规风险（三方案共有），可考虑更硬的"永不升格、一律走交付卡"——更安全但牺牲与单个成员直接对话的灵活性。

---

**关键文件锚点（绝对路径）**：
- 聊天主体：`/Users/wdx/opc/flockmux-core/web/src/components/MessagesPanel.tsx`（1799 行；Composer/草稿 263、@autocomplete 728-766、气泡 1178、思考块 1606）、`/Users/wdx/opc/flockmux-core/web/src/components/ChatMarkdown.tsx`
- 右面板支点：`/Users/wdx/opc/flockmux-core/web/src/components/agent/AgentDrawer.tsx`（872 行，五 tab 在 :82-92）
- 失败卡：`/Users/wdx/opc/flockmux-core/web/src/components/workspace/OrchestratorFailureCard.tsx`；模型：`/Users/wdx/opc/flockmux-core/web/src/components/ModelPicker.tsx`（:12 注释确证"restarts the live orchestrator"）；DAG：`/Users/wdx/opc/flockmux-core/web/src/routes/workspace/views/Dag.tsx`（843 行）、`/Users/wdx/opc/flockmux-core/web/src/lib/dagEdgeDerivation.ts`、`/Users/wdx/opc/flockmux-core/web/src/api/types.ts:92`
- 后端事件源：`/Users/wdx/opc/flockmux-core/crates/flockmux-protocol/src/ws_swarm.rs`（AgentActivity 仅 tool/system kind :74-97；广播 drops frames :78-80；AgentState 七态 :102-118）
- 后端事实：`/Users/wdx/opc/flockmux-core/crates/flockmux-server/src/routes/rest.rs`（看门狗 :33,711-730；interrupt/resume/interrupt_all/wake :1103,1806,1824,1861；activity 端点服务 in-memory ring :1125-1135）、`/Users/wdx/opc/flockmux-core/crates/flockmux-storage/src/store.rs`（messages kind+meta :1070；last_activity_at/touch :438-444）
- 上位方案对照：`.ux-review/final-redesign.md`（在 main 工作树，非本分支）
