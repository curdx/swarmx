I have the full north-star context now. The synthesis is large but well-scoped — I have all six specs plus the binding map and the north-star. I'll now produce the consolidated master document. I don't need to read more files; the load-bearing facts (file:line anchors, tokens, i18n conventions, persistence states) are all present in the inputs and verified against the north-star.

Let me write the master spec directly as my return value.

---

# swarmx 聊天窗口重设计 · 实现级设计规格总册

> 读者：接手写代码的工程师。本册把 6 个组件区详规去重整合为单一可落地真相源，消解相互矛盾处（裁决见各处「⚖裁决」），并补两样跨组件产物：统一设计 token/基元（§2）与 i18n 文案册（§4）。所有路径为绝对路径。术语守表，用户可见文案禁行话（mailbox/blackboard/wake/worktree/shim/spell/PTY/handoff/dispatch/orchestrator/agent），统一用：队长/成员/会话/计划/变更/推进/启动/隔离/引擎/分支/采纳/已看。
> 对齐北极星：`/Users/wdx/opc/swarmx-core/.claude/worktrees/funny-raman-47113f/.ux-review/chat-redesign.md`（§2 七原则、§3 整屏布局、§4 逐组件、§6 P0/P1/P2 分期）。

---

## 1. 总览

本册描述聊天窗口从「会被诊断为撒谎/firehose/概念过载」改造为「单声道诚实主轴 + 渐进披露进阶层」的完整前端实现规格。骨架是 conversation-first 的单声道主轴（你 / 队长 / 少数真发生了的系统卡），右面板是 cockpit 的工作面板语义但**默认收为脉搏条**，diff 评审借 thread 的「合并闸」收口动作。核心纪律是七原则中的**事实律**（首个真实输出前绝不出现绿色）与**入流律**（状态变更以可重放系统卡进流）。P0 的硬约束是「完全不动后端、只接现有信号 + 前端诚实化」——后端 `AgentState` 七态、90s 看门狗、`last_error*`、`last_activity_at`、interrupt/resume 端点、`messages.kind+meta` 框架**全部已存在**，P0 只是把前端接上去。

### 组件清单表

| 组件 | 复用现有 | 新建 | 依赖后端新增 | 所属期 |
|---|---|---|---|---|
| `classifyRow` 渲染物判定器 | — | ✅ `lib/messageClassify.ts` | 否 | P0 |
| USER 气泡（改：引用替代 reply、限宽 720） | ✅ `MessagesPanel.tsx:1211` | — | 否 | P0 |
| ORCH 队长气泡（改：提对比度、思考块降级折叠） | ✅ `MessagesPanel.tsx:1278` + `ReasoningDisclosure:1606` | — | 否 | P0 |
| `CaptainTypingRow` 队长 typing 行 | ✅ 从 `PendingBubble:1670` 抽出 | ✅ `components/messages/CaptainTypingRow.tsx` | 部分（`thought_trace.status` enum，缺则降级） | P0 |
| `MemberHeartbeatRow` 成员 two-signal 活态行 | ✅ `resolveMemberVisual` `lib/agent.ts:265` | ✅ `components/messages/MemberHeartbeatRow.tsx` | 否（45s 降级前端算） | P0 |
| `activityVerb` 动词翻译器 | — | ✅ `lib/activityVerb.ts` | 否 | P0 |
| `SystemCard` 统一容器 + 派发器 | ✅ 泛化 `OrchestratorFailureCard.tsx` | ✅ `components/chat/SystemCard.tsx` | 否（容器）/ 是（各 subtype 落库） | P0 容器 / P1 数据 |
| `FailureCard`（泛化失败卡，5 kind × 4 键） | ✅ `OrchestratorFailureCard.tsx` | — | 否（`last_error*` 已持久）/ 入流卡 P1 | P0 |
| `EmptyState`（问候 + 3 starter + 引擎预检） | ✅ `cliReadiness` `Chat.tsx:200` | ✅ `components/chat/EmptyState.tsx` | 否 | P0 |
| `BootstrapChecklistCard` 启动清单卡 | ✅ `TaskActivity.tsx` 计时 tick | ✅ `components/chat/BootstrapChecklistCard.tsx` | 否（P0 时间戳推断）/ 阶段链 P1 | P0 骨架 / P1 精确 |
| `useChatLifecycleState` 空/启动/失败状态机 | ✅ 收编 `orchestratorFailure` memo | ✅ `hooks/useChatLifecycleState.ts` | 否 | P0 |
| Composer 打断/排队两态 | ✅ `MessagesPanel.tsx:988` + interrupt 端点 | ✅ `InterruptConfirmBar`、`StopMenu` | 否（P0）/ pending 持久化 P1 | P0 |
| Composer 模型切换确认 + 系统卡 | ✅ `ModelPicker.tsx` | ✅ `ModelChangeConfirm` 包装 | 是（model_changed emit） | P0 确认 / P1 卡 |
| Composer 附件失败回滚 | ✅ `uploadAttachment` `http.ts:361` | ✅ per-file 上传状态机 | 否 | P0 |
| `WorkPanel` 右面板三态容器（脉搏/乐队/焦点） | ✅ `AgentDrawer.tsx` 五 tab | ✅ `WorkPanel/PulseRail/BandView/FocusView` | 否（容器）/ activity DB P1 | P0 容器 / P1 回填 |
| `DispatchCard` 派工卡 + 小团队树 | ✅ `dagEdgeDerivation.ts`、`AgentActivityLog.tsx` | ✅ `DispatchCard`、`InlineTeamTree` | 是（dispatch 落库、activity DB） | P1 |
| `DeliveryCard` 交付卡 | ✅ `Disclosure` 折叠块 | ✅ `components/review/DeliveryCard.tsx` | 是（delivery 落库 + per-file diff） | P2 |
| `PlanStickyCard` 计划置顶卡 | ✅ 台账 markdown `Ledger.tsx:67` | ✅ `PlanStickyCard`、`lib/parsePlan.ts` | 部分（P1 前端 parser / 结构化 P2） | P1 |
| `ApprovalCard` 审批卡 | ✅ `SystemCard` 容器 | ✅ `components/chat/cards/ApprovalCard.tsx` | 是（消息 + APPROVE/DENY 端点 + 状态机） | P2 |
| `ChangeReviewPanel` 变更评审 + `MergeGate` | ✅ `threadDiff` `http.ts:321`、`mergeThread` | ✅ `ChangeReviewPanel`、`MergeGate`、`VirtualFileList`、`LineCommentDraft` | 是（hunk 端点、review_comments、file_accept、test_runs） | P2 |
| `MemberSubThread` @定向子线程 | ✅ `@autocomplete` `MessagesPanel.tsx:728` | ✅ `components/chat/MemberSubThread.tsx` | 是（member_mention/handoff subtype） | P1 |
| `useAutoMarkRead` 进视口标记已读 | ✅ `markMessagesRead:455` | ✅ `hooks/useAutoMarkRead.ts` | 否（精确未读端点 P1） | P0 前端 / P1 端点 |

---

## 2. 统一设计 token 与基元

> 抽出跨组件复用的视觉基元，避免每个组件各写一套。全部用 `/Users/wdx/opc/swarmx-core/web/src/styles/global.css` 现有 CSS 变量，**不新增颜色**。

### 2.1 状态徽章五态（+ 两个终态）— 全局唯一真相

所有「态点 / 健康度点 / chip 徽章 / 清单步骤 glyph」必须从这张表取，不得各自发明。**核心纪律：首个真实输出前绝不用绿。**

| 态 | glyph | 文案 token | 色 token | 用处 |
|---|---|---|---|---|
| 启动中 | `◐`（半填） | `state.starting`「启动中」 | `--color-state-wake` #06B6D4（青，**非绿**） | bootstrap、`!shim_ready`、首卡占位 |
| 进行中 | 脉冲点 / `●` | `state.running`「进行中」 | 角色色 + 脉冲；中性进行用 `--color-state-success` | typing、派工卡、live 行 |
| 等依赖 | `◌` | `state.waiting`「等依赖」 | `--color-state-idle` #94A3B8 | `waiting_dep` |
| 可能卡住（降级） | `⚠` | `state.stalled`「可能卡住」 | `--color-state-warning` #F59E0B / 文字 `--color-status-warning` #B45309 | 45s 软降级、300s 硬 stall |
| 待命 / 等你 | `●` 绿 | `state.idle`「待命」 | `--color-state-success` | semantic idle/awaiting_user |
| 失败（终态） | `✕` | `state.error`「卡住」 | `--color-status-danger` #B91C1C（`-soft` #FEE2E2 底） | error/last_error，**置顶** |
| 已结束（终态） | 灰点 | `state.exited`「已结束」 | `--color-state-idle` | killed_at/shim_exit |
| 状态未知（中性） | `·` | `state.unknown`「状态未知」 | `--color-foreground-tertiary` | 计划项不可判定、测试未运行——**故意区别于 ✓/✕，杜绝假绿** |

判定一律走 `resolveMemberVisual(agent, live, messages, labels, now)`（`lib/agent.ts:265-358`），**不得重复实现启发式**。

