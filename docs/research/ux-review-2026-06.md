# swarmx 逐元素审查报告 — 蜂群 / 聊天 / 黑板 / 上下文

> 日期：2026-06-06　方法：挑剔 reviewer 视角，**对着真实运行的产品**（release server :7777，真机触发真实 orchestrator + codex worker 端到端）逐元素走查，并以业界最佳实践为标尺（Cursor 2.0/3、Devin、Manus、Magentic-UI、Claude Code Agent view、Linear、Figma、Temporal/Airflow、MUI X）。代码层证据由三个并行研究 agent 提供并经现实交叉验证。
>
> 严重度：🔴 阻断/误导　🟡 体验缺陷　🟢 增强机会（产品方向）

---

## 0. 一句话结论

swarmx 的**任务台账 + 黑板 + 协作图 + typed handoff key** 这套"结构化协作底座"，正是 2025-2026 业界（Cursor 100-agents、OpenAI Symphony）用真实数据验证过的最大杠杆（issue-tracker 当 agent 控制平面 → OpenAI 内部 +500% landed PR）。**底座已是产品护城河，但 UI 把它当只读进度条用了**。最高价值的方向不是加功能，而是把这套已有的底座在 UI 上"做成一等公民、可人机共写"。

同时，**逐元素审出一批真实可用性问题**，其中一条（通知里 manual wake 显示成空白行）是上一次 commit `f5d4dd4` 我自己引入的回归 —— 一并诚实列出。

---

## 1. 站在巨人肩膀上：swarmx 已经做对的（先肯定）

现实走查中验证为**符合或领先最佳实践**的点，不要在重构中弄丢：

