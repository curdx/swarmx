# 组件区规格：消息流 + 系统事件卡语法

> 范围：4 类渲染物（你的消息 / 队长消息 / 成员消息 / 系统事件卡）的完整可落地规格。  
> 对齐文档：`.ux-review/chat-redesign.md` §4.1（4 类渲染物）、§4.2（现在发生什么）、§2 原则 1/2/3/4/5/7。  
> 主文件：`web/src/components/MessagesPanel.tsx`（系统消息渲染当前在 `:1178-1201`，气泡在 `:1211/1278`，分组 `buildRows` 在 `:219-231`）。

---

## 1. 目的与边界

**解决的诊断**（来自 chat-redesign §1）：
- **诊断 1（队长气泡隐形）**：用户蓝实底是全屏最强元素，队长（真 payload）反而弱 → 本规格提队长正文对比度、思考块降级折叠。
- **诊断 4（firehose 结构性存在）**：N 个并行成员 token 流交织 → 本规格定义**物理隔离规则**：成员默认不发气泡，只在三时刻升格；高频机器输出退到系统卡的折叠层/右面板。
- **诊断 6（失败/系统信息双份、措辞不一、被截断）** → 本规格定义**统一系统事件卡语法**，单一真相源、永不截断、刷新可重放。

**对应原则**：原则 2 入流律（所有状态变更以可重放事件进流）、原则 3 单声道主轴、原则 4 对话/活动分离、原则 5 渐进披露、原则 7 稳定身份。

**本组件做什么**：
1. 渲染消息流里 4 类视觉物，并强制成员气泡的物理隔离闸。
2. 定义系统事件卡的**统一语法**（一个 `SystemCard` 容器 + 每个 `subtype` 一个解剖）。
3. 消息分组、`>5min` 时间分隔、"N 条新消息"分隔、未读语义、auto-mark-read。
4. `@定向`时成员升格为临时内联子线程。
5. markdown / 代码 / 预览 / reply / 引用渲染接线。

**本组件不做什么**（边界，交给同期其它规格）：
- Composer 本体（打断/排队/模型/优化/附件/键盘）→ 见 §4.4 规格。
- 右面板 / 乐队栏 / live 顶置区容器 → 见 §4.5 规格。
- 计划置顶粘性卡的台账 markdown 解析、diff review 面、合并闸 → 见 §4.3 规格。本规格只负责**派工/交付/审批卡在流里的渲染壳**，不负责审批端点状态机、diff 行级评论桥。
- 后端事件落库逻辑（subtype 定义、emit 点）→ 本规格列出**依赖后端新增**清单（§6），但实现属后端工单。

---

## 2. 完整状态枚举

### 2.1 渲染物类型枚举（顶层判定）

每条 `MessageRecord` 经判定函数 `classifyRow(m)` 落入唯一一类：

| 类 | 判定条件（数据源） | 形态 |
|---|---|---|
| `USER` | `m.from_agent === "user"` | 右对齐 accent 实底气泡 |
| `ORCH` | `m.from_agent === orchestratorId`（角色 `orchestrator`）且 `m.kind !== "system"` | 左对齐强对比正文气泡 + 折叠思考块 |
| `MEMBER_BUBBLE` | 成员发出（`role ∉ {orchestrator}`）且 `m.kind !== "system"` 且**满足升格条件**（见 2.4） | 成员色独立气泡 |
| `SYSTEM_CARD` | `m.kind === "system"`（含所有 `meta.subtype`） | 系统事件卡（带左色条/居中徽章） |
| `MEMBER_SUPPRESSED` | 成员发出但**不满足升格条件** | **不渲染气泡**；其信号只通过其所属系统卡（派工/交付/求助）体现 |
| `WAKE_FILTERED` | `m.kind === "wake"` 且 `meta.reason === "blackboard"` | **不渲染**（协调噪声，复用现状 `:528` 过滤） |

### 2.2 每类的视觉子状态

**USER 气泡**：`default` / `highlighted`（被引用跳转命中，ring）/ `hover`（露出引用入口）/ `with-attachment`（图片缩略图）/ `quoting`（携带 `in_reply_to` 引用条）。

**ORCH 气泡**：
- `default` / `highlighted` / `hover` / `unread`（header 蓝点）
- `with-reasoning-collapsed`（思考块默认收起，**新默认态**）/ `with-reasoning-expanded`
- `reasoning-active`（思考进行中，`thought_trace.status === "active"` → 思考块自动展开 + 旋转图标）
- `streaming`（正文逐 token 增长，可选）
- 空正文边缘：`thought-only`（只有思考无结论，仍渲染气泡壳 + "思考完成，无文字结论"占位，不留裸空气泡）

**MEMBER_BUBBLE**（仅升格时）：`mention-reply`（@定向回复）/ `quoted-by-orch`（被队长引用浮上）/ `help`（求助，琥珀左缘）。归属一律成员色头像 + 角色派生名。

**SYSTEM_CARD**：见 2.3 全枚举。

### 2.3 系统事件卡 subtype × 状态全枚举

每张卡有 `collapsed`（默认一行）/ `expanded`（`▾` 展开）两形态，外加 subtype 专属状态：

| subtype | 专属状态枚举 | 持久化（见 §6） |
|---|---|---|
| `dispatch`（派工） | `started` / `running` / `done` / `failed` | **missing → 依赖后端新增** |
| `delivery`（交付） | `clean`（测试过）/ `tests_failed` / `untested` | **missing → 依赖后端新增** |
| `plan_ref`（计划引用，流内引用置顶计划卡的某项） | `cited`（队长引用某 checklist 项） | **missing → 依赖后端新增** |
| `approval_required`（审批/需要你） | `pending` / `approved` / `denied` / `expired` | **missing → 依赖后端新增** |
| `agent_error`（失败） | `auth` / `rate_limit` / `watchdog` / `fatal` × `unresolved` / `retrying`(n/max) / `resolved` | 部分：`agents.last_error*` 已持久化（迁移 0022）；**入流卡 missing** |
| `bootstrap`（启动清单） | 多阶段子状态：`spawning` / `isolated` / `logged_in` / `engine_starting` / `awaiting_first_response` / `failed`（任一步翻转） | **missing → 依赖后端新增 bootstrap 事件链** |
| `model_changed`（模型切换） | `applied` | **missing → 依赖后端新增 emit 点** |
| `engine_fallback`（引擎回退） | `fell_back`（A→B）| **missing → 依赖后端新增** |
| `isolation_degraded`（隔离降级） | `degraded`（无法隔离到分支，降级为共享工作区） | **missing → 依赖后端新增** |