### 2.2 成员色身份（原则 7 稳定身份）

| 基元 | 来源 | 用处 |
|---|---|---|
| `roleColorClass(role)` / `roleColorHex(role)` / `roleInitial` / `resolveRole` | `lib/agent.ts:42-68` | 头像底色、左色条、chip 边、子线程细条、diff 归属 |
| `resolveMemberVisual` | `lib/agent.ts:265` | 态点/typing/isError/降级的整套诚实层 |
| `activityVerb(label)` | **新建** `lib/activityVerb.ts` | `AgentActivity.label` → 用户动词，strip 隔离路径前缀 |

**铁律**：同一成员在 typing 行 / 活态行 / 派工卡左色条 / 小团队树节点 / 交付卡 / diff 归属 / 脉搏点 / 乐队 chip / `@`autocomplete / `■停` 处用**同一色同一名**，全程禁裸 `worker_7`。

### 2.3 消息容器基元

| 基元 | 规格 |
|---|---|
| 主轴限宽 | `max-w-[720px]` 居中（⚖裁决见 §3.0） |
| USER 气泡 | `max-w-[min(82%,780px)]`，`rounded-2xl` + `rounded-br-sm` 尾角，`bg-accent-primary` / `text-foreground-on-accent`，右对齐无头像 |
| ORCH 气泡 | `max-w-[min(82%,820px)]`，`rounded-2xl` + `rounded-bl-sm`，`bg-surface-secondary` / `border-border-subtle`，正文 **`text-foreground-primary`**（提对比，治诊断 1） |
| 头像 gutter | `w-8`（28–32px） |
| 组间距 | header 行 `mt-3`，同发送方续行 `mt-2`（复用 `buildRows` `MessagesPanel.tsx:219`） |
| 正文 | `font-body` 13px / `leading-snug`；markdown 走 `.prose-chat`（13px / 1.6） |
| 时间戳 | `font-caption` 10px / `tabular-nums` / `text-foreground-tertiary` |

### 2.4 系统事件卡基类（`SystemCard` 统一语法）

所有系统卡共用同一容器壳，按 `meta.subtype` 派发子组件。

```
collapsed (默认, 卡头 min-h-8):
▏[⬡徽章] 一行原因文案                              02:57   ▾
 └ 左色条 3px = subtype 语义色

expanded (▾→▴):
▏[⬡徽章] 一行原因文案                              02:57   ▴
▏ ──────────── hairline (border-subtle) ────────────
▏  卡专属正文区: 时间线 / 文件列表 / 三要素 / 小团队树
▏ ──────────────────────────────────────────────────
▏  [行动按钮A]  [行动按钮B]  [次要链接]            (min-h-8)
```

**语法规则（不可违反）**：
- `[⬡徽章]` = 事件类型徽章（菱形/方形，**非圆头像**），避免误读为「某人说话」。
- 系统卡**不右对齐、不进 28px 头像 gutter**，占满主轴宽（居中限宽内）。
- 容器 `rounded-xl`（比气泡略方，区别形状），左色条 `border-l-[3px]`。
- 未知 subtype → 降级为中性单行卡，**不崩**。
- `title` attr 带 `#id · subtype · 全时间戳`，可重放。
- **审批卡 / 失败卡默认 `expanded`**（永不消失 + 必须可操作）；其余默认 `collapsed`。
- 展开态懒加载重数据（派工活动时间线、交付 diff、变更 hunk）：仅展开时打端点。

左色条语义色：派工=成员色 / 交付=成员色 / 失败=`--color-status-danger` / 审批=`--color-status-warning` / 模型·回退·隔离=`--color-state-info`（信息蓝）/ 启动中=`--color-state-wake`。

### 2.5 心跳行基元（two-signal 活态行）

```
┌测┐ 测试成员 正在 写 refund.test.ts   ⋯   14s · 上次输出 2s前
└──┘ 名13/600  动词(activityVerb)   三点  两个会动的数字
 size-7 角色色                       动画   计时 · 文件数/上次输出
```
- 行高 `py-1.5`（≈28–44px），头像 `size-7`（活态行）/ `size-8`（typing 行）。
- 计时：running 用 `now − activity.at`，settled 用 `duration_ms`，500ms tick（`MessagesPanel.tsx:1682` 模式）。
- 降级：`now − last_activity_at > 45s` → `⚠ 已 Ns 无活动 · 上次输出 Ns前`，**停三点动画**（不假装在跑）。
- 死即移除：`AgentState ∈ {error,exited}` 或 `killed_at`/`shim_exit` → 行卸载（≤1 渲染帧 / ≤300ms 防抖，**不挂 60s**）。
- `role="status" aria-live="polite"`，降级/异常带 `⚠`/`✕` 字符 + 文字（色盲可读），不只用颜色。

### 2.6 空 / 失败卡基类

| 基类 | 规格 |
|---|---|
| 空态容器 | 无边框，`max-w-[460px] mx-auto mt-16 flex flex-col gap-6`；**永不显裸「暂无消息」**——空但失败显失败卡，空且健康显能力揭示 |
| 加载态 | 骨架占位（灰条 / 骨架点），**永不显绿点或假数字** |
| 失败卡容器 | `border border-status-danger/30 bg-status-danger/5`，`rounded-2xl`，`px-5 py-4 gap-3`，`max-w-[460px] mx-auto`；reason 容器**无 `truncate`/`line-clamp`/固定高溢出**（防截断硬约束） |
| 启动清单卡容器 | `border border-accent-primary/30 bg-accent-primary-soft`，`rounded-xl`，徽章用 `--color-state-wake`/`--color-status-busy`（**非绿**） |
| 行动按钮 | 复用 `Button` `size="sm" h-8 gap-1.5`；主=实底，次=`outline`，三+=`ghost`；`min-h-8` 触达 |

### 2.7 折叠块基元

`ReasoningDisclosure`（`MessagesPanel.tsx:1606`）提取为通用 `Disclosure`，默认收起。复用于：队长思考块（`status==='done'` 默认收起 + `bg-surface-primary/70` 降对比）、交付卡测试输出、变更 hunk、失败详情。

### 2.8 阈值汇总（全局统一，避免各组件各定）

| 行为 | 阈值 | 来源 |
|---|---|---|
| 时间分隔 | `GROUP_GAP_MS = 5min` | `MessagesPanel.tsx:140` |
| typing 行出现 | ≤ 1–2s（前端乐观点亮，不等 WS） | 设计 |
| 活态行软降级（灰提示） | 45s（`HEARTBEAT_STALE_MS`，**新建独立常量**，文案级） | ⚖见下 |
| 硬 stall（红/黄判定） | 300s（`STALL_RUNNING_MS`/`NO_RESPONSE_MS`，保留不动） | `lib/agent.ts:126-128` |
| 启动宽限期 | 45s（`STARTUP_GRACE_MS`） | `lib/agent.ts:127` |
| 看门狗失败翻转 | 90s（`FIRST_RESPONSE_WATCHDOG_MS`） | `rest.rs:33` |
| typing 行慢提示 | 60s「响应较慢」灰字（仍 `◐`，不翻失败） | 设计 |
| auto-mark-read 去抖 | 视口停留 ≥ 800ms + ≥50% 可见 | 设计 |
| @子线程收拢 | 60s 无新 @ 或队长接管 | 设计 |
| 计时 tick | 500ms（行）/ 1000ms（时间线） | 复用 |
| 死即移除防抖 | 300ms（避免 error→recovered 抖动） | 设计 |
| 自动重试封顶 | 2–3 次后翻「需要你/换引擎」 | 设计 |
| pending chip 存活上限 | 60s（`PENDING_TIMEOUT_MS`），AgentState 死优先移除 | `MessagesPanel.tsx:698` |

**⚖裁决（45s vs 300s 矛盾）**：三份详规对「无活动降级」阈值不一致——§4.2 详规要求 45s 降级，但 `resolveMemberVisual` 现状是 300s（`STALL_RUNNING_MS`/`NO_RESPONSE_MS`）。裁决：**新建独立 `HEARTBEAT_STALE_MS=45_000` 只做文案级软降级（灰字 + 停三点），不改 `resolveMemberVisual` 的红/黄硬判定（保留 300s）**。理由：45s 直接改硬判会误报红（很多工具单步合理耗时 >45s），软/硬两阈分离既满足「45s 诚实提示慢」又不误判失败。两份详规（§4.2、右面板）均独立提出此分离，采纳为统一裁决。

---

## 3. 6 个组件区详规

### 3.0 整屏布局与三处全局裁决

**布局**（北极星 §3）：1280px 默认 = 会话栏 + 对话主轴（max-w 720 居中）+ 右侧 54px 脉搏条；1536+ = 右面板可常驻（`clamp(360px,26vw,440px)`），对话仍居中限宽不拉全宽。

**⚖裁决 A（主轴限宽 720 vs 1040）**：消息流详规要求收窄到 `max-w-[720px]`（现状 `MessagesPanel.tsx:1156` 是 1040）。**裁决：720**。理由：1040 导致两条短消息贴顶留大空白（诊断 5）且行长伤可读；右面板三态详规、北极星 §3 线框均以 ≈720 为准。统一为 720。

