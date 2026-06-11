I have everything I need. The i18n convention is nested namespaces (`chat.orchestratorFailure.*`, `task.*`), pluralization via `_one`/`_other`, interpolation via `{{var}}`. The existing copy still leaks "agent" but the redesign mandates the 行话防火墙 (队长/成员/会话). I now have all load-bearing facts verified:

- **Token system**: `--color-status-danger` (#B91C1C), `--color-status-warning`/`--color-state-busy` (#F59E0B/#B45309), `--color-state-success`/`--color-status-success`, `--color-foreground-primary/secondary/tertiary`, `--color-accent-primary`, radius `--radius-md/lg/xl/full`, fonts `--font-heading/body/caption/mono`.
- **Error taxonomy** (canonical, from `HealthNeedle.kind` + watchdog + shim exit): `auth | rate_limit | fatal` (HealthFail), `watchdog` (90s), crash via non-zero `ShimExit` → `AgentState::Error`.
- **Persistence**: `agents.last_error / last_error_kind / last_error_at` (migration 0022, persisted). AgentActivity phase=error is **live-only**. Bootstrap stage chain is **missing**. System-card-in-stream is **missing**.
- **Existing components**: `OrchestratorFailureCard.tsx` (auth/generic, 2 buttons), `TaskActivity.tsx` (pending/spawning/ready, auto-dismiss), `fallback_from` (`select_spawn_plugin`), `cliReadiness.installed/missing` precheck.

Now I'll write the spec.

I have all the load-bearing facts verified. Here is the implementable component spec.

---

# 组件区规格：空 / 启动 / 失败状态机（诚实三态）

> 对齐文档：`/Users/wdx/opc/flockmux-core/.claude/worktrees/funny-raman-47113f/.ux-review/chat-redesign.md` §4.6（空/启动/失败状态）+ §2 原则1（事实律）、原则2（入流律）。
> 行话防火墙：本规格所有用户可见字符串禁用 mailbox/blackboard/wake/worktree/shim/spell/PTY/handoff，统一用 **队长/成员/会话/计划/变更/推进/引擎/分支**。

---

## 1. 目的与边界

**解决的诊断**（来自 §1）：
- **诊断 2「等待期撒谎」**：`PendingBubble` 在成员静默死亡后仍挂 60s；首响应前显示假绿点/转圈。→ 本组件在「首个真实输出前绝不出现绿色」，并把后端已有的 `AgentState::Error` + 90s 看门狗（`rest.rs:33,711-730` 真的 fire `record_agent_error("watchdog")`）接到 UI。
- **诊断 5「空状态浪费教学时机」**：裸「暂无对话」居中漂浮。→ 能力揭示问候 + 3 starter + 引擎预检 + 右面板自我说明。
- **诊断 6「失败信息双份、措辞不一致、被截断」**：失败既进失败卡又进成员栏徽章且被截断成「请在终… 0:00」。→ 单一真相源 + 完整不截断 + 永不消失。

**解决的原则**：原则1（事实律：状态只描述服务器证实的事实）、原则2（入流律：失败/启动以可重放事件进流）。

**不做（边界）**：
- 不做对话气泡渲染（队长/成员/你的消息）——那是「消息流 4 类渲染物」组件区（§4.1）。
- 不做「现在发生什么」的 two-signal 活态行——那是 §4.2 组件区。本组件只负责**会话尚无真实对话内容时**填满主轴（空/启动/失败），一旦队长产出首条真实消息，主轴交还给消息流组件。
- 不做派工/交付/审批/diff 卡——§4.3。
- 不做 Composer——§4.4，但本组件需消费 Composer 的「首次发送」事件作为「空→启动」的转换触发点。
- 不实现后端新表/新事件。凡依赖 missing 信号处，本规格明确标注「依赖后端新增 X」并给出 P0 可用的降级实现（用现有时间戳推断）。

**渲染位置**：本组件是 `MessagesPanel` 的 `emptyStateOverride`（`Chat.tsx:862-873` 已有此插槽）的扩展——把当前「仅 OrchestratorFailureCard 或 undefined」二选一，升级为「空 / 启动 / 失败」三态状态机。失败卡同时**入流为一张持久化系统卡**（P1，见数据绑定表）。

---

## 2. 完整状态枚举

状态机由「会话是否有真实队长输出」+「队长 agent 的 `AgentState`」+「bootstrap 阶段」+「`last_error*`」共同决定。优先级从高到低（高优先级覆盖低）：

| # | 状态 ID | 触发条件（数据） | 视觉 |
|---|---|---|---|
| **空态族** | | | |
| E0 | `empty.ready` | 会话无队长消息 且 至少一个引擎 `installed` 且 无 orchestrator agent | 问候 + 3 starter + 引擎预检（全✓）+ 右面板自我说明 |
| E1 | `empty.no_engine` | 会话无队长消息 且 `cliReadiness.installed.length===0 && missing.length>0` | 问候降级 + starter 禁用 + 引擎预检（全✕，带安装指引）|
| E2 | `empty.precheck_loading` | `cliReadiness.loading===true` | 问候 + 引擎预检骨架行（spinner，**非绿**）|
| **启动态族** | | | |
| B0 | `boot.start` | 用户首次发送后、orchestrator 已 spawn、未收首响、未失败 | 启动清单卡：4 步逐步消费，徽章 `◐启动中`（**非绿**）|
| B1 | `boot.step_isolating` | bootstrap 阶段=隔离分支 | 清单第1步 `◐`，其余 `○` |
| B2 | `boot.step_auth` | bootstrap 阶段=引擎登录校验 | 第1步`✓` 第2步`◐` |
| B3 | `boot.step_engine` | bootstrap 阶段=启动引擎 | 第1-2`✓` 第3`◐`（带计时秒数）|
| B4 | `boot.step_first_response` | 引擎 ready、等首响（看门狗计时中）| 第1-3`✓` 第4`◐` |
| **失败态族（永不消失）** | | | |
| F-auth | `fail.auth` | `last_error_kind==='auth'` 或 reason 命中 `/未登录\|not logged in\|\/login/i` | 失败卡 + 登录命令可复制 + `[打开终端登录][重试][换引擎][看日志]` |
| F-rate | `fail.rate_limit` | `last_error_kind==='rate_limit'` | 失败卡 + 倒计时/稍后重试 + `[稍后重试][换引擎][看日志]` |
| F-crash | `fail.crash` | `last_error_kind==='fatal'` 或 非零 `ShimExit`→`AgentState::Error` | 失败卡 + `[重试][换引擎][看日志]` |
| F-timeout | `fail.timeout` | `last_error_kind==='watchdog'`（90s 无响应）| 失败卡 + `[重试][打开终端登录][换引擎][看日志]` |
| **边缘/降级** | | | |
| X0 | `recovered` | `freshSignalAt > last_error_at`（恢复守卫，`Chat.tsx:466-471` 已有逻辑）| 失败卡**消失**，回退到正常流（不属本组件）|
| X1 | `boot.watchdog_pending` | B4 计时已过 60s 但未到 90s | 第4步文案加「响应较慢…」灰字提示，徽章仍 `◐`（不翻失败）|
| X2 | `fail.retry_in_flight` | 任一失败态点 `[重试]` 后 | 失败卡保留 + 顶部加 `◐ 重试中（第 N/3 次）` 行，按钮禁用 |
| X3 | `fail.escalated` | 自动/手动重试达 2-3 次上限仍失败 | 失败卡文案升级为「多次重试未成功」，弱化 `[重试]`，突出 `[换引擎][打开终端登录]` |
| X4 | `empty.partial_engine` | 部分引擎装了部分没装（`installed>0 && missing>0`）| E0 行为 + 引擎预检逐行混合 ✓/✕ |

**关键不变量**：E* 和 B* 互斥（发送即从 E→B）；F* 覆盖 B*（任一 bootstrap 步失败立即翻 F，原地，不换视图）；X0 覆盖 F*（恢复守卫优先于失败）。

---

## 3. 逐状态 ASCII 线框

所有卡片宽度 `max-w-[460px]`（沿用 `OrchestratorFailureCard` 的 `max-w-[460px]`），居中 `mx-auto`。

### E0 — 空态·就绪（1280px 主轴居中）

```
              对话主轴 max-w≈720 居中
┌─────────────────────────────────────────────────┐
│                                                   │  ← 顶部留白 mt-16
│   ┌队┐  我是这个会话的队长                         │  问候区
│   └──┘  说出你想做的事，我会拆成计划、              │  heading 18px/600
│         派给成员推进，并在这里向你汇报。            │  body 14px 次级色
│                                                   │
│   试试这样开始：                                   │  caption 12px 三级色
│   ┌───────────────────────────────────────────┐  │  starter ×3
│   │ 重构这个函数，抽出可复用逻辑            ↵ │  │  每个 44px 高
│   ├───────────────────────────────────────────┤  │  hover accent 边
│   │ 给这段代码补上失败用例的测试            ↵ │  │  点击填入 Composer
│   ├───────────────────────────────────────────┤  │
│   │ 帮我查这个 bug 的根因                   ↵ │  │
│   └───────────────────────────────────────────┘  │
│                                                   │
│   ─────────────────────────────────────────────  │  hairline 分隔
│   引擎就绪                                        │  precheck 区
│   ✓ Claude Code 已登录                            │  ✓绿 / ✕红
│   ✓ Codex 已登录                                  │
│                                                   │
└─────────────────────────────────────────────────┘

右面板空态（脉搏条/工作面板，64px 或常驻）：
┌────────────────┐
│  还没有成员     │  ← 居中 caption
│  开始后，这里   │
│  会实时显示     │
│  每个成员       │
│  正在做什么     │
└────────────────┘
```

### E1 — 空态·无引擎

```
┌─────────────────────────────────────────────────┐
│   ┌队┐  队长还没法上岗                             │  heading
│   └──┘  需要先装好并登录至少一个 AI 引擎。         │  body
│                                                   │
│   ┌─ ✕ 没有可用引擎 ───────────────────────────┐ │  status-danger 软底
│   │ Claude Code 未安装                          │ │
│   │ npm i -g @anthropic-ai/claude-code  [复制]  │ │  mono 命令 + 复制
│   │ 装完后 claude /login 登录                    │ │
│   │ ─────────────────────────────────           │ │
│   │ Codex 未安装                                 │ │
│   │ npm i -g @openai/codex              [复制]  │ │
│   └─────────────────────────────────────────────┘ │
│                                                   │
│   ［我装好了，重新检测］                           │  outline 按钮
│   （starter prompt 区灰显禁用，tooltip：先装引擎） │
└─────────────────────────────────────────────────┘
```

### B0–B4 — 启动清单卡（原地逐步消费）

```
┌─ ◐ 队长正在上岗… ──────────────────────  3 步完成 ─┐  卡头：徽章◐(busy色) + 进度
│                                                     │  圆角 radius-xl
│  ✓ 已隔离到分支  main ↘ 退款流程                    │  ✓ state-success
│  ✓ Claude Code 已登录                               │  ✓ state-success
│  ◐ 正在启动队长引擎…                          8s    │  ◐ busy色 旋转 + 计时mono
│  ○ 等待队长第一次响应                                │  ○ idle色 空心
│                                                     │
└─────────────────────────────────────────────────────┘

X1（B4 计时 > 60s，未到 90s）：第4步行附加灰字
│  ◐ 等待队长第一次响应… 响应较慢，再等等       72s   │
```

每步 4 行，行高 28px，左侧 16px 图标列对齐。卡总高随步数，约 180px。

### F-auth — 失败卡·未登录（永不消失）

```
┌─ ✕ 队长还没法开始 ─────────────────────────────────┐  status-danger 边+软底
│                                                     │  radius-2xl
│  ⚠ Claude Code 未登录                               │  TriangleAlert + reason
│    （完整 reason，可换行，绝不截断）                 │  整段服务器原文
│                                                     │
│  ┌ 在终端运行 ──────────────────────────────────┐  │  surface 内嵌行
│  │ claude /login                         [复制] │  │  mono 命令 + 复制
│  └───────────────────────────────────────────────┘  │
│                                                     │
│  [打开终端登录]  [重试]  [换 Codex 重试]  [看日志]  │  按钮条 flex-wrap
└─────────────────────────────────────────────────────┘
```

### F-rate — 失败卡·额度受限

```
┌─ ✕ 队长暂时被限流 ─────────────────────────────────┐
│  ⚠ Claude Code 触发用量上限                         │
│    （reason 原文，含官方建议时间）                   │
│                                                     │
│  [稍后重试]  [换 Codex 重试]  [看日志]              │  无登录命令行
└─────────────────────────────────────────────────────┘
```

### F-crash — 失败卡·异常退出

```
┌─ ✕ 队长意外退出 ───────────────────────────────────┐
│  ⚠ 引擎进程异常退出（退出码 1）                      │
│    （reason 原文 / 最后一段错误输出，不截断）        │
│                                                     │
│  [重试]  [换 Codex 重试]  [看日志]                  │
└─────────────────────────────────────────────────────┘
```

### F-timeout — 失败卡·启动后无响应（90s 看门狗）

```
┌─ ✕ 队长启动后没有响应 ─────────────────────────────┐
│  ⚠ 启动后 90 秒无响应（可能未登录或卡住）           │  watchdog reason 原文
│                                                     │
│  [重试]  [打开终端登录]  [换 Codex 重试]  [看日志]  │  watchdog 兼顾 auth 入口
└─────────────────────────────────────────────────────┘
```

### X2 / X3 — 重试中 / 升级

```
X2（重试进行中，按钮禁用）：
┌─ ✕ 队长还没法开始 ─────────────────────────────────┐
│  ◐ 重试中（第 2/3 次）…                       4s    │  顶部插入一行 busy
│  ⚠ Claude Code 未登录                               │
│  …（按钮整排 disabled + 半透明）                     │
└─────────────────────────────────────────────────────┘

X3（达上限，升级文案）：
┌─ ✕ 多次重试仍未成功，需要你处理 ───────────────────┐
│  ⚠ Claude Code 未登录（已自动重试 3 次）            │
│  ┌ 在终端运行  claude /login           [复制] ┐    │
│  [打开终端登录]  [换 Codex 重试]  · 重试  · 看日志  │  突出登录/换引擎，弱化重试
└─────────────────────────────────────────────────────┘
```

---

## 4. 精确中文文案 + i18n key

i18n 约定（沿用 `zh.json` 现状）：嵌套命名空间、插值 `{{var}}`、复数 `_one/_other`。**新增命名空间 `chat.bootstrap.*` 和 `chat.emptyState.*`，扩展 `chat.orchestratorFailure.*`。** 所有字符串走 `t(key, fallback)` 双参形式（与 `OrchestratorFailureCard` 现状一致）。

### 空态 `chat.emptyState.*`

| key | 中文 |
|---|---|
| `chat.emptyState.greetingTitle` | `我是这个会话的队长` |
| `chat.emptyState.greetingBody` | `说出你想做的事，我会拆成计划、派给成员推进，并在这里向你汇报。` |
| `chat.emptyState.starterHint` | `试试这样开始：` |
| `chat.emptyState.starter.refactor` | `重构这个函数，抽出可复用逻辑` |
| `chat.emptyState.starter.test` | `给这段代码补上失败用例的测试` |
| `chat.emptyState.starter.bug` | `帮我查这个 bug 的根因` |
| `chat.emptyState.precheckTitle` | `引擎就绪` |
| `chat.emptyState.engineLoggedIn` | `{{name}} 已登录` |
| `chat.emptyState.engineMissing` | `{{name}} 未安装` |
| `chat.emptyState.engineLoading` | `正在检测引擎…` |
| `chat.emptyState.noEngineTitle` | `队长还没法上岗` |
| `chat.emptyState.noEngineBody` | `需要先装好并登录至少一个 AI 引擎。` |
| `chat.emptyState.noEngineCardTitle` | `没有可用引擎` |
| `chat.emptyState.installHint` | `装完后 {{loginCommand}} 登录` |
| `chat.emptyState.recheck` | `我装好了，重新检测` |
| `chat.emptyState.starterDisabledTip` | `先装好并登录一个引擎` |
| `chat.emptyState.panelEmpty` | `开始后，这里会实时显示每个成员正在做什么` |
| `chat.emptyState.panelEmptyTitle` | `还没有成员` |

### 启动 `chat.bootstrap.*`

| key | 中文 |
|---|---|
| `chat.bootstrap.cardTitle` | `队长正在上岗…` |
| `chat.bootstrap.badge` | `启动中` |
| `chat.bootstrap.progress_one` | `还差 {{count}} 步` |
| `chat.bootstrap.progress_other` | `还差 {{count}} 步` |
| `chat.bootstrap.stepIsolate` | `隔离到分支 {{base}} ↘ {{branch}}` |
| `chat.bootstrap.stepIsolateDone` | `已隔离到分支 {{base}} ↘ {{branch}}` |
| `chat.bootstrap.stepAuth` | `校验 {{name}} 登录` |
| `chat.bootstrap.stepAuthDone` | `{{name}} 已登录` |
| `chat.bootstrap.stepEngine` | `正在启动队长引擎…` |
| `chat.bootstrap.stepEngineDone` | `队长引擎已启动` |
| `chat.bootstrap.stepFirstResponse` | `等待队长第一次响应` |
| `chat.bootstrap.slowHint` | `响应较慢，再等等` |

### 失败 `chat.orchestratorFailure.*`（扩展现有命名空间）

复用现有：`title`、`runInTerminal`、`openTerminalLogin`、`openTerminal`、`retry`、`retrying`、`common.copy`、`common.copied`。新增：

| key | 中文 |
|---|---|
| `chat.orchestratorFailure.titleAuth` | `队长还没法开始` |
| `chat.orchestratorFailure.titleRate` | `队长暂时被限流` |
| `chat.orchestratorFailure.titleCrash` | `队长意外退出` |
| `chat.orchestratorFailure.titleTimeout` | `队长启动后没有响应` |
| `chat.orchestratorFailure.titleEscalated` | `多次重试仍未成功，需要你处理` |
| `chat.orchestratorFailure.retryLater` | `稍后重试` |
| `chat.orchestratorFailure.switchEngine` | `换 {{name}} 重试` |
| `chat.orchestratorFailure.viewLog` | `看日志` |
| `chat.orchestratorFailure.retryingNth` | `重试中（第 {{n}}/{{max}} 次）…` |
| `chat.orchestratorFailure.escalatedSuffix` | `（已自动重试 {{n}} 次）` |
| `chat.orchestratorFailure.exitCode` | `引擎进程异常退出（退出码 {{code}}）` |

**行话防火墙自检**：以上无 mailbox/blackboard/wake/worktree/shim/spell/PTY/handoff。「隔离到分支」用「分支」而非 worktree；「换引擎重试」而非 fallback；「队长/成员」而非 orchestrator/agent。注意 `zh.json` 现状仍含「agent」「调度」「编排」等词（如 `chat.memberCount`、`chat.runSpell`），本组件新增 key 不沿用这些，且建议附带任务清理旧 key（不在本组件范围）。

---

## 5. 尺寸 / 间距 / 色彩 token

全部引用 `web/src/styles/global.css` 的现有 CSS 变量，**不新增 token**。

| 元素 | token / 值 |
|---|---|
| **失败卡容器** | `border border-status-danger/30 bg-status-danger/5`，`rounded-2xl`(=`--radius-xl` 12px 偏大，沿用现卡 `rounded-2xl`)，`px-5 py-4 gap-3`，`max-w-[460px] mx-auto` |
| **启动清单卡容器** | `border border-accent-primary/30 bg-accent-primary-soft`(`--color-accent-primary-soft` #DBEAFE)，`rounded-xl`(`--radius-xl`)，`px-5 py-4`，`max-w-[460px]` |
| **空态容器** | 无边框，`max-w-[460px] mx-auto mt-16 flex flex-col gap-6` |
| **✓ 已完成图标/文字** | `text-state-success`(`--color-state-success` #16A34A) / 软底用 `--color-status-success-soft` |
| **◐ 进行中** | `text-accent-primary-deep`(#1D4ED8) `animate-spin`；启动徽章用 `--color-status-busy`(#B45309) on `--color-status-busy-soft` —— **关键：非绿** |
| **○ 未开始** | `text-status-idle`/`--color-state-idle`(#94A3B8) 空心圆 |
| **✕ / ⚠ 失败** | `text-status-danger`(`--color-status-danger` #B91C1C) |
| **限流橙** | `text-status-warning`(#B45309) on `--color-status-warning-soft`(#FEF3C7) |
| **卡标题** | `font-heading text-sm font-semibold text-foreground`(`--color-foreground-primary`) |
| **reason 正文** | `font-caption text-xs leading-5 text-foreground-secondary`，**不加 `truncate`/`line-clamp`**（防截断硬约束）|
| **starter 行** | `min-h-11`(44px 可点区)，`rounded-lg`(`--radius-lg` 8px)，`border-border-subtle`，hover `border-accent-primary/40 bg-accent-primary-soft/50`，文字 `font-body text-sm text-foreground` |
| **mono 命令行** | `font-mono text-xs text-foreground`，容器 `rounded-lg border-border-subtle bg-surface px-3 py-2` |
| **计时数字** | `font-mono text-[10px] text-foreground-tertiary` |
| **引擎预检行** | `font-caption text-xs`，✓行 `text-foreground-secondary`，✕行 `text-status-danger` |
| **按钮** | 复用 `Button` (`@/components/ui/button`) `size="sm" h-8 gap-1.5`，主按钮实底，次按钮 `variant="outline"`，第三+ `variant="ghost"` |
| **成员色头像** | `--color-agent-*`（planner/test/backend…），复用 `roleColorClass`/`resolveMemberVisual`(`web/src/lib/agent.ts`) |
| **图标库** | `lucide-react`：`TriangleAlert/Copy/Check/LogIn/RotateCw/SquareTerminal/Loader2/Sparkles/CircleDashed/FileText` |

---

## 6. 数据绑定表

每个动态元素 ← 信号来源 / 持久化级别 / 引用。**missing 处明确标注后端依赖。**

| UI 元素 | 数据源 | 持久化 | 引用 file:line | 备注 |
|---|---|---|---|---|
| **空态触发**（无队长消息）| `messages` 为空 / 无 orchestrator agent | persisted | `MessagesPanel` emptyState 插槽 `Chat.tsx:862` | 已有插槽，扩展为三态 |
| **引擎预检 ✓/✕** | `cliReadiness.installed[] / missing[]`（`CliPluginInfo`）| live（GET /api/plugins，前端 state）| `Chat.tsx:200-203,391-394` | 已有；E0/E1/E2 直接读 |
| **登录命令** | `cliReadiness.installed.find(p=>p.id===cli).install.login_command` | live | `Chat.tsx:479-481`, `types.ts:54` | 已有 |
| **starter prompt 点击→填 Composer** | 静态 i18n + Composer setBody | n/a | 新建（onPickStarter 回调）| Composer 接线 |
| **启动清单卡·隔离步** | `thread.isolation/branch`（`ThreadRecord`）+ ❗bootstrap 阶段事件 | **missing**（阶段链）| `models.rs:77-78` 有 branch；阶段链无 | **依赖后端新增**：spawn 时同步落 `kind='system', meta={subtype:'bootstrap', stage:'isolate'\|'auth'\|'engine'\|'first_response', status}`。P0 降级：用 `spawned_at`(`store.rs:416`) + 首个 `AgentActivity.at` + `last_error_at` 推断阶段，不读真实链 |
| **启动清单卡·登录步** | bootstrap 阶段 `stage:'auth'` | **missing** | 同上 | P0 降级：spawn 后立即标 ✓（HealthFail 才翻 F-auth）|
| **启动清单卡·启动引擎步** | `AgentState::Spawning→Ready`（`ShimReady`）| persisted（state 转换）live（WS）| `rest.rs:670-672,687-695`；`ws_swarm.rs:102-118` | Spawning 事件**同步发**，不等 tailer。计时=now−spawned_at |
| **启动清单卡·等首响步** | 首个非 system `AgentActivity` 或首条队长 message | live | `rest.rs:687-695`；`ws_swarm.rs:74-97` | 收到即整卡消失，交还消息流 |
| **首卡占位即时落地** | spawn 同步发 `SwarmEvent::AgentState{Spawning}` | persisted | `rest.rs:670-672` | ❗P0：建议 spawn 同步落一条 `kind='system', meta={subtype:'bootstrap_start'}` 占位 message，保证刷新可重放（**依赖后端新增 1 个 emit 点**）|
| **90s 看门狗→F-timeout** | `record_agent_error(reason,"watchdog")` + `AgentState::Error` + `AgentActivity{phase:'error'}` | persisted（`last_error*`）+ live（事件）| `rest.rs:33,711-743` | 已实现且真 fire。前端绑 `AgentState::Error` |
| **失败 reason（单一真相源）** | `agents.last_error`（持久）优先回退；`AgentActivity.label`(phase=error) 即时 | last_error: **persisted**（迁移0022）；activity: **live-only** | `models.rs:52-65`；`rest.rs:716-743,786-803`；`Chat.tsx:454-458` | 现 `orchestratorFailure` 已 liveErr ?? last_error。**不截断** |
| **失败 kind** | `agents.last_error_kind`（`auth\|rate_limit\|fatal\|watchdog`）| persisted | `types.ts:129-134`；`plugins.rs:180-184`（HealthNeedle kind）；`Chat.tsx:476-478` | F-crash 额外来自非零 `ShimExit`(`rest.rs:766-770`) |
| **失败 at（恢复守卫）** | `agents.last_error_at` vs `last_activity_at`/`activity.at` | persisted | `Chat.tsx:466-471`；`store.rs:438-444` | X0 已有逻辑，复用 |
| **失败卡入流（可重放）** | ❗`messages kind='system', meta={subtype:'agent_error', reason, kind}` | **missing** | `store.rs:1070`（kind+meta 框架已有）| **依赖后端新增**：失败时同步 INSERT 一条系统消息。P0 降级：仅靠 `agents.last_error*`（刷新可见，但不在流内位置固定）|
| **换引擎（fallback_from）** | `select_spawn_plugin` 的 fallback；重 spawn 用另一 installed cli | live | `rest.rs:568-577,3150-3162` | `[换 X 重试]` = kill 当前 + spawn(`cliReadiness.installed` 中另一个) |
| **重试** | `api.killAgent` + 重新 init/spawn | live | `Chat.tsx:486-495`（retryOrchestrator 已有）| 自动重试计数器**前端 state**（live-only），上限 2-3 |
| **看终端/看日志** | `openAgent(agentId)` → AgentDrawer terminal tab | n/a | `Chat.tsx:868`；`AgentDrawer.tsx:82-92` | 已有 |
| **接管/交还** | AgentDrawer 终端 `[接管]/[交还]` | — | `AgentDrawer.tsx`（pause/resume 区）；`rest.rs:1806,1824`（interrupt/resume）| 端点已有，本组件仅引导跳转 |
| **乐队栏失败点（不重复）** | `AgentState::Error` → ✕ 点 + tooltip（reason）| persisted | `ws_swarm.rs:102-118` | **只显点+tooltip，整段错误只在流内卡讲一次**（治诊断6）|

**汇总后端依赖（按 P 分级）**：
- **P0（本组件最低可用，0 新表）**：spawn 同步落 `bootstrap_start` 占位 system message（1 个 emit 点）；其余用现有 `spawned_at`/`AgentState`/`last_error*` 时间戳推断启动清单 + 失败态。看门狗、`last_error*`、`AgentState` 七态、`fallback_from`、interrupt/resume **全部已存在**。
- **P1（兑现入流律/重放）**：① bootstrap 阶段链落库（`meta.subtype='bootstrap', stage`）；② 失败事件落 `kind='system', meta.subtype='agent_error'`。两者均复用 `messages.kind+meta`（`store.rs:1070`，迁移 0012），**无需新表**。

---

## 7. 复用 vs 新建

| 动作 | 对象 |
|---|---|
| **复用·改** | `OrchestratorFailureCard.tsx`（`web/src/components/workspace/`）：泛化为 `FailureCard`——`kind` 从 `auth\|generic` 扩到 `auth\|rate_limit\|crash\|timeout\|escalated`；按钮从 2 个扩到 4 个（加 `[换引擎][看日志]`）；title 按 kind 切换；新增 `retryCount/retryMax/retrying/onSwitchEngine/onViewLog/engineCandidates` props。`isAuth` 判定逻辑(`:58-59`)保留 |
| **复用·改** | `TaskActivity.tsx`：抽出其 `TaskRow` 的计时 tick（`useEffect setInterval 500ms`，`:62-67`）与状态图标映射模式，作为启动清单卡每步的计时来源。**改**：从「pending/spawning/ready 自动消失」语义改为「4 步清单 + 可翻失败」——不复用其 auto-dismiss（失败永不消失与之冲突）|
| **复用·原样** | `Button`(`@/components/ui/button`)、`cn`(`@/lib/cn`)、`roleColorClass/resolveMemberVisual`(`web/src/lib/agent.ts`)、`cliReadiness` state + `orchestratorFailure` memo（`Chat.tsx:200-203,446-483`）、`retryOrchestrator`(`Chat.tsx:486`)、`openAgent`、lucide 图标、复制逻辑（`OrchestratorFailureCard.tsx:61-70`）|
| **复用·原样** | `Chat.tsx:862-873` 的 `emptyStateOverride` 插槽机制——把当前 `orchestratorFailure ? <Card> : undefined` 二元，换成 `<EmptyBootFailStateMachine state={...}/>` |
| **新建** | `EmptyState.tsx`（问候 + starter ×3 + 引擎预检 + 右面板自我说明文案）|
| **新建** | `BootstrapChecklistCard.tsx`（4 步清单卡，消费 bootstrap 阶段/时间戳推断，原地可翻 `FailureCard`）|
| **新建** | 状态机选择器 hook `useChatLifecycleState(...)`：输入 `{ hasOrchestratorMessage, cliReadiness, orchestratorAgent, agentState, bootstrapStage, retryState }`，输出当前 `state ID`（§2 枚举），供 `emptyStateOverride` 渲染对应组件。把现在散在 `Chat.tsx` 的 `orchestratorFailure` memo 收编进来 |
| **新建·前端 state** | 自动重试计数器（per-orchestrator，封顶 2-3，live-only）；引擎候选列表（`cliReadiness.installed` 排除当前 cli）|
| **新建·后端（P1）** | bootstrap 阶段 emit + 失败 system message emit（复用 `wake.rs:389,849` 的 `kind+meta+broadcast+store` 模式）|

---

## 8. 交互与时序

### 状态转换图（事件 → 转换）

```
E2(precheck loading) ──cliReadiness loaded, installed>0──▶ E0
E2 ──installed==0 && missing>0──▶ E1
E0/E1 ──Composer 首次发送 + spawn orchestrator──▶ B0
B0 ──AgentState:Spawning(rest.rs:670)──▶ B3(启动引擎, ◐计时)
B3 ──ShimReady→AgentState:Ready(rest.rs:687)──▶ B4(等首响)
B4 ──首个 AgentActivity / 首条队长 message──▶ [退出本组件, 交还消息流]
B4 ──60s 计时 still no response──▶ X1(B4 + 慢字)
任一 B* ──HealthFail(auth/rate_limit/fatal) / ShimExit≠0 / 90s watchdog──▶ F-*  (原地翻转, 不换视图)
F-* ──[重试] click──▶ X2(重试中, 按钮禁用) ──spawn ok & first response──▶ 退出
X2 ──重试失败 & count<max──▶ F-*（计数+1）
X2/F-* ──count>=max(2~3)──▶ X3(escalated)
F-* ──freshSignalAt > last_error_at(用户终端 /login 成功)──▶ X0(recovered, 卡消失)
```

### 阈值 / 计时

- **首卡占位**：spawn 后 **≤1s** 必须出现 B0（同步 `Spawning` 事件，**不等 700ms tailer**）。
- **启动引擎步计时**：`now − spawned_at`，每 500ms tick（复用 `TaskActivity` 的 interval）。
- **慢提示阈值 X1**：B4 计时 > **60s** 加灰字「响应较慢」；徽章仍 `◐`，**不翻失败**。
- **看门狗 90s**：后端 `FIRST_RESPONSE_WATCHDOG_MS=90_000`（`rest.rs:33`）→ 前端收 `AgentState::Error(kind=watchdog)` 翻 F-timeout。前端不自行计 90s（避免与后端双源），以后端事件为准（事实律）。
- **自动重试**：失败后**不自动重试**首次（避免静默 fix-loop，原则2/rAutonomous），由用户点 `[重试]`；若启用自动，封顶 **3** 次，每次入流一行 X2，达上限翻 X3。
- **复制反馈**：复制后 1500ms 回弹（复用 `OrchestratorFailureCard.tsx:66`）。
- **防抖**：`[重试]/[换引擎]` 点击后立即 `disabled`（X2），防连点重复 spawn（复用 `retrying` 守卫 `Chat.tsx:487`）。

### 键盘

- starter prompt：`Tab` 可聚焦，`Enter`/`Space` 触发填入 Composer。
- 失败卡按钮：原生 `<button>`/`Button` Tab 序，主按钮（`[打开终端登录]`/`[重试]`）排首位获得初始焦点。
- 复制行：`Enter` 触发复制。
- 全局：不抢占 Composer 焦点（用户随时能打字覆盖空态）。

### 可达性（aria）

| 元素 | aria |
|---|---|
| 启动清单卡 | `role="status" aria-live="polite"`，每步 `aria-label` 含状态（如「已完成：已登录」「进行中：启动队长引擎」）|
| 失败卡 | `role="alert" aria-live="assertive"`（失败需立即播报）|
| ◐ 旋转图标 | `aria-hidden="true"`（语义在文字，不让屏读念「旋转」）|
| starter 行 | `role="button" aria-label="使用这个开头：{文本}"` |
| 禁用 starter（E1）| `aria-disabled="true"` + `aria-describedby` 指向「先装引擎」提示 |
| 引擎预检行 | `aria-label="{name} {已登录/未安装}"` |
| 复制按钮 | `aria-label="复制命令"`，复制后 `aria-live` 播报「已复制」|
| 进度徽章 | 文字徽章而非纯色点，确保色盲可读（`◐启动中`/`✕卡住` 带字）|

---

## 9. 验收标准（含「不许撒谎」诚实性断言）

### 诚实性断言（硬约束，CR 必查）

- [ ] **H1 无假绿点**：B0–B4 任何时刻徽章为 `◐启动中`（busy/accent 色），**绝不出现 `--color-state-success` 绿**，直到收到首个真实 `AgentActivity`/队长 message。
- [ ] **H2 死即翻牌**：orchestrator `AgentState` 变 `Error`（HealthFail/ShimExit≠0/watchdog）后，启动清单卡**原地**翻为对应 F-* 卡，**≤1 个事件周期**内完成，不残留转圈、不换视图。
- [ ] **H3 单一真相源**：同一失败，整段 reason **只在流内失败卡出现一次**；乐队栏/成员栏只显 `✕` 点 + tooltip，**不重复整段错误**。
- [ ] **H4 不截断**：失败卡 reason 容器无 `truncate`/`line-clamp`/固定高度溢出隐藏；长 reason 完整换行可读（治「请在终… 0:00」）。
- [ ] **H5 永不消失**：F-* 卡在未处理前不自动消失、不被新消息挤走（钉在流相应位置）；仅 X0 恢复守卫（`freshSignalAt > last_error_at`）可清除。
- [ ] **H6 计时来自真实戳**：启动计时 = `now − spawned_at`（真实 spawn 时刻），非前端凭空起算。

### 功能验收

- [ ] E0：无对话时显示问候 + 3 个可点 starter，点击填入 Composer（不直接发送）。
- [ ] E0：引擎预检逐行显示 `✓{name} 已登录` / `✕{name} 未安装`，数据来自 `cliReadiness`。
- [ ] E1：无任何引擎时 starter 禁用 + tooltip，显示安装命令 + 复制 + `[重新检测]`。
- [ ] E2：precheck loading 显示 spinner 行（非绿）。
- [ ] B0 在 spawn 后 ≤1s 出现（首卡占位即时落地）。
- [ ] B 步序：隔离✓ → 登录✓ → 启动引擎◐(计时) → 等首响○，逐步推进。
- [ ] B4 > 60s 出现「响应较慢」灰提示，徽章仍 `◐`。
- [ ] 收到首个真实输出，启动卡消失，主轴交还消息流。
- [ ] F-auth：显示 `claude /login` 可复制；`[打开终端登录][重试][换引擎][看日志]` 四键齐。
- [ ] F-rate/F-crash/F-timeout：各自 title + 按钮集正确（rate 无登录命令行；timeout 含登录入口）。
- [ ] `[换 X 重试]`：候选取自 `cliReadiness.installed` 中非当前 cli；无候选时该键隐藏。
- [ ] `[重试]`：点击后禁用整排按钮 + 显示 `◐ 重试中（第 N/max 次）`；成功则退出，失败计数+1。
- [ ] 重试达 2-3 次上限 → X3 升级文案，弱化 `[重试]`、突出 `[换引擎][打开终端登录]`。
- [ ] `[看日志]`/`[打开终端登录]` 跳 AgentDrawer 对应 tab（`openAgent`）。
- [ ] X0 恢复：用户在终端 `/login` 成功产出新活动后，失败卡自动消失（复用 `Chat.tsx:466-471`）。
- [ ] 刷新页面：F-* 卡靠 `agents.last_error*` 重建（persisted）；B* 靠 `AgentState`/`spawned_at` 重建（P1 落库后流内位置稳定）。

### 行话防火墙验收

- [ ] 全部新增 i18n value `grep -iE 'mailbox|blackboard|wake|worktree|shim|spell|PTY|handoff'` 命中 0 条。
- [ ] 使用「队长/成员/会话/计划/变更/推进/引擎/分支」术语；无 `worker_N` 裸 ID（成员名走 `resolveMemberVisual`）。

### 可达性验收

- [ ] 启动卡 `role="status" aria-live="polite"`；失败卡 `role="alert" aria-live="assertive"`。
- [ ] 所有状态用「图标+文字徽章」表达，去色后仍可辨（色盲）。
- [ ] starter / 按钮 / 复制行键盘可达，Tab 序合理，主操作初始聚焦。

---

**关键文件锚点（绝对路径）**：
- 改：`/Users/wdx/opc/flockmux-core/web/src/components/workspace/OrchestratorFailureCard.tsx`（泛化为 FailureCard，:58-59 isAuth、:61-70 复制、:73 容器、:109-132 按钮条）
- 复用计时：`/Users/wdx/opc/flockmux-core/web/src/components/TaskActivity.tsx`（:20 TaskStatus、:62-67 tick、:37-41 图标映射）
- 接线点 + 已有 memo：`/Users/wdx/opc/flockmux-core/web/src/routes/workspace/views/Chat.tsx`（:200-203 CliReadiness、:446-483 orchestratorFailure、:486-495 retryOrchestrator、:862-873 emptyStateOverride 插槽）
- 类型：`/Users/wdx/opc/flockmux-core/web/src/api/types.ts`（:54 login_command、:129-134 last_error*、:431 SwarmAgentState）
- token：`/Users/wdx/opc/flockmux-core/web/src/styles/global.css`（:71-99 状态色、:35-37 前景、:103-110 agent 色、:129-132 radius）
- 后端事实：`/Users/wdx/opc/flockmux-core/crates/flockmux-server/src/routes/rest.rs`（:33 看门狗常量、:670-695 Spawning/Ready 同步事件、:711-743 watchdog record_agent_error、:766-803 ShimExit/HealthFail、:568-577 fallback_from）
- 错误 kind 来源：`/Users/wdx/opc/flockmux-core/crates/flockmux-server/src/plugins.rs`（:173-185 HealthNeedle kind=auth|rate_limit|fatal）
- 持久化框架（P1 入流落库）：`/Users/wdx/opc/flockmux-core/crates/flockmux-storage/src/store.rs`（:1070 messages kind+meta、:438-444 last_activity_at）；`/Users/wdx/opc/flockmux-core/crates/flockmux-server/src/wake.rs`（:389,849 kind+meta+broadcast+store 可复制模式）
- i18n：`/Users/wdx/opc/flockmux-core/web/src/i18n/locales/zh.json`（新增 `chat.emptyState.*` `chat.bootstrap.*`，扩展 `chat.orchestratorFailure.*`）