### 2.4 成员气泡升格条件（物理隔离闸）

成员消息**默认 `MEMBER_SUPPRESSED`**（不进流）。仅以下任一为真时升格 `MEMBER_BUBBLE`：

1. **@定向**：当前用户最近一条消息 `@<该成员>` 且该成员的消息是其直接回复（`in_reply_to` 指向用户 @消息，或处于活跃 `@子线程`窗口内，见 §8.6）。判据字段：**依赖后端新增** `meta.subtype === "member_mention"`。
2. **被队长引用**：队长某条消息 `in_reply_to` 指向该成员消息（队长把成员结论冒泡）。判据：`orchMsg.in_reply_to === memberMsg.id`（现有字段可推导）。
3. **求助**：成员主动请求人介入。判据：**依赖后端新增** `meta.subtype === "member_handoff"`（求助）或归一为 `approval_required` 卡。

升格之外的一切成员产物（编辑文件、跑测试、读代码）**绝不进流**，只走：派工卡展开的活动时间线（§8.4）/ 右面板。这是 firehose 物理隔离的核心。

### 2.5 流级状态（整个消息列表）

| 状态 | 触发 | 渲染 |
|---|---|---|
| `empty` | `rows.length === 0` 且队长健康 | 能力揭示空态（§4.5 空态规格接管；本组件渲染 `emptyStateOverride` 插槽，复用 `:1150`） |
| `empty-but-failed` | `rows.length === 0` 且 `agents.last_error != null` | 渲染失败卡（`OrchestratorFailureCard`），**不是**裸"暂无消息"（治诊断 6） |
| `loading` | 首次拉历史中 | 骨架占位（3 行灰条），不显"暂无消息" |
| `error-load` | 历史拉取失败 | 居中"消息加载失败 · 重试"行 + 重试按钮，**不**伪装成空 |
| `populated` | 有消息 | 正常流 |
| `populated-pending` | 有消息 + 队长 typing 行 / 成员活态行在尾部 | 见 §8.3 |

---

## 3. 逐状态 ASCII 线框

> 主轴限宽 `max-w-[720px]` 居中（治诊断 5 的"两条短消息贴顶"）。坐标尺寸标注用 design token。

### 3.1 USER 气泡（右对齐，accent 实底）

```
                                          ┌── max-w: min(82%,780px) ──┐
                              我                                        ← header(可选): font-heading 11px, foreground-tertiary
                              ┌──────────────────────────────────────┐
                              │ 把校验抽成独立函数                     │ ← body: font-body 13px / leading-snug
                              │                              02:56 ↗  │ ← clock: caption 10px, on-accent/70 浮右
                              └──────────────────────────────────────┘
                              radius: 2xl(12px) + br-sm(4px) 尾角
                              bg: accent-primary  text: on-accent
                              hover 露出: [引用] (替代旧 reply)
```

### 3.2 ORCH 气泡（左对齐，强对比 + 思考块折叠）

```
┌28px┐
│ 队 │ 队长 · 02:56  ●        ← header: 角色名 heading 13px/semibold/foreground-PRIMARY, 时间 caption 10px, 未读蓝点 1.5px
└────┘ ┌──────────────────────────────────────────────────┐
       │ ▸ 思考摘要                           ⏱ 4s          │ ← 折叠块: 默认收起(改: status=done 收起), border-subtle, bg-surface-primary/70
       ├──────────────────────────────────────────────────┤   text 11px caption
       │ 好，我拆两步：先抽出 validateRefundAmount，          │ ← 正文: ChatMarkdown, .prose-chat
       │ 再补失败用例。                                      │   color: foreground-PRIMARY (提对比, 治诊断1)
       │ ```ts                                              │ ← 代码块: prose-chat pre, mono 0.82em
       │ function validateRefundAmount(x){…}                │
       │ ```                                                │
       └──────────────────────────────────────────────────┘
       radius: 2xl + bl-sm 尾角  bg: surface-secondary  border: border-subtle
```
**关键改动 vs 现状**：正文 `text-foreground-primary`（已是），但需**移除思考块挤占视觉权重**——思考块改 `bg-surface-primary/70`（更暗更退后）且 `status="done"` 默认 `open=false`（现状 `ReasoningDisclosure` `:1614` 已支持，仅需确保传入 `status` 正确）。

### 3.3 系统事件卡 · 统一语法（解剖）

```
▏┌──────────────────────────────────────────────────────────────┐
▏│ [⬡徽章] 一行原因文案                              02:57   ▾   │ ← 卡头(collapsed): 32px 高
▏│        ‹左色条 3px›                                            │
▏└──────────────────────────────────────────────────────────────┘
 └ 左色条: 3px, 颜色=subtype 语义色 (派工=成员色, 失败=danger, 模型=info...)

展开态(expanded, ▾→▴):
▏┌──────────────────────────────────────────────────────────────┐
▏│ [⬡徽章] 一行原因文案                              02:57   ▴   │
▏│ ────────────────────────────────────────────────────────────│ ← border-subtle hairline
▏│  ‹卡专属正文区: 时间线 / 文件列表 / 三要素 / 小团队树›          │
▏│ ────────────────────────────────────────────────────────────│
▏│  [行动按钮A]  [行动按钮B]  [次要链接]                          │ ← 可选行动条, 按钮 min-h 32px
▏└──────────────────────────────────────────────────────────────┘
```
**语法规则**：
- 左侧 `[⬡徽章]`= **事件类型徽章（非头像）**，与气泡头像形状区别（菱形/方形 vs 圆形），避免误读为"某人说话"。
- 系统卡**不右对齐、不进 28px 头像 gutter**，占满主轴宽（居中限宽内）。
- 每张卡服务器持久化、可重放（§6）。`title` attr 带 `#id · subtype · 全时间戳`（复用现状 `:1196`）。