**⚖裁决 B（脉搏条宽度 54 vs 64）**：北极星 §3 文案写 64px，右面板详规写 54px。**裁决：54px**。理由：右面板详规是该组件的权威细化，54px 容纳约 6 行 36px 态点 + 召出钮，已据此定溢出折叠（>6 成员 +N）。北极星 64px 是粗略示意。

**⚖裁决 C（系统卡命名空间）**：消息流详规用 `chat.systemCard.*`，「现在发生什么」详规用 `messages.dispatch.*`/`messages.live.*`，变更评审详规用 `chat.review.*`。**裁决：统一为 `chat.*` 根，分 `chat.systemCard.*`（卡内通用）/ `chat.live.*`（typing/心跳）/ `chat.dispatch.*`（派工）/ `chat.delivery.*`/`chat.review.*`/`chat.bootstrap.*`/`chat.emptyState.*`/`chat.verb.*`（动词）/`chat.activityVerb.*`。** 理由：三份详规各起一套命名空间会分裂；`chat.*` 与现有 `chat.orchestratorFailure.*` 一致。**动词表统一为 `chat.verb.*`**（两份详规分别叫 `chat.activityVerb.*` 和 `messages.verb.*`，合并为 `chat.verb.*`）。

---

### 3.1 消息流 + 4 类渲染物 + 系统事件卡语法

**渲染物判定**（`classifyRow(m)` 唯一判定，落 `lib/messageClassify.ts`）：

| 类 | 判定 | 形态 |
|---|---|---|
| `USER` | `from_agent==="user"` | 右对齐 accent 实底气泡（§2.3） |
| `ORCH` | `from_agent===orchestratorId && kind!=="system"` | 左对齐强对比正文 + 折叠思考块 |
| `MEMBER_BUBBLE` | 成员发出 + 满足升格条件 | 成员色独立气泡 |
| `SYSTEM_CARD` | `kind==="system"` | 系统事件卡（§2.4） |
| `MEMBER_SUPPRESSED` | 成员发出但**不满足升格** | **不渲染气泡**；信号只走派工卡时间线 / 右面板 |
| `WAKE_FILTERED` | `kind==="wake" && meta.reason==="blackboard"` | **不渲染**（复用 `MessagesPanel.tsx:528` 过滤）、不计未读 |

**成员气泡升格条件（物理隔离闸，治诊断 4）**——默认 `MEMBER_SUPPRESSED`，仅三时刻升格：
1. @定向：`meta.subtype==="member_mention"`（依赖后端）或处于活跃 @子线程窗口。
2. 被队长引用：`orchMsg.in_reply_to===memberMsg.id`（现有字段可推导）。
3. 求助：`meta.subtype==="member_handoff"`（依赖后端）或归一为 `approval_required` 卡。
升格之外的一切成员产物（编辑/读/跑测试）**绝不进流**。

**系统卡 subtype × 状态全枚举**（9 种，逐状态线框见各详规，统一语法见 §2.4）：

| subtype | 专属状态 | 持久化 | 期 |
|---|---|---|---|
| `dispatch` | started/running/done/failed | missing | P1 |
| `delivery` | clean/tests_failed/untested | missing | P2 |
| `plan_ref` | cited | missing | P2 |
| `approval_required` | pending/approved/denied/expired | missing | P2 |
| `agent_error` | auth/rate_limit/watchdog/fatal × unresolved/retrying/resolved | 字段已持久 / 入流卡 missing | P0 字段 / P1 卡 |
| `bootstrap` | spawning/isolated/logged_in/engine_starting/awaiting_first_response/failed | missing | P0 推断 / P1 链 |
| `model_changed` | applied | missing | P1 |
| `engine_fallback` | fell_back | missing | P1 |
| `isolation_degraded` | degraded | missing | P1 |

**ORCH 气泡关键改动**：正文 `text-foreground-primary`（视觉权重强于 USER）；思考块 `status==='done'` 默认收起 + `bg-surface-primary/70` 降对比；空正文边缘 `thought-only` 渲染气泡壳 + 「（思考完成，未给出文字结论）」占位，不留裸空气泡。

**USER 气泡改动**：移除 user→user reply 入口，改「引用任意流内卡片」；hover 露 `[引用]`。

**时间分隔/未读**：复用 `TimeDivider`（`:1571`）、`NewMessagesDivider`（`:1587`）；`buildRows`（`:219`）系统卡也走它（不进头像 gutter 但参与时间分隔）。

**@定向子线程**（`MemberSubThread`）：升格成员气泡缩进 `pl-4` + `border-l-2` 成员色细条；队长重新发言或 60s 无新 @ 即收拢回 `MEMBER_SUPPRESSED`。

**auto-mark-read**（`useAutoMarkRead`）：`IntersectionObserver` 观察未读 ORCH/MEMBER 气泡（`to_agent==='user' && read_at===null`），进视口 ≥50% 停留 ≥800ms → 批量 `markMessagesRead`；窗口失焦不标；系统卡不计未读但可被读。

逐状态线框（USER/ORCH/9 种卡/时间分隔/@子线程）见消息流详规 §3，本册不复制，照其线框实现。

---

### 3.2 「现在发生什么」（typing / 心跳 / 派工展开）

填 doomscrolling gap 三组件，全部 two-signal 诚实降级。

**① 队长 typing 行（`CaptainTypingRow`，A0–A7 八态）**：
- 用户发消息 → **≤2s 必出 A1「队长 正在思考…」**（前端乐观点亮，不等 WS 往返）。
- 文案随 `thought_trace.status` 切：思考(A1)→派工(A2)→合并(A3)→启动(A4，`Spawning` 同步发)→慢(A5，>45s「已思考 Ns…」灰字仍三点不判红)。
- **A6 死亡→移除**：`live.state ∈ {error,exited}` 立即卸载，失败卡接管。
- A2/A3 缺 `thought_trace.status` enum 时降级：用 `live.state` + 成员 spawn/exit 时序推断（仍可上线）。

**② 成员 two-signal 活态行（`MemberHeartbeatRow`，B0–B7 八态）**：直接消费 `resolveMemberVisual` 的 `MemberVisual`（B5 死即移除 = `isError===true` 时不渲染，根因修复无需新逻辑）。线框见 §2.5。B7 冷加载/重连显骨架（不显假数字），回填到达填实。

**③ 派工卡（`DispatchCard`，C0–C6 七态）**：折叠一行（左色条=成员色 + 状态 chip + `▾`）/ 展开 = 小团队树（`InlineTeamTree`，喂 `dagEdgeDerivation` 边）+ 活动时间线（复用 `AgentActivityLog`，每条 label 过 `activityVerb()`）。C0 首卡占位 = spawn 瞬间同步落 `◐启动中`，不等 700ms tailer。

**首卡占位即时落地**：spawn 时同步 emit `SwarmEvent::AgentState{Spawning}`（`rest.rs:670` 已有），前端绑此立即出 typing/占位，堵「盯绿点」空窗。

**断线兜底**：WS 重连先打 `GET /api/agent/:id/activity` 回填再接流——**前提该端点走 DB（P1 硬依赖，治诊断 3）**；未补前显「正在恢复活动…」而非空白。

---

### 3.3 空 / 启动 / 失败状态机（`useChatLifecycleState`）

渲染于 `MessagesPanel` 的 `emptyStateOverride` 插槽（`Chat.tsx:862`），把「仅失败卡 or undefined」升级为三态机。优先级：X0 恢复 > F* 失败 > B* 启动 > E* 空（互斥不变量见空态详规 §2）。

**空态族**：E0 就绪（问候 + 3 starter + 引擎预检全✓）/ E1 无引擎（starter 禁用 + 安装命令 + 复制 + 重新检测）/ E2 预检加载（spinner 非绿）/ X4 部分引擎。starter 点击**填入 Composer 不直接发送**。

**启动态族（B0–B4）**：启动清单卡逐步消费——`✓隔离 → ✓登录 → ◐启动引擎(计时) → ○等首响`，徽章 `◐启动中`（**非绿**）。X1：B4>60s 加「响应较慢」灰字仍 `◐`。计时 = `now − spawned_at`（真实戳）。

**失败态族（F-auth/rate/crash/timeout，永不消失）**：
- 单一真相源：完整 reason 不截断；乐队栏/成员栏只显 `✕` 点 + tooltip，不重复整段。
- 四键：F-auth `[打开终端登录][重试][换引擎][看日志]`；F-rate 无登录命令行；F-timeout 含登录入口（兼顾 watchdog 可能是未登录）。
- X2 重试中（按钮禁用 + `◐ 重试中 N/3`）；X3 达上限升级文案（弱化重试，突出换引擎/登录）。
- X0 恢复守卫：`freshSignalAt > last_error_at`（复用 `Chat.tsx:466`）→ 失败卡消失。
- 90s 看门狗：前端绑 `AgentState::Error(kind=watchdog)` 翻 F-timeout，**不自行计 90s**（避免双源，以后端事件为准）。
- **失败后不自动重试首次**（避免静默 fix-loop），由用户点 `[重试]`。