| 已做对 | 现实证据 | 对应最佳实践 |
|---|---|---|
| 任务台账内容结构 | 任务台账含 Facts/Guesses/Acceptance/**Plan(DAG, checkbox)**；进展状态含 Status/Assignments(owner+产出)/Blockers | OpenAI Symphony：claim/owner/status/blocker/handoff state 状态机 |
| typed handoff key | DAG 节点详情 `…/main/backend.done`，依赖图 spawn 时校验 | OpenAI AgentKit typed edges（带 schema 的连线） |
| 工具级实时活动 | 成员栏实时显示 "Bash ls -la …" / "swarm_list_agents" + 计时 | Manus "Manus's Computer"、Claude Code Agent view |
| 自动上线 | 空闲 workspace 发消息 → orchestrator 自动复活，placeholder 动态切换 | — |
| 协作图 DAG | 正确画 orchestrator→worker 派生边 + 三类边图例 + 角色 filter + URL 状态持久化 + 节点详情面板 | Airflow/GitHub Actions Graph view |
| 富内容安全渲染 | 代码块 sandbox iframe（CSP 锁死）/ SVG data-url / mermaid 懒加载 strict | Claude Artifacts 安全模型 |
| composer 「优化输入」+ undo | 真机验证 disabled→enabled 正确，`pointer-events:none` 真正不可点 | — |
| 未读 Slack 风格单分隔线 | 非"逐条彩色边框"（呼应去 AI-slop） | NN/g 指示器克制原则 |

> 现实验证的副产品：研究 agent 推测的"composer disabled 按钮 hover 仍像可点"(#9) **被现实推翻** —— 实测 computed `pointer-events:none`，真正点不了。这正是"对着现实审"的意义。

---

## 2. 🔴/🟡 现实验证的 UI/UX 问题（高置信，我亲自在浏览器复现）

### 2.1 通知 / 身份呈现

**N1 🔴 manual wake 显示成空白"系统"通知（我引入的回归）**
- 现实：通知 popover 出现一条只有"系统 + 1m前"、**无任何正文**的项。
- 根因（真实数据 id=487）：该消息 `kind:"wake", reason:"manual"`。`isHiddenWake` 特意**保留** manual wake（人为介入留痕），但上次 commit `f5d4dd4` 我加的 `notifBody` 无差别 `if(kind==="wake") return undefined` 把它正文清空了。能显示出来的 wake 只可能是 manual ⇒ 这条 drop 专打它。
- 建议：`notifBody` 不再无差别 drop wake；对 manual wake 保留正文，或把标题升级成"操作员唤醒了 {role}"。`web/src/lib/notif.ts:36-44`。

**N2 🟡 通知 popover 暴露内部 agent hash，且与全页 /notifications 不一致**
- 现实：popover 显示"orchestrator 5dc3e8b0 / 00926071 / d740c7ed"（同角色不同实例 hash）；全页 /notifications 显示纯"orchestrator"。
- 根因：popover 走 `<AgentChip>` 默认带 short-id（未传 `showId={false}`），而全页用本地 `friendlyAgent()` 去 id。`NotificationPopover.tsx:290-301`。
- 最佳实践：Slack/Linear 显示角色/人名，内部 id 仅 hover/详情。通知场景下一串不同 hash 的同名 orchestrator 是纯噪声。
- 建议：popover `AgentChip` 加 `showId={false}`，与全页统一。

### 2.2 蜂群 / 协作图

**S1 🟡 活动行 ✓/✗ 图标语义不清**
- 现实：orchestrator 活动行先后出现 "✓ swarm_list_agents" 和 "**✗** Bash ls -la …"。✗（叉）极易读成"失败"，但更可能是"进行中/未完成"。无图例。
- 最佳实践（状态图标统一语义）：用进度态符号（spinner/·）而非"✗"表示进行中；失败才用红 ✗。
- 建议：核对 `formatActivityLine`（`Chat.tsx:668-706`）的符号语义，区分 进行中/成功/失败 三态并加色。

**S2 🟡 DAG / 节点详情的 handoff key 未 humanize（与通知页不一致）**
- 现实：DAG 节点 "→ c288f8742eca0aa4b4d178a65…"，详情面板 handoff `c288f8742eca0aa4b4d178a6596e76b9/main/backend.done` —— 前 32-hex 是 workspace id，用户读不懂。
- 对照：通知页已用 `humanizeBlackboard` 渲染成"工作空间 · 方向"。DAG 没复用 ⇒ 不一致。
- 建议：DAG 复用 `humanizeBlackboard`，显示"add review · backend.done"。

**S3 🟡 节点/handoff 的状态颜色语义易误读**
- 现实：worker 状态 READY（还没写 backend.done），但其 handoff key 用**绿色**显示；而图例里绿色=「已满足」。节点状态仅靠右上角一个绿点，无 running/done/error 边框色。
- 最佳实践（Airflow/GitHub 状态色）：灰=pending、黄=running、绿=done、红=failed，且节点边框即状态。绿色复用给"produces key"会与"已满足"撞义。
- 建议：handoff key 未满足时中性色、满足后才绿；节点边框按 running/done/error 上色。

**S4 🟢 缺"计划态 vs 执行态"分离（Temporal 最大教训）**
- 现实：DAG 只画**已 spawn** 的 agent（执行态），无 orchestrator 规划的完整依赖全貌（计划态，含未开始）。
- 建议：一张计划全貌图 + 一张执行进度图叠加；并在边上标 typed 契约（produces→consumes kind）。

**S5 🔴 worker「hang 不退出」是兜底盲区；但「退出」有优秀的 .error fallback + 自愈（真机复现 + kill 验证）**
- 现实①（盲区）：codex worker spawn 后 `last_activity_at:null` 持续 10+ 分钟，**进程没退出**（`shim_ready:true, killed_at:null, shim_exit:null`）。这种"活着但 hang"态**不触发任何兜底**——无"无活动超时→主动 kill/error"，orchestrator 永等 `backend.done`，UI 标 🟡 无响应但整条编排静默 stuck，只能人工干预。
- 现实②（kill 验证，工作优秀）：一旦 worker **真正退出/被 kill**，`.error` fallback 链路完整且超预期 —— 手动 kill 后 **1ms 内** server 写 `…/main/backend.done.error`（原因 "agent exited without writing its handoff signal"）→ wake orchestrator（两条 wake，见下方"重复 wake"小注）→ orchestrator **不仅汇报用户，还自动重派了一个改进 prompt 的新 worker 自愈**。
- **结论修正**（推翻初稿"死亡无兜底"的推断）：兜底盲区**不是"死亡无 fallback"**（死亡有，且带 orchestrator 自愈重试），**而是"hang 不退出"这一态缺探活/超时**，没把它转成"退出"去触发那条已经很完善的 fallback。
- 建议：给 worker 加**无活动探活超时**（`last_activity_at` 超 N 分钟仍为 null/不变 → 主动 kill），让 hang 态落入现成的 `.error` fallback + 自愈路径即可，无需新建一套。
- 小注：kill 触发了 **两条几乎相同的 wake**（id 489/490，body 都是 `backend.done.error`）给同一 orchestrator —— 轻微重复唤醒，呼应后端 W3/lag-rewake，无害但可去重。
- 命名校正：fallback 实际写的是 **`<signal>.error`（`backend.done.error`）**，与 memory `project_m6c_error_fallback_design` 记的"写 `<role>.error`"不符 —— 以现实为准，相关 memory 已更新。

### 2.3 上下文 / 工作台账

**L1 🟡 台账渲染扁平：section 与 Status 无视觉层级 / 无状态色**
- 现实：Facts / Guesses / Acceptance / Plan 像普通段落；"Status: dispatched"、"Blockers: —" 纯文本无 badge。
- 最佳实践（Linear 视觉分级 + 状态色）：section 标题分级，Status 用 dispatched/running/done/blocked 彩色 badge。
- 建议：台账 markdown 复用 `ChatMarkdown` 的 rehype-highlight + 给 Status/Blockers 结构化 badge（`Ledger.tsx:349` 当前仅 remarkGfm）。

**L2 🟡「近况」时效性误导**
- 现实：聊天右栏与台账"近况"都显示 "reviewer 22:47 写入最终 review · **43h 前**"，并标"1个worker"；而当前真正在跑的 backend worker 尚无心跳却不在列。
- 最佳实践（Figma presence：空闲淡出 + 时效窗）："近况/活动流"应有时间窗，43h 前不该占据"近况"首位。
- 建议：近况按时间窗过滤/降权，区分"在场 worker"与"历史心跳"。

**L3 🟢 台账只读，未成为"人机共写控制平面"（最高价值）**
- 现实：台账/Plan 是 orchestrator 自由写的 markdown，人在 UI 上**只能看**，不能勾选 checkbox / 认领 / 改 plan。
- 最佳实践（Cursor/Symphony 最强信号）：把 task ledger 做成人+agent 共同读写的状态机 = 比换模型更有效的杠杆。
- 建议：Plan 结构化为可交互（人可勾选/认领/加步骤/标 blocker），写回黑板触发 wake。这是产品护城河的兑现点。

### 2.4 聊天 / 列表

**C1 🟡 列表项裸数字徽章语义歧义**
- 现实：左栏 workspace 行尾裸数字"0/1/2"实为 `member_count`（AI 成员数），但顶部又有"3 未读"也是数字徽章。
- 最佳实践（Slack/Discord/Linear）：列表项旁的数字徽章 = 未读数，是用户的强心智。这里用作成员数会被误读。
- 建议：成员数加图标/标签（如 👥2）与未读区分，或移除裸数字。

**C2 🟡 多方对话缺"参与者身份视觉系统"**
- 现实：人/orchestrator/各 worker-role 的消息靠左右对齐 + 浅色气泡区分，但同为左侧的多个 agent 角色之间区分弱（无稳定色 / 角色徽章）。
- 最佳实践（Discord 多方 + bricxlabs）：给名字着色（非整气泡）、稳定参与者色、角色徽章（人/orchestrator/worker-role）。
- 建议：建立参与者色板 + 角色徽章，连续消息分组头像折叠（MUI X ChatMessageGroup）。

**C3 🟢 空态文案对有历史的 workspace 误导**
- 现实：重启后成员栏"该工作空间还没有 AI 成员"——但该 ws 有大量历史 agent（发消息后立即变正常）。
- 建议：区分"从未有过成员" vs "当前无在场成员（历史见录像/近况）"。

---

## 3. 代码层 findings（研究 agent 提供，标注置信度）

> 这些来自读代码，**未全部现实复现**；列为工程债 backlog，重要者建议先写回归测试再断言。

### 3.1 前端（置信度：中，需现实/单测验证）
- 性能：消息列表 `buildRows` 全量重算（`MessagesPanel.tsx:433`）、pending responders O(agents×messages) 每 5s 重算（:465-499）、图片串行上传（:625-642）、`rowRefs` Map 过滤后不清理可能累积（:232-235）。
- 正确性（**待验证，可能与现实不符**）：unread 跨方向计数是否串房间（`useWorkspaceShellData.ts:141-157`）、`agentStateById[id]` 可能 undefined 的 NPE（`Chat.tsx:600-612`）、`stripLedgerHeading` 正则偏宽（`Ledger.tsx:55-57`）。
- a11y：pending 气泡/lightbox 缺 aria-label；消息/成员列表缺键盘导航。
- 缺虚拟化：大消息列表/大 DAG 无虚拟滚动（Slack/Discord 标配）。

### 3.2 后端（置信度：高，多数有代码证据；注意很多历史坑已修）
- 🔴 `pty kill()` 不回收孙进程（`pty/lib.rs:164-171` 注释与实现不符）；boot orphan-settle 只改 DB 不杀 OS 进程（`main.rs:139`）。
- 🔴 WS 无 Origin 白名单 / session token（安全）。
- 🟡 多 CLI 抽象半成品：`mcp_inject`/`ready_detect` manifest 字段零读取（`plugins.rs:19-23`），实际派发仍 `match id`（`pre_spawn.rs:544`）；加第 3 个 CLI 需改 Rust 而非配置。
- 🟡 单个坏 `cli-plugins/*.toml` 拖垮启动（`plugins.rs:102-106` 用 `?`，而 `roles.rs:109-122` 是 warn+skip，韧性不一致）。
- 🟡 depends-on 生产者中途死亡/不 spawn → 下游永久挂起（无 blocked 状态/超时，P1-D 延期）。
- 🟢 多 kind 的 `.error` fan-out 仅单 kind；orphaned wake 诊断可加"当前订阅集"信息。
- ✅ 已修勿动：F6 blackboard 非原子（已加 liveness+boot reconcile）、F12 lag 后 rewake、F13 auto-kill writer 守卫、F17 flag 探测超时。

---

## 4. 对照最佳实践的"可直接照搬"清单（按价值排序）

1. **【最高】台账→人机共写控制平面**：Plan 可勾选/认领/加步骤/标 blocker，写回黑板触发 wake。（Cursor/Symphony）
2. **【高】Magentic-UI HITL 三件套**：可编辑 plan + Accept 才执行；可折叠 step banner（完成即折叠减噪）；action guards（不可逆动作才弹审批）。原则"尽量少打断人"。
3. **【高】多方聊天身份视觉系统**：稳定参与者色 + 名字着色（非整气泡）+ 角色徽章 + 连续消息分组头像折叠。（Discord + MUI X）
4. **【高】DAG 两图分离 + 统一状态色 + key humanize + 事件组折叠**：计划态/执行态分离；灰/黄/绿/红；handoff key 复用 humanize；高频活动折成 span。（Temporal/Airflow）
5. **【中】整体走 Linear 视觉路线**（信息密集但平静：调暗次要元素、减分隔线、少强色）+ Figma ephemeral 在场感（黑板读写高亮、跟随某 worker、空闲淡出、隐藏开关）。同时解决"去 AI-slop"。
6. **【中】全屏 agent 控制台**：一屏看每个 worker 在干什么/是否等输入/是否完成 + token 用量，"窥视不打断"。（Claude Code Agent view）
7. **【中】可回放工具级时间轴**：swarmx 已读会话 JSONL，升级成可 step-through 回放。（Manus/Devin）

---

## 5. 端到端真机验证（核心流程闭环）

发"派 backend worker 写 greet.py+pytest"→ 现实观察：
1. ✅ orchestrator 自动复活（member 0→1）；
2. ✅ 实时活动行显示工具级进度；
3. ✅ orchestrator spawn codex worker（Backend Engineer 6fc9b645）；
4. ✅ DAG 出现派生边 + 节点详情（handoff backend.done / depends_on —）；
5. ✅ 台账实时更新（Plan: backend 进行中 / Status: dispatched / Assignments owner）；
6. ❌ worker done → 汇报闭环 **未达成**：codex worker spawn 后 10+ 分钟 `last_activity_at:null`（进程 shim_ready，但从未产生任何工具活动），UI 正确标 🟡「无响应」；orchestrator 已 idle，永久等待 `backend.done`，无超时兜底。

**闭环结论**：happy-path 的**可观测性全链路打通**（自动上线 / 实时活动 / spawn / DAG / 台账实时更新均现实验证通过）；但撞上真实的 **codex worker 卡死（从未活动）** 边界 —— 同时暴露 swarmx 可靠性短板（S5）与 codex 集成脆弱（呼应历史 m5b codex trust gate）。**"对着现实从头跑一遍"恰好逼出 happy-path 测不到的故障路径。**

---

## 6. 优先级总表

| 优先级 | 项 | 类型 | 位置 |
|---|---|---|---|
| P0 | N1 manual wake 空白通知（回归） | 修 bug | notif.ts |
| P0 | N2 popover hash 暴露/不一致 | 修 bug | NotificationPopover.tsx |
| P0 | 后端 kill 不回收进程组 / WS 鉴权 | 安全 | pty, ws |
| P0 | S5 卡死 worker 无超时兜底（编排 stuck） | 可靠性 | wake.rs, Ledger/Chat |
| P1 | S2 DAG key humanize | 一致性 | Dag |
| P1 | S1 活动行 ✗ 语义 / S3 状态色 | 可读性 | Chat.tsx, Dag |
| P1 | L1 台账层级/状态色 · L2 近况时效 | 体验 | Ledger.tsx |
| P1 | C1 裸数字徽章 · C2 参与者身份视觉 | 体验 | sidebar, MessagesPanel |
| P2 | L3 台账可交互控制平面 | 产品方向 | Ledger + 后端 |
| P2 | HITL 三件套 / DAG 双图 / Linear 视觉 | 产品方向 | 多处 |
| P3 | 前端性能/虚拟化/a11y · 后端多 CLI 抽象/blocked 状态 | 工程债 | 见 §3 |