### 3.4 派工卡（dispatch）— collapsed / expanded

```
collapsed:
▏[⬡派工] 派给 测试成员：补失败用例              ● 进行中   ▾
 └ 左色条=测试成员色(--color-agent-test 绿)

expanded = 小团队树 + 活动时间线:
▏[⬡派工] 派给 测试成员：补失败用例              ● 进行中   ▴
▏ ─────────────────────────────────────────────────────
▏  队长
▏   └─◐ 测试成员 ── 依赖: validateRefundAmount ✓已就绪    ← 小团队树(depends_on/handoff_signal)
▏ ─────────────────────────────────────────────────────
▏  时间线:
▏   ✓ 读 refund.ts            · 1s
▏   ◐ 写 refund.test.ts       · 14s            ⌃▣        ← ⌃▣ = 看终端(去右面板)
▏ ─────────────────────────────────────────────────────
▏  [跳到该成员焦点]
```

### 3.5 交付卡（delivery）

```
▏[⬡交付] 测试成员 交付：补失败用例 · 3 文件 +48 −5     ✓ 测试通过   ▾
expanded:
▏  refund.test.ts        +42 −0
▏  refund.ts             +6  −5
▏  test_helpers.ts       +0  −0(改名)
▏ ─────────────────────────────────────────────────────
▏  ▸ 测试输出 (12 passed)                                 ← 折叠, 复用 ReasoningDisclosure 壳
▏  [查看变更]                                              ← 去变更 tab
```

### 3.6 审批/需要你卡（approval_required，红/琥珀，永不消失）

```
▏┌─ 琥珀左色条 ─────────────────────────────────────────┐
▏│ [⬡需要你] 需要你确认：合并退款分支到 main             ▴ │
▏│ ─────────────────────────────────────────────────────│
▏│  做什么：把 退款流程 的 3 处变更合并回主线              │ ← 三要素(必填)
▏│  为什么：测试已通过，队长判断可交付                     │
▏│  预期结果：main 增加 refund 校验 + 失败用例            │
▏│ ─────────────────────────────────────────────────────│
▏│  [批准]   [拒绝]   [看终端]                            │ ← 禁裸 APPROVE/DENY
▏└───────────────────────────────────────────────────────┘
 永不消失: 处理前一直钉在流内 + 收件箱 + 桌面通知
```

### 3.7 失败卡（agent_error，单一真相源）

```
▏┌─ danger 左色条 ──────────────────────────────────────┐
▏│ [✕失败] 队长还没法开始                                 │
▏│ Claude 未登录                                          │ ← 完整原因, 永不截断
▏│ ┌──────────────────────────────────────┐             │
▏│ │ claude /login              [复制]      │             │ ← 登录命令(auth 类)
▏│ └──────────────────────────────────────┘             │
▏│ [打开终端登录] [换 Codex 重试] [重试] [看日志]         │ ← 四键(加"看日志")
▏└───────────────────────────────────────────────────────┘
 自动重试 ≤2-3 次, 每次入流; retrying 态显 "重试中 2/3…"
```

### 3.8 启动清单卡（bootstrap，逐步消费 + 90s 看门狗原地翻转）

```
进行中:
▏[⬡启动中] 退款流程 正在启动…                    8s   ▴
▏ ─────────────────────────────────────────────────────
▏  ✓ 隔离到分支 main↘退款流程
▏  ✓ Claude 已登录
▏  ◐ 启动队长引擎…                                8s
▏  ○ 等待首次响应
 徽章 ◐启动中(非绿 / state-busy 琥珀)

90s 看门狗触发 / 任一步失败 → 原地翻转(同一卡, 不换视图):
▏[✕失败] 退款流程 启动失败                              ▴
▏  ✓ 隔离到分支 ✓ 已登录
▏  ✕ 队长引擎 90s 无响应 (看门狗)
▏  [看日志] [重试] [换 Codex]
```

### 3.9 模型切换 / 引擎回退 / 隔离降级（轻量单行卡，少展开）

```
模型切换:  [⬡切换] 队长模型已切换 opus·中 → sonnet·中            02:58
引擎回退:  [⬡回退] Claude 不可用，已回退到 Codex 重试            02:58   ▾
隔离降级:  [⬡注意] 无法隔离到独立分支，已在共享工作区继续         02:58   ▾
```

### 3.10 时间分隔 / "N 条新消息" / @定向子线程

```
>5min 时间分隔:        ──────────  今天 03:14  ──────────   ← TimeDivider, 复用 :1571
"N 条新消息" 分隔:     ━━━━━━━━━  3 条新消息  ━━━━━━━━━     ← NewMessagesDivider, 复用 :1587, 蓝色

@定向临时子线程(升格):
  我: @测试成员 这个用例的边界值对吗？
   ┌测┐ 测试成员 · 03:15            ← 成员色头像独立气泡(临时子线程)
   └──┘ 对，我已经覆盖了 0 和负数…
  ╰─ 子线程窗口: 缩进 16px + 左侧 2px 成员色细条, 队长重新发言或 60s 无新 @ 即收拢
```

---

## 4. 精确中文文案 + i18n key

> 行话防火墙：**禁** mailbox/blackboard/wake/worktree/shim/spell/PTY/handoff/dispatch。用 队长/成员/会话/计划/变更/推进/启动/隔离。  
> key 命名沿用现状 `messages.*`（流相关）、`chat.systemCard.*`（系统卡，新增命名空间）、`common.*`。已有 key 直接复用，标 ⟳。

### 4.1 流通用