**FailureCard 泛化**：`OrchestratorFailureCard` → `kind` 从 `auth|generic` 扩到 `auth|rate_limit|crash|timeout|escalated`；按钮 2→4；新增 `retryCount/retryMax/retrying/onSwitchEngine/onViewLog/engineCandidates` props。`[换 X 重试]` 候选取自 `cliReadiness.installed` 非当前 cli，无候选则隐藏。

逐状态线框（E0/E1/B0–B4/F-*/X2/X3）见空态详规 §3。

---

### 3.4 Composer（打断/排队/@/模型/优化/附件/键盘）

锚点 `MessagesPanel.tsx:1437-1565`（渲染）、`:258-1023`（逻辑）。

**收件人轴**：默认队长（`defaultRecipient` orchestrator→scout→first-alive `:719`）/ @定向（成员色「→ 给 X」标签）/ mention-typing（autocomplete `:1456`）/ no-captain-revive（placeholder「发送将启动队长…」）/ dead-end（disabled）。

**提交模式（原则 6 核心）**：
- 桌面 `Enter=排队`（立即 `sendMessage`，UI 呈现 pending chip「已排队·队长接手后送达」，浮在 Composer 上方非流内）；`⌘/Ctrl+Enter=打断`（**先弹 `InterruptConfirmBar` 内联确认**，确认后 `interruptAgent(队长)` → 再 send；无在跑队长时降级普通发送）。
- 移动端 Enter=换行 + 显式发送键，⌘Enter 不绑（`isMobileLike` `:993`）。
- `StopMenu`：`[打断▾]` 列在跑成员（`state ∈ {thinking,spawning,waiting_dep}`）各 `■停`（用 `roleColorHex`）+「全部打断」（`interruptAllInWorkspace`）；全死则隐藏（不挂幽灵停止键）。

**pending chip 绑 AgentState（P0 必做硬接线，治诊断 2 根因）**：现状 `pendingResponders`（`:672-706`）只看 `shim_exit/killed_at`，**需加 `liveState(id)?.state ∉ {error,exited}` 过滤**——依赖父组件透传 `agentLiveStateById`。死即移除，不挂 60s。

**模型切换**：`ModelChangeConfirm` 包在 `onSet` 外——先弹「换模型会重启队长、打断在跑回复 [取消][确认]」，确认后 `onSetModel` + 入流「模型已切换 X→Y」系统卡（`meta.subtype='model_changed'`，P1 后端 emit；P0 阶段前端乐观插本地系统消息标注待持久化）。

**优化 ✦**：复用已实现 `optimize/undoOptimize`（`:827-859`）——就地改写 + undo（`preOptimize` 保原文）；`changed=false` 显「没改动」不伪造编辑。

**附件失败回滚（P0，治诊断遗留）**：替换 `handleImageFiles`（`:871`）为 per-file 状态机（uploading/ok/failed）。失败 → 红框缩略图 +「未上传·重试」+ **path 不写进 body + 禁用发送**；`hasFailedAttachment` 派生进 `sendDisabled`。

**草稿**：扩 `draftKey` 含收件人（`:267` 加 `:${explicitRecipient?.role ?? "captain"}`），@定向草稿不串味；切无活队长会话 placeholder 显式「发送将启动队长」。

**aria**：Textarea 补 `aria-describedby` 指向当前状态说明句（禁发时解释「为何不可发」）；InterruptConfirmBar `role="alertdialog"` 焦点入「打断并发送」、Esc 关；autocomplete `role="listbox"` 补 ↑↓+Enter 键盘导航。

逐状态线框（默认/排队/打断确认/@/附件失败/停止下拉/无活队长/移动端）见 Composer 详规 §3。

---

### 3.5 右面板三态（脉搏 / 乐队 / 焦点）+ 成员

覆盖 `≥1280px`（移动端右面板不渲染）。三态共用同一容器（`WorkPanel`），band/focus 区别仅在内容（全队 vs 单成员）。

**形态**：`pulse` 脉搏条（54px，1280–1535 默认，态点列 + 召出钮 + >6 溢出 +N）/ `band` 工作面板（1536+ 常驻 `clamp(360,26vw,440)` / 1280 召出覆盖 420px）/ `focus` 单成员焦点（同容器，复用 `AgentFocusHeader` + AgentDrawer 五 tab）。

**band 内三区**：乐队栏（`MemberBand` 横向 chip = 角色色点 + 五态徽章 + 「正在<动作>」小字）/ live 顶置区（`LiveNow` 每在跑成员一行 two-signal + `⌃▣` 看终端）/ 深度 tab（`[活动][变更N][终端]` = AgentDrawer 内容面复用）。

**焦点五 tab 原样复用** `AgentDrawer.tsx:82-93`（terminal `XtermPane` gap-replay / activity `AgentActivityLog` / recordings / messages / context）。失败成员焦点头嵌 `FailureCard`。

**跨高亮**：点脉搏点 / 乐队 chip / 流内派工卡 三路径都进同一成员焦点（共享 `focusedAgentId`，`usePanelMode` hook）。

**转场**：pulse→band 滑出 180ms；band→focus 横移 160ms（乐队栏收面包屑）；resize 跨 1536 直切无动画状态保持。

**键盘**：`⌘1/2/3` 切深度 tab；Esc focus→band / band(覆盖)→pulse。

**失败成员三态呈现（单一真相源）**：pulse 红点置顶 + tooltip 短因 / band chip 红边「✕卡住」 / focus 嵌完整失败卡（唯一权威源，不在前两处重复整段错误）。

逐状态线框见右面板详规 §3。

---

### 3.6 计划卡 + 变更评审 + 合并闸

覆盖 §4.3 的「计划 / 交付 / diff·变更 / 合并」四行（审批卡/派工卡在 §3.1/§3.2 已规，此处仅交接引用）。**后端数据几乎全 missing**，前端策略「降级态优先」：先落 R-10/G-6/D-2/P-0 诚实降级，端点上线再点亮。

**① 计划置顶卡（`PlanStickyCard`，P-0..P-6）**：sticky 钉主轴顶，checklist `✓/◐/○` + 拥有成员徽章 + 卡头队长健康度点。P-0 无计划 → hairline「队长尚未给出计划」**不阻塞流不渲染空卡**。P-6 不可判定项 → `·` 中性 + tooltip「状态未知」**不默认填 ✓**。P-4 队长不健康 → 冻结灰化 + 健康点 `✕` 链失败卡。P-5 全完成 → 折叠为一行取消粘顶。数据：P1 前端 `parsePlan(markdown)` 桥（台账已有），结构化 `owner_role` 依赖后端（P2）。

**② 交付卡（`DeliveryCard`，D-0..D-4）**：`N 文件 +x/−y` + 测试输出折叠 + `[查看变更]`。D-2 无变更 → 禁用按钮。`[查看变更]`：<1536 流内就地展开大卡（R-8，虚拟化 `max-h:60vh` 防撑爆）/ ≥1536 进右面板变更 tab（R-9）。依赖后端 delivery 落库 + per-file diff（P2）。

**③ 变更评审（`ChangeReviewPanel`，R-0..R-10）**：虚拟化文件列表（`VirtualFileList`，行高 32px）+ per-file `已看`/`采纳` 复选 + 逐文件 hunk 展开（R-3）+ 行级评论（R-4 `LineCommentDraft` 预填 `file:line`，复用成员选择器发给成员）。R-7 base 脏横幅。R-10 hunk 端点缺失降级（行不可展开，显「在终端看 diff」**不假装能行级评论**）。依赖后端 hunk 端点 / `review_comments` / `file_accept` / `resolved_at`（全 P2）。

**④ 合并闸（`MergeGate`，G-0..G-6）**：四条件全 `✓` 才点亮 `[合并回 main]`——已看 / rebase / 测试 / 评论已解决。**G-6 测试未接入显灰 `·`「未运行测试」不假绿不阻塞**；base 脏 rebase 显红 `✕` **禁用合并**（硬闸）。合并前弹确认。G-3 成功入流「已合并 N 文件」系统卡；G-4 冲突告知已派成员解决（不报错）。依赖后端 `test_runs` + 合并系统卡（P2）。

逐状态线框见变更评审详规 §3。

---

## 4. i18n 文案册

> ⚖按裁决 C 统一命名空间。所有 value 严守行话防火墙（已逐条检查无禁词）。zh+en 双份，`web/src/i18n/locales/{zh,en}.json`。已有 key 标 ⟳，新增标 +。

### 4.1 新增/修改 key 大表