| key | zh 文案 | 备注 |
|---|---|---|
| `messages.you` ⟳ | `我` | 已存在 |
| `messages.newMessages` ⟳ | `{{count}} 条新消息` | 已存在 |
| `messages.empty` ⟳ | `暂无消息` | 仅 fallback；空态由能力揭示插槽覆盖 |
| `messages.loadFailed` | `消息加载失败` | 新增 |
| `messages.retryLoad` | `重试` | 新增 |
| `messages.reasoning.summary` ⟳ | `思考摘要` | 已存在 |
| `messages.reasoning.thinking` ⟳ | `思考中` | 已存在 |
| `messages.thoughtOnly` | `（思考完成，未给出文字结论）` | 新增，治空气泡 |
| `messages.quote` | `引用` | 替代旧 `reply` 语义，新增 |
| `messages.quoting` | `引用了 #{{id}}` | 新增 |
| `messages.jumpQuote` | `跳转到被引用的内容` | 新增 |
| `messages.memberThreadHint` | `与 {{role}} 的临时对话` | @子线程标签，新增 |

### 4.2 系统卡（新命名空间 `chat.systemCard.*`）

**统一：**
| key | zh |
|---|---|
| `chat.systemCard.expand` | `展开` |
| `chat.systemCard.collapse` | `收起` |
| `chat.systemCard.viewTerminal` | `看终端` |
| `chat.systemCard.viewLog` | `看日志` |

**派工 dispatch：**
| key | zh |
|---|---|
| `chat.systemCard.dispatch.title` | `派给 {{role}}：{{task}}` |
| `chat.systemCard.dispatch.running` | `进行中` |
| `chat.systemCard.dispatch.done` | `已完成` |
| `chat.systemCard.dispatch.failed` | `失败` |
| `chat.systemCard.dispatch.depReady` | `已就绪` |
| `chat.systemCard.dispatch.depWaiting` | `等待中` |
| `chat.systemCard.dispatch.focusMember` | `跳到该成员` |

**交付 delivery：**
| key | zh |
|---|---|
| `chat.systemCard.delivery.title` | `{{role}} 交付：{{task}}` |
| `chat.systemCard.delivery.fileStat` | `{{files}} 文件 +{{ins}} −{{del}}` |
| `chat.systemCard.delivery.testsPassed` | `测试通过` |
| `chat.systemCard.delivery.testsFailed` | `测试未通过` |
| `chat.systemCard.delivery.untested` | `未测试` |
| `chat.systemCard.delivery.testOutput` | `测试输出` |
| `chat.systemCard.delivery.viewChanges` | `查看变更` |

**计划引用 plan_ref：**
| key | zh |
|---|---|
| `chat.systemCard.planRef.title` | `计划第 {{n}} 项：{{item}}` |
| `chat.systemCard.planRef.jump` | `跳到计划` |

**审批 approval_required：**
| key | zh |
|---|---|
| `chat.systemCard.approval.title` | `需要你确认：{{action}}` |
| `chat.systemCard.approval.what` | `做什么：{{what}}` |
| `chat.systemCard.approval.why` | `为什么：{{why}}` |
| `chat.systemCard.approval.expected` | `预期结果：{{expected}}` |
| `chat.systemCard.approval.approve` | `批准` |
| `chat.systemCard.approval.deny` | `拒绝` |
| `chat.systemCard.approval.approved` | `你已批准 · {{time}}` |
| `chat.systemCard.approval.denied` | `你已拒绝 · {{time}}` |
| `chat.systemCard.approval.expired` | `已过期` |

**失败 agent_error：**
| key | zh |
|---|---|
| `chat.systemCard.error.titleOrch` | `队长还没法开始` |
| `chat.systemCard.error.titleMember` | `{{role}} 卡住了` |
| `chat.systemCard.error.runInTerminal` ⟳ | `在终端运行` （复用 `chat.orchestratorFailure.runInTerminal`） |
| `chat.systemCard.error.openTerminalLogin` ⟳ | `打开终端登录` |
| `chat.systemCard.error.switchEngine` | `换 {{engine}} 重试` |
| `chat.systemCard.error.retry` ⟳ | `重试` |
| `chat.systemCard.error.retrying` | `重试中 {{n}}/{{max}}…` |
| `chat.systemCard.error.viewLog` | `看日志` |

**启动清单 bootstrap：**
| key | zh |
|---|---|
| `chat.systemCard.bootstrap.title` | `{{name}} 正在启动…` |
| `chat.systemCard.bootstrap.failedTitle` | `{{name}} 启动失败` |
| `chat.systemCard.bootstrap.isolated` | `隔离到分支 {{branch}}` |
| `chat.systemCard.bootstrap.loggedIn` | `{{engine}} 已登录` |
| `chat.systemCard.bootstrap.engineStarting` | `启动队长引擎…` |
| `chat.systemCard.bootstrap.awaitingResponse` | `等待首次响应` |
| `chat.systemCard.bootstrap.watchdogTimeout` | `队长引擎 {{sec}}s 无响应` |

**模型切换 / 回退 / 隔离降级：**
| key | zh |
|---|---|
| `chat.systemCard.modelChanged` | `队长模型已切换 {{from}} → {{to}}` |
| `chat.systemCard.engineFallback` | `{{from}} 不可用，已回退到 {{to}} 重试` |
| `chat.systemCard.isolationDegraded` | `无法隔离到独立分支，已在共享工作区继续` |

**用户动词翻译表**（`AgentActivity.label` → 中文用户动词，**绝不泄漏行话**），新增 `chat.activityVerb.*`：
| 内部 label 含 | zh 动词 | key |
|---|---|---|
| `Edit`/`Write` | `写 {{file}}` | `chat.activityVerb.edit` |
| `Read` | `读 {{file}}` | `chat.activityVerb.read` |
| `Bash`(test) | `跑测试` | `chat.activityVerb.test` |
| `Bash`(install) | `装依赖` | `chat.activityVerb.install` |
| `Bash`(其它) | `执行命令` | `chat.activityVerb.run` |
| `Grep`/`Glob` | `查代码` | `chat.activityVerb.search` |
| 未知 | `处理中` | `chat.activityVerb.generic` |

---

## 5. 尺寸 / 间距 / 色彩 token

全部用现有 token（`web/src/styles/global.css`）。