| key | zh-CN | 说明 |
|---|---|---|
| `messages.you` ⟳ | 我 | 已有 |
| `messages.newMessages` ⟳ | {{count}} 条新消息 | 已有 |
| `messages.empty` ⟳ | 暂无消息 | 仅 fallback，空态由能力揭示覆盖 |
| `messages.loadFailed` + | 消息加载失败 | error-load 态 |
| `messages.retryLoad` + | 重试 | |
| `messages.thoughtOnly` + | （思考完成，未给出文字结论） | 治空气泡 |
| `messages.quote` + | 引用 | 替代旧 reply 语义 |
| `messages.quoting` + | 引用了 #{{id}} | |
| `messages.jumpQuote` + | 跳转到被引用的内容 | aria |
| `messages.memberThreadHint` + | 与 {{role}} 的临时对话 | @子线程标签 |
| `messages.reasoning.summary` ⟳ | 思考摘要 | 已有 |
| `messages.reasoning.thinking` ⟳ | 思考中 | 已有 |
| **`chat.live.*`（typing + 心跳）** | | |
| `chat.live.captainThinking` + | 队长 正在思考… | A1 |
| `chat.live.captainDispatching` + | 队长 正在派工… | A2 |
| `chat.live.captainMerging` + | 队长 正在合并结果… | A3 |
| `chat.live.captainStarting` + | 队长 正在启动… | A4 |
| `chat.live.captainSlow` + | 队长 已思考 {{secs}}s… | A5 |
| `chat.live.memberStarting` + | {{role}} 正在启动… | B1 |
| `chat.live.memberDoingPrefix` + | 正在  | B2 前缀 + 动词 |
| `chat.live.memberSignal` + | {{secs}}s · {{files}} 文件 | B2 two-signal |
| `chat.live.memberWaitingDep` + | 在等 {{dep}} 完成 | B3（dep 经稳定身份解析非 key 字面） |
| `chat.live.memberStalled` + | ⚠ 已 {{secs}}s 无活动 | B4 |
| `chat.live.memberLastOutput` + |  · 上次输出 {{ago}}前 | B4 尾 |
| `chat.live.memberBackfilling` + | 正在载入… | B7 |
| **`chat.verb.*`（动词翻译，禁行话）** | | |
| `chat.verb.edit` + | 写 {{file}} | Edit/Write/MultiEdit |
| `chat.verb.read` + | 读 {{file}} | Read |
| `chat.verb.test` + | 跑测试 {{cmd}} | Bash+test |
| `chat.verb.install` + | 装依赖 | Bash+install |
| `chat.verb.build` + | 构建 | Bash+build/cargo/tsc/vite |
| `chat.verb.git` + | 执行 git {{sub}} | Bash+git |
| `chat.verb.run` + | 运行 {{cmd}} | Bash 其它 |
| `chat.verb.search` + | 查代码 {{q}} | Grep/Glob |
| `chat.verb.web` + | 查资料 | WebSearch/WebFetch |
| `chat.verb.task` + | 派子任务 | Task |
| `chat.verb.todo` + | 更新计划 | TodoWrite |
| `chat.verb.advance` + | 推进中 | system kind |
| `chat.verb.generic` + | 处理中 | 未匹配 |
| **`chat.dispatch.*`（派工卡）** | | |
| `chat.dispatch.title` + | 派给 {{role}}：{{task}} | 折叠头 |
| `chat.dispatch.statusRunning` + | 进行中 | |
| `chat.dispatch.statusDone` + | 已完成 | |
| `chat.dispatch.statusFailed` + | 未完成 | |
| `chat.dispatch.statusStarting` + | 启动中 | |
| `chat.dispatch.teamTree` + | 小团队 | |
| `chat.dispatch.depReady` + | {{dep}} 已就绪 | |
| `chat.dispatch.depWaiting` + | 在等 {{dep}} | |
| `chat.dispatch.timeline` + | 做了什么 | |
| `chat.dispatch.focusMember` + | 跳到该成员 | |
| `chat.dispatch.expand` / `.collapse` + | 展开活动 / 收起活动 | aria |
| `chat.dispatch.loading` + | 正在载入活动… | C5 |
| **`chat.systemCard.*`（卡内通用）** | | |
| `chat.systemCard.expand` / `.collapse` + | 展开 / 收起 | |
| `chat.systemCard.viewTerminal` + | 看终端 | |
| `chat.systemCard.viewLog` + | 看日志 | |
| `chat.systemCard.modelChanged` + | 队长模型已切换 {{from}} → {{to}} | model_changed |
| `chat.systemCard.engineFallback` + | {{from}} 不可用，已回退到 {{to}} 重试 | engine_fallback |
| `chat.systemCard.isolationDegraded` + | 无法隔离到独立分支，已在共享工作区继续 | isolation_degraded |
| **`chat.delivery.*`（交付卡）** | | |
| `chat.delivery.title` + | {{role}} 交付：{{task}} | |
| `chat.delivery.fileStat` + | {{files}} 文件 +{{ins}} −{{del}} | |
| `chat.delivery.testsPassed` / `.testsFailed` / `.untested` + | 测试通过 / 测试未通过 / 未测试 | |
| `chat.delivery.testOutput` + | 测试输出 | 折叠头 |
| `chat.delivery.viewChanges` + | 查看变更 | |
| `chat.delivery.summarizing` + | 正在汇总变更… | D-0 |
| `chat.delivery.noChanges` + | 未改动文件 | D-2 |
| `chat.delivery.openFailed` + | 变更暂时打不开 | D-4 |
| **`chat.review.*`（变更评审 + 合并闸）** | | |
| `chat.review.changes.tab` + | 变更 | 右面板 tab |
| `chat.review.changes.empty` + | 这条会话还没有改动 | R-0 |
| `chat.review.changes.baseDirty` + | 主线有未提交改动，先处理才能合并 | R-7 |
| `chat.review.changes.reviewed` / `.accept` + | 已看 / 采纳 | per-file |
| `chat.review.changes.askCaptainHere` + | 让队长改这里 | R-3 起评论 |
| `chat.review.changes.commentPlaceholder` + | 说说这一行哪里要改… | R-4 |
| `chat.review.changes.sendTo` + | 发给 {{member}} | R-4 |
| `chat.review.changes.viewInTerminal` + | 在终端看 diff | R-10 降级 |
| `chat.review.changes.hunkUnavailable` + | 逐行变更暂不可用，可在终端查看 | R-10 tooltip |
| `chat.review.plan.title` + | 计划 {{done}}/{{total}} | P-2 |
| `chat.review.plan.titleDraft` + | 计划草案 {{done}}/{{total}} | P-3 |
| `chat.review.plan.captainHealthy` / `.captainStuck` + | 队长 健康 / 队长卡住，点查看 | |
| `chat.review.plan.noPlan` + | 队长尚未给出计划 · 先聊聊你想做什么 | P-0 |
| `chat.review.plan.ownerSelf` + | 队长 | owner=队长 |
| `chat.review.plan.stateUnknown` + | 状态未知 | P-6 |
| `chat.review.plan.approvePrompt` + | 按此计划开始？派工后成员将各自动手。 | P-3 |
| `chat.review.plan.editPlan` / `.approveRun` + | 改计划 / 批准并开始 | P-3 |
| `chat.review.gate.title` + | 合并闸 | |
| `chat.review.gate.condReviewed` + | 全部 {{count}} 文件已看 | |
| `chat.review.gate.condRebase` / `.condRebaseBlocked` + | 可干净 rebase 到 main / 主线有改动，暂时合不了 | |
| `chat.review.gate.condTests` / `.condTestsNone` / `.condTestsFail` + | 测试通过 / 未运行测试 / 测试未通过 | G-6 中性 |
| `chat.review.gate.condComments` / `.condCommentsOpen` + | 评论已解决 / {{count}} 条评论待解决 | |
| `chat.review.gate.mergeButton` + | 合并回 main | |
| `chat.review.gate.merged` + | 已合并 {{count}} 文件回 main | G-3 |
| `chat.review.gate.conflictTitle` / `.conflictBody` + | 合并遇到冲突 / 已派 {{member}} 解决冲突 · 涉及 {{files}} | G-4 |
| **`chat.approval.*`（审批卡）** | | |
| `chat.approval.title` + | 需要你确认：{{action}} | |
| `chat.approval.what` / `.why` / `.expected` + | 做什么：{{what}} / 为什么：{{why}} / 预期结果：{{expected}} | 三要素 |
| `chat.approval.approve` / `.deny` + | 批准 / 拒绝 | |
| `chat.approval.approved` / `.denied` / `.expired` + | 你已批准 · {{time}} / 你已拒绝 · {{time}} / 已过期 | |
| **`chat.bootstrap.*`（启动清单）** | | |
| `chat.bootstrap.cardTitle` + | 队长正在上岗… | |
| `chat.bootstrap.badge` + | 启动中 | |
| `chat.bootstrap.stepIsolateDone` + | 已隔离到分支 {{base}} ↘ {{branch}} | |
| `chat.bootstrap.stepAuthDone` + | {{name}} 已登录 | |
| `chat.bootstrap.stepEngine` + | 正在启动队长引擎… | |
| `chat.bootstrap.stepFirstResponse` + | 等待队长第一次响应 | |
| `chat.bootstrap.slowHint` + | 响应较慢，再等等 | X1 |
| `chat.bootstrap.failedTitle` + | {{name}} 启动失败 | 翻转 |
| `chat.bootstrap.watchdogTimeout` + | 队长引擎 {{sec}}s 无响应 | 看门狗 |
| **`chat.emptyState.*`（空态）** | | |
| `chat.emptyState.greetingTitle` + | 我是这个会话的队长 | |
| `chat.emptyState.greetingBody` + | 说出你想做的事，我会拆成计划、派给成员推进，并在这里向你汇报。 | |
| `chat.emptyState.starterHint` + | 试试这样开始： | |
| `chat.emptyState.starter.refactor` + | 重构这个函数，抽出可复用逻辑 | |
| `chat.emptyState.starter.test` + | 给这段代码补上失败用例的测试 | |
| `chat.emptyState.starter.bug` + | 帮我查这个 bug 的根因 | |
| `chat.emptyState.precheckTitle` + | 引擎就绪 | |
| `chat.emptyState.engineLoggedIn` / `.engineMissing` + | {{name}} 已登录 / {{name}} 未安装 | |
| `chat.emptyState.noEngineTitle` / `.noEngineBody` + | 队长还没法上岗 / 需要先装好并登录至少一个 AI 引擎。 | |
| `chat.emptyState.installHint` + | 装完后 {{loginCommand}} 登录 | |
| `chat.emptyState.recheck` + | 我装好了，重新检测 | |
| `chat.emptyState.starterDisabledTip` + | 先装好并登录一个引擎 | |
| `chat.emptyState.panelEmptyTitle` / `.panelEmpty` + | 还没有成员 / 开始后，这里会实时显示每个成员正在做什么 | 右面板自我说明 |
| **`chat.orchestratorFailure.*`（扩展失败卡）** | | |
| `chat.orchestratorFailure.titleAuth` + | 队长还没法开始 | |
| `chat.orchestratorFailure.titleRate` + | 队长暂时被限流 | |
| `chat.orchestratorFailure.titleCrash` + | 队长意外退出 | |
| `chat.orchestratorFailure.titleTimeout` + | 队长启动后没有响应 | |
| `chat.orchestratorFailure.titleEscalated` + | 多次重试仍未成功，需要你处理 | X3 |
| `chat.orchestratorFailure.retryLater` + | 稍后重试 | F-rate |
| `chat.orchestratorFailure.switchEngine` + | 换 {{name}} 重试 | |
| `chat.orchestratorFailure.viewLog` + | 看日志 | 第四键 |
| `chat.orchestratorFailure.retryingNth` + | 重试中（第 {{n}}/{{max}} 次）… | X2 |
| `chat.orchestratorFailure.exitCode` + | 引擎进程异常退出（退出码 {{code}}） | F-crash |
| `chat.orchestratorFailure.runInTerminal` / `.openTerminalLogin` / `.retry` ⟳ | 在终端运行 / 打开终端登录 / 重试 | 已有，复用 |
| **`messages.*`（Composer）** | | |
| `messages.composerPlaceholder` ⟳改 | 发消息给队长，或 @成员… | 点明收件人 |
| `messages.composerPlaceholderRevive` ⟳改 | 发送将启动队长，AI 随后上线开工… | 治静默 bootstrap |
| `messages.describe.captain` / `.member` / `.revive` / `.attachFailed` / `.empty` + | （aria-describedby 说明句，见 Composer 详规 §4） | 解释为何不可发/会发生什么 |
| `messages.queuedToCaptain` / `.queuedToMember` + | 已排队 · 队长接手后送达 / 已排队 · 发给 {{role}} | pending chip |
| `messages.interruptConfirmTitle` + | 打断队长当前回复，并把这条作为新指令？ | |
| `messages.interruptConfirmBody` + | 会中止在跑成员的重规划。 | |
| `messages.interruptConfirmYes` / `.interruptCancel` + | 打断并发送 / 取消 | |
| `messages.stopMenuLabel` / `.stopMenuHeading` / `.stopMember` + | 打断 / 在跑成员 / 停 | |
| `messages.stopMemberConfirm` + | 打断 {{role}}？会中止它当前这轮。 | |
| `messages.stopAll` + | 全部打断（{{count}}） | |
| `messages.modelConfirmTitle` / `.modelConfirmBody` + | 换模型会重启队长 / 当前在跑的回复会被打断，从 {{from}} 切到 {{to}}。 | |
| `messages.modelConfirmYes` / `.modelConfirmCancel` + | 确认切换 / 取消 | |
| `messages.attachFailed` / `.attachRetry` / `.attachFailedAria` + | 未上传 / 重试 / {{name}} 上传失败，点击重试 | |
| **`workspace.panel.*` / `.band.*` / `.live.*` / `.focus.*`（右面板）** | | |
| `workspace.panel.title` / `.expand` / `.collapse` + | 工作面板 / 展开工作面板 / 收起工作面板 | |
| `workspace.panel.empty.title` / `.empty.hint` + | 还没有成员在工作 / 这里会实时显示每个成员在做什么。 | |
| `workspace.panel.overflow` + | 还有 {{count}} 名 | 脉搏溢出 |
| `workspace.band.state.*` + | （见 §2.1 五态文案） | chip 徽章 |
| `workspace.band.doing` + | 正在 {{verb}} | chip 小字 |
| `workspace.live.title` / `.empty` / `.emptyHint` + | 现在 / 此刻没有成员在跑 / 成员一旦开始干活，这里会逐行显示它正在做什么。 | |
| `workspace.live.lastOutput` / `.stalled` + | 上次输出 {{delta}}前 / 已 {{delta}} 无活动 | |
| `workspace.live.viewTerminal` / `.viewAllTerminals` + | 看终端 / 查看全部终端 | |
| `workspace.focus.back` / `.backAria` + | 全部 / 返回全队视图 | |

### 4.2 需废弃的旧 key（行话泄漏，建议清理）

| 旧 key | 问题 | 处置 |
|---|---|---|
| `agent.injectPlaceholder` | 含 "wake" | 改用「唤起」或废弃；焦点模式复用前清洗 |
| `agent.confirm.wake.desc` | 含 "mailbox/blackboard" | 改用「共享区」清洗 |
| `chat.runSpell` 等含 "spell" | 行话 | 用户可见处一律换「推进/启动」 |
| `chat.memberCount` 等含 "agent/调度/编排" | 行话 | 用户可见处换「成员/队长」 |
| 系统消息壳 `MessagesPanel.tsx:1178-1201` 居中 hairline pill | 被 `SystemCard` 派发器替换 | 渲染逻辑废弃，过滤逻辑（`:528`）保留 |
| USER→USER reply 入口 | 无语义（诊断遗留） | 移除，改「引用」 |

> 清洗旧 key 是独立任务（不阻塞本组件），但焦点模式 ActionRow 若复用到 `agent.wake/pause/resume` 中含行话的 value，必须随本期清洗。

---

## 5. 后端缺口清单（后端需新增/改造）

> 汇总所有 binding 标 missing/live-only 的项。每项：要什么 / 为什么 / 属哪期 / 量级 S·M·L。**P0 全不需要后端改动**（下表无 P0 项 = 验证 P0「完全不动后端」成立）。

| # | 要什么 | 为什么 | 期 | 量级 |
|---|---|---|---|---|
| 1 | **七种 `meta.subtype` 落库 + emit**（dispatch/delivery/plan_ref/model_changed/engine_fallback/isolation_degraded + member_mention/member_handoff） | 入流律：派工/交付/模型切换等以可重放系统卡进流，刷新重连完整重放 | P1 | M（**复用 `messages.kind+meta` 框架 `store.rs:1070`，无需新表**，参照 `wake.rs:389` 模式） |
| 2 | **`AgentActivity` 持久化新表 `agent_activities` + `GET /api/agent/:id/activity` 走 DB** | 治诊断 3：现端点只读 in-memory ring（`rest.rs:1125`，drops frames），冷加载/重连归零 | P1 | M（参照 `touch_agent_activity` `store.rs:438` 单行幂等模式） |
| 3 | **bootstrap 阶段事件链落库**（隔离/登录/启动/等待 各 subtype）+ spawn 同步落 `bootstrap_start` 占位 | 实现「启动清单卡原地翻转失败」+ 重连可重放；P0 用时间戳推断，P1 精确 | P1 | M（1 个同步 emit 点 + 阶段链） |
| 4 | **`agent_error` 入流卡** + 自动重试 n/max 计数 | 失败也入流一张系统卡，刷新后流内位置固定可重放（agents 表已有 `last_error*` 但无入流副本） | P1 | S |
| 5 | **agent 状态转换审计表**（thinking→idle→error 历史） | 冷加载需状态历史（现仅 live-only WS 流） | P1 | M |
| 6 | **精确未读端点**（`GROUP BY from_agent,thread_id WHERE to=user AND read_at IS NULL AND kind NOT IN ('wake',...)`） | 治诊断 6：系统噪声不进未读徽章，重连即拉 | P1 | S（改 `store.rs:1531` 查询） |
| 7 | **`thought_trace.status` enum**（thinking/dispatching/merging） | typing 行 A2/A3 精确文案；缺则前端用 live.state + spawn/exit 时序降级 | P1 | S |
| 8 | **计划结构化**（`task.plan.json` blackboard key 含 `[{text, owner_role, status}]`，或 orchestrator 改结构化写法） | 计划 checklist `✓/◐/○` + 拥有成员徽章机读；P1 前端 parser 桥，P2 结构化 | P2 | M |
| 9 | **per-file diff 详情**（扩展 `/diff` 或新端点返回 `{path, insertions, deletions, hunks}`） | 交付卡 `+x/−y` + 变更面逐文件行展开（现 `diff_summary` 只回文件名） | P2 | M |
| 10 | **`review_comments` 表**（+ `POST /threads/:tid/comments` + 转 user→成员消息 + `resolved_at` + 标记端点） | 行级评论 + 合并闸「评论已解决」条件 | P2 | M |
| 11 | **`file_accept` 表**（thread_id, file_path, accepted_by, accepted_at, **reviewed_at**） | per-file 采纳/已看 + 合并闸「全部文件已看」条件 | P2 | S-M |
| 12 | **`test_runs` 表 + runner + webhook**（thread_id, status, output, run_at） | 合并闸「测试通过」条件；未接入前 G-6 灰中性不阻塞 | P2 | L |
| 13 | **合并/计划系统卡 emit**（`kind=system, meta.subtype=merged/plan`） | 合并成功/计划提案入流可重放 | P2 | S |
| 14 | **approve-before-run 闸门**（会话级自治级字段 + spawn 挂起逻辑） | 计划卡 P-3 批准门；现 `spawn_bootstrap_inject` 即时注入，插 gate 非平凡 | P2 | L |
| 15 | **`@`文件枚举端点**（`GET /api/workspaces/:id/files` 或复用 worktree 树） | Composer `@` 选文件作行级上下文 | P1 | S |
| 16 | **pending 排队持久化**（`messages kind='pending'`） | Composer pending chip 刷新即丢，重连恢复排队队列 | P1 | S（schema 已就绪） |
| 17 | （可选增强）45s 无活动后端 heartbeat 事件 / Activity status enum | 把 45s 降级从纯前端时间比较改为服务器事实 | P2 | S |