### 5.1 布局尺寸
| 项 | 值 |
|---|---|
| 主轴限宽 | `max-w-[720px]`（**改**：现状 `:1156` 为 `max-w-[1040px]`，重设计收窄为 720 治行长） |
| 气泡最大宽 | `max-w-[min(82%,780px)]`（USER）/ `max-w-[min(82%,820px)]`（ORCH，复用现状） |
| 头像 gutter | `w-8`（28-32px，复用 `:1291`） |
| 系统卡左色条 | `3px`（`border-l-[3px]`） |
| 卡头高 | `min-h-8`（32px） |
| 行动按钮 | `min-h-8`（32px，触达） |
| @子线程缩进 | `pl-4`（16px）+ `border-l-2` 成员色 |
| 组间距 | header 行 `mt-3`，同发送方续行 `mt-2`（复用 `buildRows` 逻辑） |

### 5.2 圆角 / 字号 / 行高
| 项 | token |
|---|---|
| 气泡圆角 | `rounded-2xl`（12px）+ 尾角 `rounded-br-sm`/`rounded-bl-sm`（4px） |
| 系统卡圆角 | `rounded-xl`（12px，比气泡略方，区别形状） |
| 正文 | `font-body` 13px / `leading-snug`；markdown 走 `.prose-chat`（13px / 1.6） |
| header 名字 | `font-heading` 13px / `font-semibold` / `text-foreground-primary`（ORCH 提对比） |
| 时间戳 | `font-caption` 10px / `tabular-nums` / `text-foreground-tertiary` |
| 系统卡原因 | `font-caption` 12px / `leading-5` |
| 思考块 | 11px / `font-caption` |

### 5.3 色彩
| 元素 | token |
|---|---|
| USER 气泡 | `bg-accent-primary` / `text-foreground-on-accent` |
| ORCH 气泡 | `bg-surface-secondary` / `border-border-subtle` / 正文 `text-foreground-primary` |
| ORCH 思考块（done） | `bg-surface-primary/70` / `border-border-subtle`（**改**：比 active 的 `accent-primary-soft` 更退后） |
| 成员色（头像/左色条/子线程条） | `roleColorClass(role)` → `--color-agent-{planner/backend/frontend/test/...}` |
| 派工卡左色条 | 成员色 |
| 交付卡 `✓测试通过` | `text-status-success` / `bg-status-success-soft` |
| 交付卡 `测试未通过` | `text-status-danger` / `bg-status-danger-soft` |
| 失败卡 | 左色条 `border-status-danger`，bg `bg-status-danger/5`（复用 `OrchestratorFailureCard` `:73`） |
| 审批卡 | 左色条 `--color-state-warning`（琥珀），bg `bg-status-warning-soft` |
| 启动中徽章 | `text-status-busy` / `--color-state-busy`（**非绿**，事实律） |
| 模型/回退/隔离卡 | `text-status-info` / `--color-state-info` |
| 未读蓝点 | `bg-accent-primary`，1.5px |
| "N 条新消息"分隔 | `accent-primary` |
| `⬡` 徽章背景 | `bg-surface-tertiary`，icon `text-foreground-tertiary`（中性，避免抢色） |

---

## 6. 数据绑定表

> 持久化标注：`persisted`(可重放) / `live-only`(WS，刷新丢) / `missing`(需后端新增)。引用 `file:line`。

| 动态元素 | 数据源信号 | 持久化 | file:line | 缺口 / 动作 |
|---|---|---|---|---|
| USER 气泡正文/时间 | `messages`(from=user) | persisted | `MessagesPanel.tsx:1211-1268`；`store.rs:1070-1081` | 无缺口 |
| ORCH 气泡正文 | `messages`(from=orchestrator, kind≠system) | persisted | `MessagesPanel.tsx:1278-1396` | 无缺口 |
| ORCH 思考块 | `thought_trace`(status/summary/started_at) | persisted | `types.ts:177-189`；`models.rs:264-267` | 无缺口；仅改默认折叠 + 对比度 |
| ORCH typing 行 status | `thought_trace.status` + 首个 `AgentActivity` | live-only | `types.ts:454-462`；`ws_swarm.rs:81-97` | **依赖后端**：`thought_trace.status` 已有 enum (`types.ts:184` active/done/error)，但 typing 行需绑首个 `AgentActivity` live；spawn 需同步落占位（见下） |
| 成员气泡升格判定 | `meta.subtype` ∈ {member_mention, member_handoff} + `in_reply_to` | persisted(框架) | `types.ts:143-152` | **依赖后端新增** `member_mention`/`member_handoff` subtype 生成逻辑 |
| 成员活态行 动词 | `AgentActivity.label` → `chat.activityVerb.*` | live-only | `ws_swarm.rs:74-97` | 翻译表前端做；label 来源 live |
| 成员活态行 计时/上次输出 | `last_activity_at`(agents 表) | persisted | `store.rs:438-452`；迁移 0013；`types.ts:118-123` | 45s 降级用此戳；前端时间比较 |
| 成员活态行 死即移除 | `SwarmAgentState`(error/exited) | live-only | `ws_swarm.rs:102-118`；`types.ts:429-441` | **接线**：绑 `AgentState`，治诊断 2（现状 PendingBubble 挂 60s 不绑 state） |
| 派工卡 collapsed | `messages`(kind=system, meta.subtype=dispatch) | **missing** | — | **依赖后端新增** dispatch 消息落库（参考 `wake.rs:389` 模式） |
| 派工卡 小团队树 | `depends_on`/`handoff_signal`/`parent_agent_id` + `dagEdgeDerivation` | persisted | `types.ts:92-109`；`lib/dagEdgeDerivation.ts:51-95` | 数据全；**承认是 `Dag.tsx`(843行) 内联重写**(chat-redesign §6 P2) |
| 派工卡 活动时间线 | `GET /api/agent/:id/activity`(in-memory ring) | **live-only→missing** | `rest.rs:1125-1136`；`ws_swarm.rs:78-80`(drops frames) | **依赖后端**：改走 DB(治诊断 3)，新表 `agent_activities`(参考 `touch_agent_activity` `store.rs:438-452`) |
| 交付卡 | `messages`(kind=system, meta.subtype=delivery) + worktree diff | **missing** | `models.rs`(ThreadRecord:77-78 dirty/ahead/behind) | **依赖后端新增** delivery 消息(files/+x/−y/test_output 入 meta) |
| 计划引用卡 plan_ref | `messages`(kind=system, meta.subtype=plan_ref) | **missing** | — | **依赖后端新增**；计划卡本体属 §4.3 规格 |
| 审批卡 | `messages`(kind=system, meta.subtype=approval_required) + APPROVE/DENY 端点 | **missing** | — | **依赖后端新增** 消息 + 操作端点 + 状态机(全缺) |
| 失败卡 原因/类别 | `agents.last_error/last_error_kind/last_error_at` | persisted | `models.rs:52-65`(迁移0022)；`rest.rs:710-730` | 字段已持久；**依赖后端**：失败也入流一张 kind=system 卡(meta.subtype=agent_error) 以便重放 |
| 失败卡 自动重试计数 | 重试审计 | **missing** | — | **依赖后端新增** 重试 n/max 记录 |
| 启动清单卡 | `messages`(kind=system, meta.subtype=bootstrap 各阶段) + `AgentState` 转换 | **missing** | `rest.rs:33`(看门狗90s)、`:670-672`(Spawning 同步发) | **依赖后端**：bootstrap 事件链(隔离/登录/启动/等待)落库；spawn 同步落 `bootstrap_start` 占位卡 |
| 启动卡 90s 翻转 | 看门狗 `record_agent_error("watchdog")` + `AgentState::Error` | persisted | `rest.rs:33,711-743` | 后端已 fire；前端**接线**消费 Error 翻转卡 |
| 模型切换卡 | `messages`(kind=system, meta.subtype=model_changed) | **missing** | `ModelPicker.tsx:12`(注释确证重启) | **依赖后端新增** emit 点；Composer 规格负责确认弹窗 |
| 引擎回退卡 | `messages`(kind=system, meta.subtype=engine_fallback) + `fallback_from` | **missing** | — | **依赖后端新增** emit 点 |
| 隔离降级卡 | `messages`(kind=system, meta.subtype=isolation_degraded) | **missing** | — | **依赖后端新增** emit 点 |
| 时间分隔 `>5min` | `messages.sent_at` 差 > `GROUP_GAP_MS` | persisted | `MessagesPanel.tsx:140,219-231,1571` | 无缺口（复用） |
| "N 条新消息" | `messages.read_at` + `firstUnreadId` | persisted | `MessagesPanel.tsx:1171-1176,1587` | 无缺口（复用） |
| 未读计数(精确) | `count_unread` | persisted | `store.rs:1531-1575` | **依赖后端**：改 `GROUP BY from_agent,thread_id WHERE to=user AND read_at IS NULL AND kind NOT IN ('wake',...)`，重连即拉(治诊断 6) |
| auto-mark-read | `mark_read` 端点 | persisted | `store.rs:1531-1575`；`MessagesPanel.tsx:455` | 无缺口；前端补 IntersectionObserver(见 §8.5) |
| reply/引用 | `messages.in_reply_to` | persisted | `MessagesPanel.tsx:1240-1247,1341-1349` | 复用；**改**：user→user reply 入口移除，改"引用任意流内卡片" |

**§6 缺口汇总（依赖后端新增 X）**：
1. dispatch/delivery/plan_ref/approval_required/model_changed/engine_fallback/isolation_degraded 七种 `meta.subtype` 的定义与 emit 落库（复用 `messages.kind+meta` 框架，无需新表，`store.rs:1070`）。
2. bootstrap 事件链落库 + spawn 同步占位卡。
3. `agent_error` 入流卡（agents 表已有字段，缺入流副本）+ 自动重试计数。
4. `AgentActivity` 持久化新表 + `GET /api/agent/:id/activity` 走 DB（诊断 3 硬依赖）。
5. `member_mention`/`member_handoff` subtype 生成逻辑。
6. 精确未读端点 `GROUP BY` 改写。
7. 审批 APPROVE/DENY 端点 + 状态机（全缺）。

---

## 7. 复用 vs 新建

### 复用（接线/小改，不重写）
| 资产 | file | 用途 | 改动 |
|---|---|---|---|
| `buildRows` | `MessagesPanel.tsx:219-231` | 分组/时间分隔/showHeader | 无改；系统卡也走它 |
| `TimeDivider` | `MessagesPanel.tsx:1571` | `>5min` 分隔 | 复用 |
| `NewMessagesDivider` | `MessagesPanel.tsx:1587` | "N 条新消息" | 复用 |
| `ReasoningDisclosure` | `MessagesPanel.tsx:1606-1668` | 思考块 + 交付卡测试输出折叠壳 | **改**：done 默认收起；正文区降对比 |
| `ChatMarkdown` | `web/src/components/ChatMarkdown.tsx` | GFM/代码/预览渲染 | 复用（保留 `.prose-chat`/代码/预览 tab） |
| `roleColorClass`/`roleColorHex`/`resolveRole`/`resolveMemberVisual` | `web/src/lib/agent.ts:42-265` | 稳定身份色/名（原则 7） | 复用，在派工卡/活态行/子线程一致引用 |
| `OrchestratorFailureCard` | `web/src/components/workspace/OrchestratorFailureCard.tsx` | 失败卡模板 | **泛化**为通用 `SystemCard` 的 agent_error 渲染：①任意 agent_id ②加"看日志"第四键 ③"换引擎"键 |
| USER 气泡 | `MessagesPanel.tsx:1211-1268` | 你的消息 | **改**：reply→引用；主轴限宽 1040→720 |
| ORCH 气泡 | `MessagesPanel.tsx:1278-1396` | 队长消息 | **改**：思考块默认折叠；提对比度 |
| 系统消息壳 | `MessagesPanel.tsx:1178-1201` | 当前居中 hairline pill | **替换**为 `SystemCard` 派发器（按 `meta.subtype` 路由） |
| wake meta 模式 | `wake.rs:389-396,849-858` | kind+meta 生成/广播/存储全链 | 后端各 subtype emit 复用此模式 |
| `dagEdgeDerivation` | `web/src/lib/dagEdgeDerivation.ts:51-95` | 小团队树边推导 | 复用驱动派工卡展开树 |