**省事实（砍伪 spike）**：`messages` 表已有 `kind`+`meta` 列（迁移 0012，`store.rs:1070`），系统卡直接落 `kind='system'+meta`，**砍掉三方向都估的「messages 表 vs swarm_events 表」半天 spike**。#1/#3/#4/#13/#16 全复用此框架不建新表。

---

## 6. 组件级落地清单（P0 → P1 → P2）

> 每项 = 改哪个文件/函数 + 前端/后端量级 + 依赖 + 验收。**P0 完全不动后端，只接现有信号 + 前端诚实化。**

### P0（立住「不撒谎 + 不 firehose」，零后端改动）

| # | 改哪个文件/函数 | 前端 | 后端 | 依赖 | 验收 |
|---|---|---|---|---|---|
| P0-1 | ORCH 气泡提对比度 + 思考块降级折叠：`MessagesPanel.tsx:1278`（正文 `text-foreground-primary`）、`ReasoningDisclosure:1606`（`status==='done'` 默认 `open=false` + `bg-surface-primary/70`） | S | 0 | 复用 ChatMarkdown | 截图对比队长气泡视觉权重 > USER 气泡；done 思考块默认收起且退后 |
| P0-2 | two-signal 活态行 + 死即移除：新建 `MemberHeartbeatRow.tsx`、`CaptainTypingRow.tsx`（从 `PendingBubble:1670` 抽出），消费 `resolveMemberVisual`（`lib/agent.ts:265`）；绑 `AgentState`（`ws_swarm.rs:102`） | M | 0 | `AgentState`/`last_activity_at` 已有 | 手动 kill 一成员，活态行 ≤300ms 消失不挂 60s；45s 无输出灰降级停三点；首个输出前无绿 |
| P0-3 | pending chip 绑 AgentState：`MessagesPanel.tsx:672-706` 过滤加 `liveState(id)?.state ∉ {error,exited}`（父组件透传 `agentLiveStateById`） | S | 0 | WS state | 目标成员死 pending chip 立即移除（治诊断 2 根因） |
| P0-4 | 动词翻译器：新建 `lib/activityVerb.ts`（`verbFromLabel` + strip 隔离路径） | S | 0 | — | grep 活态行/派工卡无英文工具名裸露、无 worktree/blackboard 泄漏 |
| P0-5 | 首卡占位即时落地：前端绑 spawn 同步 `AgentState::Spawning`（`rest.rs:670` 已发）立即出 typing/占位 | S | 0 | 已有同步事件 | spawn 后 ≤2s 出 typing 行，不等 700ms tailer |
| P0-6 | 启动清单卡 + 90s 翻转：新建 `BootstrapChecklistCard.tsx`（复用 `TaskActivity.tsx` 计时 tick），P0 用 `spawned_at`/`AgentState`/`last_error_at` 时间戳推断阶段；绑 `AgentState::Error(watchdog)` 翻 F-timeout | M | 0 | 看门狗已 fire（`rest.rs:711`） | 徽章 `◐` 非绿；90s 后原地翻失败不换视图；计时 = `now−spawned_at` |
| P0-7 | 失败卡泛化：`OrchestratorFailureCard.tsx` → `FailureCard`（5 kind × 4 键，加「看日志」「换引擎」），任意 agent_id；新建 `useChatLifecycleState.ts` 收编 `orchestratorFailure` memo（`Chat.tsx:446`） | M | 0 | `last_error*` 已持久 | reason 不截断（无 line-clamp）；同一失败只在流内卡讲一次；乐队栏只显点+tooltip |
| P0-8 | 空态：新建 `EmptyState.tsx`（问候 + 3 starter + 引擎预检），读 `cliReadiness`（`Chat.tsx:200`） | S | 0 | — | E0 显 3 可点 starter 填 Composer 不直发；E1 starter 禁用 + 安装命令 + 复制 |
| P0-9 | Composer 打断/排队：`MessagesPanel.tsx:994` 加 ⌘Enter 分支；新建 `InterruptConfirmBar`、`StopMenu`；接 interrupt 端点（`http.ts:229/239`，需父透传 `workspaceId`） | M | 0 | 端点已有（`rest.rs:1806/1861`） | Enter 出 pending chip；⌘Enter 弹确认不静默发；全死成员 `[打断▾]` 隐藏 |
| P0-10 | Composer 模型切换确认：`ModelChangeConfirm` 包 `ModelPicker.onSet`（`MessagesPanel.tsx:1060`） | S | 0 | — | 切换前弹确认含 from→to；取消则不重启 |
| P0-11 | Composer 附件失败回滚：替换 `handleImageFiles`（`:871`）为 per-file 状态机；`ComposerThumb` 加 failed 变体 | M | 0 | `uploadAttachment` 已有 | 失败图红框「未上传·重试」+ path 不入 body + 禁发 |
| P0-12 | 右面板三态容器：新建 `WorkPanel/PulseRail/BandView/FocusView`，提取 `AgentDrawer` Header 为 `AgentFocusHeader`，焦点五 tab 原样复用（`AgentDrawer.tsx:82`）；`usePanelMode.ts` | M | 0 | 五 tab 已齐 | 1280 默认 54px 脉搏条；1536+ 召出 band；点脉搏点/派工卡进同成员焦点 |
| P0-13 | `classifyRow` + `SystemCard` 容器派发器 + `useAutoMarkRead`：新建 `lib/messageClassify.ts`、`components/chat/SystemCard.tsx`、`hooks/useAutoMarkRead.ts`；替换系统消息壳（`:1178`） | M | 0 | — | 4 类渲染物形态正确；未知 subtype 降级不崩；进视口去抖标记已读 |
| P0-14 | 草稿扩 key 含收件人 + revive placeholder：`MessagesPanel.tsx:267` | S | 0 | — | @定向草稿不串味；切无活队长显「发送将启动队长」 |

### P1（补持久化，兑现冷加载与重放）

| # | 改哪个文件/函数 | 前端 | 后端 | 依赖 | 验收 |
|---|---|---|---|---|---|
| P1-1 | 系统事件落库 + 入流（缺口 #1）：后端 emit 点参照 `wake.rs:389`；前端 `SystemCard` 派发各 subtype 子组件（`cards/*.tsx`） | M | M | 缺口 #1 | 刷新后派工/交付/模型卡从持久化重放 |
| P1-2 | AgentActivity 走 DB（缺口 #2）：前端重连先 `GET .../activity` 回填再接流 | S | M | 缺口 #2 | 重连后活动 tab 不空、显真实历史 |
| P1-3 | bootstrap 阶段链（缺口 #3）+ `agent_error` 入流卡（缺口 #4） | S | M | 缺口 #3/#4 | 启动清单从真实阶段事件消费；失败卡刷新位置固定 |
| P1-4 | 精确未读端点（缺口 #6） | S | S | 缺口 #6 | 未读徽章不含系统噪声，重连即拉 |
| P1-5 | `DispatchCard` + `InlineTeamTree`：复用 `dagEdgeDerivation.ts`、`AgentActivityLog`（**承认是 `Dag.tsx` 843 行内联重写**） | L | S | 缺口 #1/#2 | 折叠一行；展开见小团队树 + 活动时间线 |
| P1-6 | `MemberSubThread` + member_mention/handoff（缺口 #1 子集）；`PlanStickyCard` + `lib/parsePlan.ts`（P1 前端 parser 桥，缺口 #8 P1 部分） | M | S | 缺口 #1/#8 | @升格临时子线程；计划卡 P-0/P-6 诚实降级不假绿 |
| P1-7 | Composer pending 持久化（缺口 #16）+ `@`文件（缺口 #15）+ model_changed 卡（缺口 #1） | M | S | 缺口 #15/#16 | 刷新恢复排队；@ 可选文件 |