### 新建
| 资产 | 落点 | 说明 |
|---|---|---|
| `classifyRow(m)` | `MessagesPanel.tsx`(或 `lib/messageClassify.ts`) | 4 类 + 抑制/过滤的唯一判定函数 |
| `SystemCard`（容器 + 派发器） | `web/src/components/chat/SystemCard.tsx` | 统一语法壳（左色条 + 徽章 + 原因 + 行动条 + ▾），按 subtype 派发子组件 |
| `DispatchCard` / `DeliveryCard` / `ApprovalCard` / `BootstrapCard` / `PlanRefCard` / `ModelChangedCard` / `EngineFallbackCard` / `IsolationDegradedCard` | `web/src/components/chat/cards/*.tsx` | 各 subtype 解剖；轻量卡（model/fallback/isolation）共用一个 `OneLineCard` |
| `activityVerb(label)` 翻译器 | `web/src/lib/activityVerb.ts` | `AgentActivity.label` → `chat.activityVerb.*`，防行话泄漏 |
| `MemberSubThread`（@子线程窗口） | `web/src/components/chat/MemberSubThread.tsx` | 升格的成员气泡缩进 + 成员色细条 + 收拢逻辑 |
| `useAutoMarkRead`（IntersectionObserver） | `web/src/hooks/useAutoMarkRead.ts` | 进视口去抖标记已读（见 §8.5） |

---

## 8. 交互与时序

### 8.1 渲染分派时序
1. 拉历史 `messages` → 每条 `classifyRow(m)`。
2. `kind==='wake' && meta.reason==='blackboard'` → 丢弃。
3. `kind==='system'` → `SystemCard`（按 `meta.subtype` 派发；未知 subtype → 降级为中性单行卡，**不崩**）。
4. 成员消息 → `MEMBER_SUPPRESSED` 除非满足 2.4 升格 → `MEMBER_BUBBLE` / `MemberSubThread`。
5. `buildRows` 计算 `showHeader`/`showDividerBefore`（系统卡不进头像 gutter，但仍参与时间分隔）。

### 8.2 卡展开/折叠
- 默认 `collapsed`。点卡头任意处或 `▾` → `expanded`（aria-expanded 切换）。
- **审批卡/失败卡默认 `expanded`**（永不消失 + 必须可操作）。
- 展开态懒加载重数据（派工活动时间线、交付 diff）：仅展开时打 `GET /api/agent/:id/activity` / threadDiff，避免折叠态拉全部。

### 8.3 pending / 活态时序（治诊断 2）
- 用户发消息 → **1-2s 内**必出 ORCH typing 行（`队长正在 思考…`），来源 `thought_trace.status` + 首个 `AgentActivity`；proof-of-life 先于 spinner。
- spawn 成员 → **不等 700ms tailer**，同步落 `bootstrap` 占位卡（依赖后端）。
- 成员活态行：每 500ms tick 更新计时（复用 `PendingBubble` `:1683` 的 interval）。`now - last_activity_at > 45s` → 降级灰 `⚠ 已 50s 无活动`。
- 绑 `SwarmAgentState`：`error`/`exited` → **立即移除活态行**（不再挂 60s）。

### 8.4 派工卡 ↔ 焦点
- 点小团队树成员名 → 过滤流到该成员 / 跳右面板焦点（`onOpenAgent(agentId)`，复用 `:1295`）。
- 点活动时间线 `⌃▣` → 右面板终端 tab。

### 8.5 auto-mark-read（去抖）
- `IntersectionObserver` 观察每条未读 ORCH/MEMBER 气泡（`to_agent==='user' && read_at===null`）。
- 进视口 ≥ 50% 且停留 ≥ 800ms（去抖，防快速滚动误标）→ 批量 `markMessagesRead(USER_SENDER, ids)`（复用 `:455`）。
- 窗口失焦不标；重新聚焦时对当前视口内未读补标。
- 系统卡（wake/dispatch/...）**不计入未读徽章**（精确端点过滤），但仍可被读。

### 8.6 @定向子线程
- 用户输入 `@<成员>`（autocomplete 复用 Composer `:728-766`）→ 发送后开启子线程窗口。
- 该成员回复升格为成员色气泡，缩进 16px + 成员色细条。
- 收拢条件：队长重新对用户发言 **或** 60s 内无新 `@<同成员>` → 子线程收拢回普通流（成员后续消息回到 `MEMBER_SUPPRESSED`）。

### 8.7 键盘 / 可达性 aria
| 元素 | 键盘 | aria |
|---|---|---|
| 卡展开按钮 | `Enter`/`Space` 切换 | `aria-expanded`，`aria-controls` 指向正文区 id |
| 审批 `[批准]`/`[拒绝]` | Tab 可达，`Enter` 触发 | `aria-label="批准 {{action}}"`（不裸"批准"） |
| 失败卡命令 `[复制]` | `Enter` 复制 | `aria-live="polite"` 播报"已复制" |
| 时间分隔 | — | `role="separator"` |
| 未读分隔 | — | `role="separator" aria-label="N 条新消息"` |
| 活态行降级 | — | `aria-live="polite"` 播报降级一次（防刷屏） |
| 引用跳转 | `Enter` 跳转 | `aria-label="跳转到被引用的内容"` |
| 系统卡整体 | — | `role="article"`，`aria-label` 含 subtype 中文名 |
| 焦点环 | 全部交互元素 | `outline-ring/50`（复用 base layer `global.css:780`） |

### 8.8 防抖/阈值汇总
| 行为 | 阈值 |
|---|---|
| 时间分隔 | `GROUP_GAP_MS = 5min`（`:140`） |
| typing 行出现 | ≤ 1-2s |
| 活态行无活动降级 | 45s（用 `last_activity_at`） |
| 看门狗失败翻转 | 90s（`FIRST_RESPONSE_WATCHDOG_MS`，`rest.rs:33`） |
| auto-mark-read 去抖 | 视口停留 ≥ 800ms |
| @子线程收拢 | 60s 无新 @ 或队长接管 |
| 计时 tick | 500ms |
| 自动重试封顶 | 2-3 次后翻"需要你/换引擎" |