### P2（深度操控，性能与后端更重）

| # | 改哪个文件/函数 | 前端 | 后端 | 依赖 | 验收 |
|---|---|---|---|---|---|
| P2-1 | `ChangeReviewPanel` + `MergeGate` + `VirtualFileList` + `LineCommentDraft`：复用 `threadDiff`/`mergeThread`（`http.ts:321`） | L | M | 缺口 #9/#10/#11/#12/#13 | 虚拟化列表 ≥50 文件不卡；行级评论带 file:line；四条件全真才点亮合并；测试未接入灰中性不阻塞；base 脏禁用 |
| P2-2 | `DeliveryCard` 完整（缺口 #1/#9） | M | M | 缺口 #1/#9 | `N 文件 +x/−y` + 测试折叠；<1536 流内展开 / ≥1536 进 tab |
| P2-3 | `ApprovalCard` + APPROVE/DENY 端点（缺口 全缺） | M | M | 审批消息 + 状态机 | 三要素 + 行动按钮，永不消失，禁裸 APPROVE/DENY |
| P2-4 | 计划结构化（缺口 #8）+ approve-before-run 闸门（缺口 #14） | S | L | 缺口 #8/#14 | 计划项机读 owner；P-3 批准门（默认关） |
| P2-5 | 结构化事件 grammar 覆盖率 + 状态审计表（缺口 #5） | M | L | day-1 spike 验 AgentActivity 粒度 | 冷加载状态历史完整；粒度不足处降级文本系统卡 |

---

## 7. 实现顺序建议（P0 内部）

P0 内部推荐次序——先做最快立住「不撒谎 + 不 firehose」两条核心断言的：

1. **P0-2 + P0-3（活态行 two-signal + 绑 AgentState 死即移除 + pending chip 绑 state）** — **最高优先**。这是诊断 2「等待期撒谎」的根因修复，且 `resolveMemberVisual` 已实现诚实层、`AgentState` 已广播，纯接线。一上来就把「绿点撒谎」这个最刺眼的问题摁死，立竿见影。先做这个因为它的「不撒谎」收益最大、改动面最小（消费现成信号）。

2. **P0-4（动词翻译器 `activityVerb`）** — 紧跟 P0-2，因为活态行/派工卡显示动词依赖它，且它是纯函数零依赖，可与 P0-2 并行。先有它活态行才能显「写 refund.test.ts」而非「Edit」。

3. **P0-5 + P0-6（首卡占位 + 启动清单卡 + 90s 翻转）** — 堵「盯绿点」启动空窗。绑已有的 `Spawning` 同步事件 + 看门狗 `Error`，把「启动中（青非绿）→ 90s 原地翻失败」跑通。这一步让启动阶段也不撒谎，且看门狗背书已在后端。

4. **P0-7（失败卡泛化 + `useChatLifecycleState`）** — 治诊断 6 单一真相源 + 不截断。`OrchestratorFailureCard` 已存在，泛化为 5 kind 是中等改动。状态机 hook 把空/启动/失败三态收编，为前面三步提供统一渲染落点。

5. **P0-13（`classifyRow` + `SystemCard` 容器 + `useAutoMarkRead`）** — 立「不 firehose」的物理隔离闸。`classifyRow` 让成员消息默认 `MEMBER_SUPPRESSED`，`SystemCard` 派发器统一系统卡语法。这是 firehose 隔离的结构性基础，但不依赖前四步，可在 P0-7 后并行。

6. **P0-1（ORCH 气泡对比度 + 思考块折叠）** — 治诊断 1，纯 CSS/默认态改动，风险最低，可任意穿插（建议与 P0-13 一批，因都动 `MessagesPanel` 渲染）。

7. **P0-9/P0-10/P0-11/P0-14（Composer 四件套：打断排队 / 模型确认 / 附件回滚 / 草稿+placeholder）** — 原则 6 可操控 + 治附件/静默 bootstrap 诊断遗留。interrupt 端点已有，是接线。Composer 改动集中在一个文件，建议打包一批做完。

8. **P0-8（空态）+ P0-12（右面板三态容器）** — 收尾。空态是新手教学（独立组件，无阻塞）；右面板容器是把 `AgentDrawer` 搬成三态壳（中等改动，复用度高）。两者不阻塞核心诚实化，放最后。

**一句话**：先 P0-2/3/4 把活态诚实化（最刺眼问题、改动最小），再 P0-5/6/7 把启动+失败诚实化（看门狗背书已在），再 P0-13/1 立隔离闸+提对比，最后 Composer 批量 + 空态/右面板收尾。

---

**关键文件锚点（绝对路径）**：
- 聊天主体：`/Users/wdx/opc/swarmx-core/web/src/components/MessagesPanel.tsx`（Composer/草稿 :258-1023、@autocomplete :728、buildRows :219、系统消息壳 :1178、USER :1211、ORCH :1278、过滤 :528、TimeDivider :1571、NewMessagesDivider :1587、ReasoningDisclosure :1606、PendingBubble :1670、键盘 :988、pending 推断 :672-706）
- 诚实层：`/Users/wdx/opc/swarmx-core/web/src/lib/agent.ts`（`resolveMemberVisual` :265、`formatActivityLine` :363、`roleColor*` :42-68、阈值 :126-128）
- 复用面板/时间线：`/Users/wdx/opc/swarmx-core/web/src/components/agent/AgentDrawer.tsx`（五 tab :82-93、Header/ActionRow :350-459、`formatDelta` :96）、`AgentActivityLog.tsx`、`XtermPane.tsx`
- 复用模板：`/Users/wdx/opc/swarmx-core/web/src/components/workspace/OrchestratorFailureCard.tsx`、`/Users/wdx/opc/swarmx-core/web/src/components/TaskActivity.tsx`、`/Users/wdx/opc/swarmx-core/web/src/components/ModelPicker.tsx`、`/Users/wdx/opc/swarmx-core/web/src/lib/dagEdgeDerivation.ts:51-95`、`/Users/wdx/opc/swarmx-core/web/src/components/ChatMarkdown.tsx`
- 接线点：`/Users/wdx/opc/swarmx-core/web/src/routes/workspace/views/Chat.tsx`（cliReadiness :200、orchestratorFailure :446、retryOrchestrator :486、emptyStateOverride :862、ledger :681）
- 类型/协议：`/Users/wdx/opc/swarmx-core/web/src/api/types.ts`（MessageMeta :143、ThoughtTrace :177、AgentActivity :454、SwarmAgentState :429、AgentInfo :81-135、ThreadDiff :366、MergeResult :377）、`/Users/wdx/opc/swarmx-core/crates/swarmx-protocol/src/ws_swarm.rs`（AgentActivity :74-97、AgentState 七态 :102-118）
- 后端事实（P0 接现有信号）：`/Users/wdx/opc/swarmx-core/crates/swarmx-server/src/routes/rest.rs`（看门狗 :33,711-743、Spawning 同步发 :670、activity 端点读 ring :1125、interrupt/resume/all :1806/1824/1861、attachment :2838、optimize :2392）、`/Users/wdx/opc/swarmx-core/crates/swarmx-server/src/routes/workspaces.rs`（diff :816、merge :882）
- 后端持久化框架（P1+ emit 复用）：`/Users/wdx/opc/swarmx-core/crates/swarmx-storage/src/store.rs`（messages kind+meta :1070、last_activity_at :438、未读 :1531）、`/Users/wdx/opc/swarmx-core/crates/swarmx-server/src/wake.rs:389`（kind+meta+broadcast+store 可复制模式）
- token：`/Users/wdx/opc/swarmx-core/web/src/styles/global.css`；i18n：`/Users/wdx/opc/swarmx-core/web/src/i18n/locales/{zh,en}.json`
- 新建文件落点：`web/src/components/chat/{SystemCard,EmptyState,BootstrapChecklistCard,MemberSubThread}.tsx`、`web/src/components/chat/cards/*.tsx`、`web/src/components/messages/{CaptainTypingRow,MemberHeartbeatRow,DispatchCard,InlineTeamTree}.tsx`、`web/src/components/review/{PlanStickyCard,DeliveryCard,ChangeReviewPanel,MergeGate,VirtualFileList,LineCommentDraft,Disclosure}.tsx`、`web/src/components/workspace/{WorkPanel,PulseRail,BandView,MemberBand,LiveNow,FocusView}.tsx`、`web/src/lib/{messageClassify,activityVerb,parsePlan}.ts`、`web/src/hooks/{useChatLifecycleState,useAutoMarkRead,usePanelMode,useMergeGate,useThreadDiff}.ts`