---

## 9. 验收标准（checklist）

**渲染正确性**
- [ ] 4 类渲染物各自形态正确：USER 右对齐 accent 实底 / ORCH 左对齐强对比正文 / MEMBER 仅升格时成员色气泡 / SYSTEM 带左色条+菱形徽章（非头像）。
- [ ] ORCH 正文用 `text-foreground-primary`，视觉权重强于 USER 气泡（诊断 1：截图对比队长气泡不再"隐形"）。
- [ ] ORCH 思考块 `status==='done'` 时**默认收起**，且视觉退后（不抢正文）。
- [ ] 全部 9 种 subtype 卡都有 collapsed/expanded 两态渲染，未知 subtype 降级中性卡不崩。
- [ ] 主轴限宽 720px 居中，两条短消息不贴顶留大空白（诊断 5）。

**物理隔离（firehose）**
- [ ] 成员的编辑/读/跑测试产物**绝不进流**——只在派工卡展开时间线或右面板可见。
- [ ] 成员气泡仅在三时刻升格（@定向 / 被队长引用 / 求助），其余 `MEMBER_SUPPRESSED`。
- [ ] N 个并行成员时，流里只多 N 张派工卡（折叠一行），无逐 token 交织。
- [ ] `wake reason=blackboard` 不渲染、不计未读。

**诚实性（"不许撒谎"硬断言）**
- [ ] 首个真实输出前**绝无绿色/"工作中"**——启动中徽章为琥珀 `◐启动中`（`--color-state-busy`），不是绿点。
- [ ] 成员 `AgentState` 翻 `error`/`exited` 后，活态行/typing 行**立即消失**，不挂 60s（诊断 2）。
- [ ] 45s 无活动诚实降级为灰 `⚠`，用真实 `last_activity_at`，非乐观假设。
- [ ] 失败卡显示**完整原因，永不截断**（诊断 6）；同一失败不在多处重复整段错误（单一真相源）。
- [ ] 刷新/断线重连后，所有系统卡从持久化重放，活动时间线先 `GET .../activity` 回填再接流（前提：端点走 DB——若后端缺口未补，活态行**诚实显示"重连中"**而非空白假装）。
- [ ] 90s 看门狗触发后启动清单卡**原地翻转**为失败态，不换视图、不静默。

**分组/未读**
- [ ] `>5min` 出时间分隔；同发送方连续消息合并头像。
- [ ] "N 条新消息"分隔只在首条未读前出现一次。
- [ ] 未读徽章用精确端点（系统噪声不计入）；auto-mark-read 进视口去抖标记，失焦不标。

**身份一致性（原则 7）**
- [ ] 同一成员在派工卡/活态行/子线程/交付卡归属处用同一 `roleColor` 同一角色派生名，**无 `worker_7` 裸 ID**。

**可达性**
- [ ] 所有交互元素 Tab 可达、有焦点环、`aria-expanded`/`aria-label`（按钮不裸"批准"，带动作上下文）。
- [ ] 审批/失败卡降级用 `aria-live="polite"` 播报一次，不刷屏。

**行话防火墙**
- [ ] 全部用户可见字符串无 mailbox/blackboard/wake/worktree/shim/spell/PTY/handoff/dispatch；用户动词经 `chat.activityVerb.*` 翻译（"写/读/跑测试"，不泄漏 worktree 路径）。
- [ ] i18n key 全部落 `messages.*` / `chat.systemCard.*` / `chat.activityVerb.*` / `common.*`，zh+en 双份。

---

**关键文件锚点（绝对路径）**
- 主组件：`/Users/wdx/opc/flockmux-core/web/src/components/MessagesPanel.tsx`（系统消息壳 `:1178-1201` 待替换；USER `:1211`；ORCH `:1278`；`buildRows` `:219`；TimeDivider `:1571`；NewMessagesDivider `:1587`；ReasoningDisclosure `:1606`；PendingBubble `:1670`；过滤 `:528`）
- 标记/markdown：`/Users/wdx/opc/flockmux-core/web/src/components/ChatMarkdown.tsx`
- 失败卡模板（待泛化）：`/Users/wdx/opc/flockmux-core/web/src/components/workspace/OrchestratorFailureCard.tsx`
- 身份：`/Users/wdx/opc/flockmux-core/web/src/lib/agent.ts:42-265`；小团队树边：`/Users/wdx/opc/flockmux-core/web/src/lib/dagEdgeDerivation.ts:51-95`
- 类型：`/Users/wdx/opc/flockmux-core/web/src/api/types.ts`（MessageMeta `:143-152`；MessageRecord `:154-168`；ThoughtTrace `:177-189`；AgentActivity `:454-462`；SwarmAgentState `:429-441`；AgentInfo `:81-135`）
- 后端 meta 模式（各 subtype emit 参照）：`/Users/wdx/opc/flockmux-core/crates/flockmux-server/src/wake.rs:389-396,849-858`
- 后端事实：`/Users/wdx/opc/flockmux-core/crates/flockmux-server/src/routes/rest.rs`（看门狗 `:33,711-743`；Spawning 同步发 `:670-672`；activity 端点 in-memory ring `:1125-1136`）；`/Users/wdx/opc/flockmux-core/crates/flockmux-storage/src/store.rs`（messages kind+meta `:1070`；last_activity_at `:438-452`；未读 `:1531-1575`）
- token：`/Users/wdx/opc/flockmux-core/web/src/styles/global.css`
- i18n：`/Users/wdx/opc/flockmux-core/web/src/i18n/locales/zh.json`、`en.json`

**新建文件（建议落点）**：`web/src/components/chat/SystemCard.tsx`、`web/src/components/chat/cards/*.tsx`、`web/src/components/chat/MemberSubThread.tsx`、`web/src/lib/activityVerb.ts`、`web/src/lib/messageClassify.ts`、`web/src/hooks/useAutoMarkRead.ts`
