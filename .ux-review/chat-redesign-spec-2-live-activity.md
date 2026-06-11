I have all verified anchors. Here is the spec.

---

# 组件区规格：「现在发生什么」（typing / 心跳 / 派工展开）

> 范围：填 doomscrolling gap 的三组件 —— ① 队长 typing 行 ② 成员 two-signal 心跳活态行 ③ 派工卡折叠/展开。对齐 `chat-redesign.md` §4.2「现在发生什么机制」与原则 1（事实律）/原则 5（渐进披露）/原则 7（稳定身份）。所有数据来源、持久化状态、缺口均以「已核验数据绑定地图」为准并标注 `file:line`。

---

## 1. 目的与边界

### 解决的诊断 / 原则
- **诊断 2（撒谎）**：PendingBubble 在成员静默死亡后仍挂 60s。根因 = 前端没绑 `AgentState::Error/Exited`（`crates/flockmux-protocol/src/ws_swarm.rs:115-116`），尽管后端 90s 看门狗已 `record_agent_error("watchdog")` 并 publish `AgentState::Error`（`crates/flockmux-server/src/routes/rest.rs:731-734`）。
- **诊断 3（重连空白）**：`GET /api/agent/:id/activity` 只读 in-memory ring（`rest.rs:1131-1136`，注释 :1125-1130 自承），刷新/重连后活动归零。
- **doomscrolling gap**：队长收消息后 1-2s 空窗内只有"盯绿点"，无 proof-of-life。
- **原则 1（事实律）**：首个真实输出前绝不出现绿色/"工作中"。
- **原则 5（渐进披露）**：顶层只看一行"正在 X"；派工卡展开才看成员动作时间线 + 小团队树；raw 流再下一层进抽屉终端。
- **原则 7（稳定身份）**：成员一律角色派生名 + 固定色，复用 `roleColorClass`（`web/src/lib/agent.ts:42`），禁 `worker_7` 裸 ID。

### 不做（边界）
- **不做**右面板/乐队栏/live 顶置区容器（属 §4.5，另成规格）。
- **不做**派工事件的后端落库逻辑本身（属 P1，`messages kind=system meta.subtype='dispatch'` 落库由后端规格定义）——本规格只定义**消费**该事件的 UI 与**首卡同步占位**的前端发射点。
- **不做**小团队树的边推导算法（复用 `web/src/lib/dagEdgeDerivation.ts`，本规格只定义其在派工卡展开区的容器与节点视觉）。
- **不做** raw 终端 / 完整 diff / 逐 token 流（退到抽屉，非本组件）。
- **不做** Composer 打断/排队、模型切换、附件（属 §4.4）。

---

## 2. 完整状态枚举

三组件共用一个**数据真相源**：`resolveMemberVisual(agent, live, messages, labels, now)`（`web/src/lib/agent.ts:265-358`）已实现绝大部分诚实层。本组件在其之上叠"用户动词翻译 + two-signal 数字 + 入流/折叠形态"。

### ① 队长 typing 行（live-only；唯一允许"工作中"前置的是它绑 `thought_trace.status`/`AgentActivity`，而非裸 spinner）

| # | 状态 | 触发条件 | 视觉 |
|---|---|---|---|
| A0 | **隐藏** | 队长无 in-flight 轮次（无未回复的 inbound，且无 live thinking 态） | 不渲染 |
| A1 | **思考中** | 用户消息已发；`live.state==='thinking'\|'spawning'` 或 `thought_trace.status==='thinking'`；尚无首个 `AgentActivity` | 「队长 正在思考…」+ 三点 + 计时 |
| A2 | **派工中** | `thought_trace.status==='dispatching'`（缺失，依赖后端新增）**或**回退：首个 `AgentActivity` 到达且队长开始 spawn 成员（`live.state` 仍非 idle） | 「队长 正在派工…」+ 三点 + 计时 |
| A3 | **合并/收尾中** | `thought_trace.status==='merging'`（缺失，依赖后端新增）**或**回退：所有成员 `Exited`/`done` 后队长仍 thinking | 「队长 正在合并结果…」+ 三点 + 计时 |
| A4 | **占位（首卡，spawn 同步）** | spawn 队长瞬间，`AgentState::Spawning` 已 publish（`rest.rs:670-673`），ShimReady 未到 | 「队长 正在启动…」+ 三点 + `◐` |
| A5 | **降级·慢** | A1-A3 持续 > 45s 且无任何新 `AgentActivity`/消息 | 「队长 已思考 50s…」灰文，仍三点（队长不被 tail，不判红，只软提示） |
| A6 | **死亡→移除** | `live.state==='error'\|'exited'`（队长）且 `recoveredSinceError===false` | **立即移除 typing 行**，由失败卡接管（失败卡属 §4.6，本行只负责消失） |
| A7 | **收敛** | 队长发出 `kind='message'` 正文气泡 | typing 行淡出，气泡接管 |

### ② 成员 two-signal 心跳活态行（直接消费 `resolveMemberVisual` 的 `MemberVisual`）

| # | 状态 | 来源映射 | 视觉 |
|---|---|---|---|
| B0 | **不渲染** | 成员不在 in-flight 集（无 live、无近 60s 活动、已 `killed_at`/`shim_exit`） | 不渲染 |
| B1 | **启动中** | `!agent.shim_ready`（`agent.ts:282`），`MemberVisual.dotClass==='bg-state-wake'` | 「测试成员 正在启动…」黄点 |
| B2 | **正在 \<用户动词\>** | `MemberVisual.typing===true`（running/working）+ `live.activity.label` 经动词映射表翻译 | 色头像 + 「正在 写 refund.test.ts」+ 三点 + two-signal 数字 |
| B3 | **等依赖** | `live.state==='waiting_dep'`（`agent.ts:299`） | 「测试成员 在等 后端成员 完成」灰点（依赖名经稳定身份解析，不露 blackboard key） |
| B4 | **45s 无输出·降级** | 启动 grace 后零活动（`agent.ts:336`，`NO_RESPONSE_MS`）**或**单工具 running 卡 `STALL_RUNNING_MS`（:318）→ `dotClass==='bg-state-warning'` | 「测试成员 已 50s 无活动」灰/琥珀点，**去掉三点**，two-signal 第二个数字变「上次输出 50s 前」 |
| B5 | **异常·死即移除** | `MemberVisual.isError===true`（`agent.ts:297/347`，`live.state==='error'` 或冷加载 `last_error` 且未恢复） | **立即移除该活态行**（不挂 60s）；失败信息归失败卡（单一真相源，本行不重复错误文字） |
| B6 | **已终止·移除** | `killed_at!=null` / `shim_exit!=null`（`agent.ts:276-281`） | 移除活态行 |
| B7 | **冷加载/重连·回填中** | WS 刚重连，`GET /api/agent/:id/activity` 在途 | 保留色头像 + 「正在 …」骨架（不显假数字），回填到达后填实 |

### ③ 派工卡（折叠一行 / 展开 = 活动时间线 + 小团队树）

| # | 状态 | 触发 | 视觉 |
|---|---|---|---|
| C0 | **占位·启动中（首卡同步）** | 派工瞬间，前端同步落占位（不等 tailer）；后端 dispatch 消息 `status:'started'`（缺失，依赖后端新增 `meta.subtype='dispatch'`） | 折叠一行 `◐ 派给 测试成员：补失败用例 · 启动中` |
| C1 | **折叠·进行中** | dispatch 消息存在 + 该成员 `MemberVisual.typing` | 一行 `▏派给 测试成员：补失败用例 ●进行中 ▾`（左色条 = 成员色） |
| C2 | **折叠·已完成** | 成员 `Exited` clean 或 dispatch 消息 `status:'done'`（缺失） | 一行 `▏测试成员：补失败用例 ✓已完成 ▾`（左色条转灰） |
| C3 | **折叠·失败** | 成员 `isError===true` | 一行 `▏测试成员：补失败用例 ✕未完成 ▾`（左色条转红，点开看终端） |
| C4 | **展开·活动时间线** | 用户点 `▾` 或卡 | 内联活动时间线（复用 `AgentActivityLog`）+ 小团队树（复用 `dagEdgeDerivation`） |
| C5 | **展开·空（冷加载未回填）** | 展开瞬间 activities 为空、回填在途 | 「正在载入活动…」骨架 3 行 |
| C6 | **展开·空（确无活动）** | 回填完成仍空 | 复用 `agent.activity.empty` 文案（`AgentActivityLog.tsx:49`） |

### 全局空/加载/错误
- **空**：无任何 in-flight → 三组件全不渲染，留给 §4.6 空状态 starter prompts。
- **加载**：WS 未连 / 重连中 → B7/C5 骨架，**永不**显绿点或假数字（事实律）。
- **错误**：A6/B5/C3 → 移除"活着"形态，把真相让位失败卡。

---

## 3. 逐状态 ASCII 线框（标注区域与尺寸）

主轴宽 `max-w≈720px`，活态行/派工卡左对齐贴 720 容器，与队长气泡同栏。

### ① 队长 typing 行

```
A1 思考中 (高 32px, 上 mt-3=12px)
┌──┐  ← 头像 size-8=32px, roleColor('orchestrator')
│队│  队长 正在思考…  ⠋ 02s          ← 名 13px/600 + 动词 13px/500 + 三点 + 计时10px灰
└──┘     └────────────┘ └┘ └─┘
         foreground-primary  三点  tertiary

A4 占位·启动中
┌──┐
│队│  队长 正在启动…  ◐               ← ◐ U+25D0 半填圆, state-wake 色, 14px
└──┘

A5 降级·慢 (>45s)
┌──┐
│队│  队长 已思考 50s…  ⠋            ← 文字转 foreground-tertiary, 三点保留(队长不判红)
└──┘
```

### ② 成员 two-signal 心跳活态行

```
B2 正在<动词> (高 28px, py-1.5)
┌──┐
│测│ 测试成员 正在 写 refund.test.ts   ⋯   14s · 3 文件
└──┘ └──────┘ └─┘ └──────────────┘ └┘  └────────────┘
 头像  名13/600 动词 label(动词映射) 三点  two-signal:
 size-7=28px   accent              动画   计时(变) · 文件数(变)
                                          上次输出仅在降级时出现

B4 45s无输出·降级
┌──┐
│测│ 测试成员  ⚠ 已 50s 无活动 · 上次输出 50s 前
└──┘          └──────────────────────────────┘
 头像          state-warning 文字, 无三点 (停止假装在跑)

B3 等依赖
┌──┐
│测│ 测试成员 在等 后端成员 完成        ◌
└──┘          └──────────────┘
              依赖名经稳定身份解析(非 blackboard key)
```

### ③ 派工卡

```
C1 折叠·进行中 (高 36px, 左色条 width 3px)
▏ 派给 测试成员：补失败用例          ●进行中   ▾
│ └─────────────────────────┘       └────┘  └┘
│  13px/500, 成员名用 roleColor 文字  状态chip  展开
└ 左色条 3px = roleColorHex(role)

C4 展开 (折叠头 36px + 展开区, 展开区 max-h 320px 可滚)
▏ 派给 测试成员：补失败用例          ●进行中   ▴
│ ┌─────────────────────────────────────────────┐
│ │ 小团队树 (max-h 120px)                        │  ← dagEdgeDerivation
│ │   ┌队┐──▶┌测┐  依赖:◐auth.ready ✓schema.ready │     节点=色头像
│ │   └──┘   └──┘                                 │     边=handoff/spawn
│ ├───────────────────────────────────────────────┤
│ │ 活动时间线 (复用 AgentActivityLog, 可滚)        │
│ │  ✓ 读 refund.ts            120ms              │  ← StepGlyph + 动词label + 时长
│ │  ✓ 编辑 refund.test.ts     1.2s               │
│ │  ⠋ 跑测试 npm test         8s…                │  ← running 转圈
│ └───────────────────────────────────────────────┘
└

C0 占位·启动中(首卡同步, spawn瞬间不等tailer)
▏ ◐ 派给 测试成员：补失败用例 · 启动中   ▾    ← ◐ state-wake, 灰条
```

---

## 4. 精确中文文案 + i18n key 命名

i18n 落 `web/src/i18n/locales/zh.json` 与 `en.json`，归到 `messages.live.*`（typing 行 + 活态行）与 `messages.dispatch.*`（派工卡），与现有 `messages.reasoning.*`（`MessagesPanel.tsx:1698`）平级。**行话防火墙**：以下用户字符串中禁出现 mailbox/blackboard/wake/worktree/shim/spell/PTY/handoff；统一用 队长/成员/会话/计划/变更/推进/依赖。

### ① 队长 typing 行

| 状态 | 中文 | i18n key |
|---|---|---|
| A1 | `队长 正在思考…` | `messages.live.captainThinking` |
| A2 | `队长 正在派工…` | `messages.live.captainDispatching` |
| A3 | `队长 正在合并结果…` | `messages.live.captainMerging` |
| A4 | `队长 正在启动…` | `messages.live.captainStarting` |
| A5 | `队长 已思考 {{secs}}s…` | `messages.live.captainSlow` |

### ② 成员活态行

| 状态 | 中文 | i18n key |
|---|---|---|
| B1 | `{{role}} 正在启动…` | `messages.live.memberStarting` |
| B2 前缀 | `正在 ` + 动词(见映射表) | `messages.live.memberDoingPrefix` |
| B2 数字 | `{{secs}}s · {{files}} 文件` | `messages.live.memberSignal` |
| B3 | `在等 {{dep}} 完成` | `messages.live.memberWaitingDep` |
| B4 | `⚠ 已 {{secs}}s 无活动` | `messages.live.memberStalled` |
| B4 尾 | ` · 上次输出 {{ago}}前` | `messages.live.memberLastOutput` |
| B7 | `正在载入…` | `messages.live.memberBackfilling` |

### ③ 派工卡

| 元素 | 中文 | i18n key |
|---|---|---|
| 折叠头 | `派给 {{role}}：{{task}}` | `messages.dispatch.title` |
| ●进行中 | `进行中` | `messages.dispatch.statusRunning` |
| ✓已完成 | `已完成` | `messages.dispatch.statusDone` |
| ✕未完成 | `未完成` | `messages.dispatch.statusFailed` |
| ◐启动中 | `启动中` | `messages.dispatch.statusStarting` |
| 展开树标题 | `小团队` | `messages.dispatch.teamTree` |
| 依赖·已就绪 | `{{dep}} 已就绪` | `messages.dispatch.depReady` |
| 依赖·等待 | `在等 {{dep}}` | `messages.dispatch.depWaiting` |
| 时间线标题 | `做了什么` | `messages.dispatch.timeline` |
| 展开/收起 aria | `展开活动` / `收起活动` | `messages.dispatch.expand` / `.collapse` |
| 载入中 | `正在载入活动…` | `messages.dispatch.loading` |
| 确无活动 | 复用 `agent.activity.empty` | （已存在） |

### ★ AgentActivity.label → 用户动词 映射表（禁行话）

`live.activity.label` 形如 `<ToolName> <arg>`（后端 `transcript.rs:743` `summarize()`，工具名经 `prettify_tool_name` :713 去 `mcp__…__` 前缀）。前端**新建** `web/src/lib/activityVerb.ts`，按工具名前缀映射，arg 原样保留（路径/命令对用户有意义，是文件名不是行话）。落 i18n `messages.verb.*`。

| 后端 label 前缀 | 用户动词（中文） | i18n key | 示例渲染 |
|---|---|---|---|
| `Edit ` / `Write ` / `MultiEdit ` | `写` | `messages.verb.edit` | 写 refund.test.ts |
| `Read ` / `NotebookRead ` | `读` | `messages.verb.read` | 读 refund.ts |
| `Bash <git …>` | `执行 git` | `messages.verb.git` | 执行 git status |
| `Bash <npm/pnpm/yarn … test>` | `跑测试` | `messages.verb.test` | 跑测试 npm test |
| `Bash <npm/pnpm install>` | `装依赖` | `messages.verb.install` | 装依赖 |
| `Bash <build/cargo/tsc/vite>` | `构建` | `messages.verb.build` | 构建 |
| `Bash ` (其它) | `运行` | `messages.verb.run` | 运行 ./script.sh |
| `Grep ` / `Glob ` | `搜索` | `messages.verb.search` | 搜索 validateRefund |
| `WebSearch ` / `WebFetch ` | `查资料` | `messages.verb.web` | 查资料 |
| `Task ` | `派子任务` | `messages.verb.task` | 派子任务 |
| `TodoWrite ` | `更新计划` | `messages.verb.todo` | 更新计划 |
| kind=`system`, label=`整理上下文` 等 | label 原样（后端已是中文白话） | — | 整理上下文 |
| 未匹配 | `处理` + 原 arg | `messages.verb.generic` | 处理 … |

> 实现：`verbFromLabel(label: string): string`。先 `prettify`（前端已去前缀，label 本就裸工具名），按上表 `startsWith` 匹配工具名；`Bash` 再按 arg 首词二级匹配（git/npm-test/install/build/run）。**绝不**把 label 里出现的 worktree 路径段、`.blackboard/`、`*.wake`、`spell` 名透传——映射前先 strip 这些路径前缀（若 arg 以隔离目录前缀开头，只显相对文件名）。

---

## 5. 尺寸 / 间距 / 色彩 token（用现有 design token）

token 定义见 `web/src/styles/global.css`（亮 :23-95 / 暗 :151-208）。

### 通用
- 头像：typing 行 `size-8`(32px)，活态行/派工树 `size-7`(28px)；圆角 `rounded-full`；底色 `roleColorClass(role)`；文字 `--color-foreground-on-accent`。
- 名字：`font-heading 13px / font-semibold(600)`，色 `--color-foreground-primary`。
- 计时/数字/灰文：`font-caption 10-11px`，色 `--color-foreground-tertiary`。
- 行 padding：活态行 `py-1.5`，typing 行 `mt-3`，与气泡 gap `gap-3`。

### 状态色（直接复用，禁新增）
| 语义 | token | 用处 |
|---|---|---|
| 正在跑（三点动画替代点） | （无点，`Loader2 animate-spin text-accent-primary`，复用 `AgentActivityLog.tsx:91`） | B2 / A1-A3 |
| 启动中 ◐ | `--color-state-wake`（亮 #06B6D4） | A4 / B1 / C0 |
| 等依赖 ◌ | `--color-state-idle`（#94A3B8） | B3 |
| 降级·无活动 ⚠ | `--color-state-warning`（#F59E0B），文字 `--color-status-warning`(#B45309) | B4 / A5(用 tertiary) |
| 异常→移除 | `--color-status-danger`（不在本行显，移除让位失败卡） | B5 / C3 左条 |
| 已完成 ✓ | `--color-status-success`（#15803D），步骤 glyph 复用 `AgentActivityLog.tsx:97` | C2 / 时间线 ok |

### 派工卡
- 卡容器：`rounded-2xl border border-border-subtle bg-surface-secondary`（与 PendingBubble 容器一致，`MessagesPanel.tsx:1729`）。
- 左色条：`width 3px`，`background: roleColorHex(role)`（`agent.ts:47`），进行中实色 / 完成 `--color-state-idle` / 失败 `--color-status-danger`。
- 状态 chip：`font-caption 10px`，`●` 进行中 `--color-state-success`(中性绿，非假"工作中") / `✓` `--color-status-success` / `✕` `--color-status-danger` / `◐` `--color-state-wake`。
- 展开区背景 `--color-surface-tertiary`，最大高 `max-h-80`(320px) `overflow-y-auto`；小团队树 `max-h-32`(120px)。
- hover：`hover:bg-surface-tertiary`（复用 `AgentActivityLog.tsx:63`）。

---

## 6. 数据绑定表

每个动态元素 ← 信号 / 端点，标注持久化态与 `file:line`。

| UI 元素 | 数据来源 | 持久化 | file:line |
|---|---|---|---|
| ① typing 行·思考/派工/合并文案 | `live.state`(thinking/spawning) + `trigger.thought_trace.status` | **缺失**：`thought_trace.status` 无 dispatching/merging enum（依赖后端新增 status 枚举或用 started_at/completed_at 推断）；`live.state` 为 live-only | `ws_swarm.rs:81-97,102-117`；`types.ts:469-472`；现状 `MessagesPanel.tsx:1687-1708` 只读 `thought_trace.summary` |
| ① typing 行·计时 | `now - (thought_trace.started_at ?? trigger.sent_at)`，前端 500ms tick | live-only（已实现） | `MessagesPanel.tsx:1682-1688` |
| ① A4 占位·启动中 | `AgentState::Spawning`，spawn 同步 publish | **persisted**（spawn 即发，不等 tailer） | `rest.rs:670-673` |
| ① A6 死亡移除 | `live.state==='error'\|'exited'` | live-only（WS）；冷加载靠 `last_error`(persisted) | `ws_swarm.rs:115-116`；`rest.rs:731-734`；`agent.ts:296,346` |
| ② 活态行·整体决策(点/typing/label/isError) | `resolveMemberVisual(agent, live, messages, labels, now)` | 混合：硬态(`killed_at`/`shim_exit`/`shim_ready`)persisted，`live.state/activity` live-only，`last_activity_at`/`last_error*` persisted | `agent.ts:265-358`；`store.rs:438-452`(touch)；migrations `0013`/`0022` |
| ② 「正在 \<动词\>」label | `live.activity.label` → `verbFromLabel()`(新建) | live-only；冷加载需 `GET /api/agent/:id/activity` 回填 | `ws_swarm.rs:81-97`；后端 label 构造 `transcript.rs:723-746` |
| ② two-signal·计时 | `live.activity.phase==='running' ? now-at : duration_ms`，复用 `formatActivityLine` | live-only | `agent.ts:363-371` |
| ② two-signal·文件数 | **缺失**：无"本轮改了几个文件"聚合字段（需后端在 dispatch/activity 累计 file count，或前端从 activities 中 `label.startsWith('Edit/Write')` 去重计数兜底） | **missing**（依赖后端新增 或 前端从 activity 流推导） | 兜底推导处 `AgentActivityLog` activities[] |
| ② B4·45s 降级 | `now - (last_activity_at ?? live.activity.at)` 比阈值；`resolveMemberVisual` 已用 `NO_RESPONSE_MS`/`STALL_RUNNING_MS` | `last_activity_at` persisted | `agent.ts:128(NO_RESPONSE_MS),318,336`；`store.rs:438-444` |
| ② B4·「上次输出 N 前」 | `now - last_activity_at` | **persisted** | `store.rs:438-444`；`types.ts` AgentInfo.last_activity_at |
| ② B3·等依赖名 | `live.state==='waiting_dep'` + `agent.depends_on` 经稳定身份解析成员名 | state live-only；`depends_on` persisted（blackboard key 列表） | `agent.ts:299`；`types.ts:92-95` |
| ② B5/B6 移除 | `MemberVisual.isError` / `killed_at` / `shim_exit` | persisted | `agent.ts:276-281,297,347` |
| ② B7 重连回填 | WS 重连 → `GET /api/agent/:id/activity` | **missing 持久化**：端点只读 in-memory ring，冷加载/重启即空 | `rest.rs:1125-1136`（依赖后端改走 DB：新表 `agent_activities`，参考 `touch_agent_activity` 模式 `store.rs:438-452`） |
| ③ 派工卡·折叠头(role/task) | 派工消息 `kind='system' meta={subtype:'dispatch', agent_role, task, status}` | **missing**：派工事件无落库（依赖后端新增 dispatch 消息，复用 `messages` 已有 kind+meta `store.rs:1070-1081`，参考 wake 示范 `wake.rs:389-396`） | gap 见数据地图 |
| ③ C0 首卡同步占位 | spawn/dispatch 瞬间前端同步插入占位（不等 700ms tailer），后端补 `meta.subtype='dispatch' status:'started'` | persisted（后端补后） | 发射点 `rest.rs:1161-1250 spawn_bootstrap_inject`（依赖后端在此同步 emit dispatch 消息） |
| ③ 派工卡·进行/完成/失败状态 | 该成员 `MemberVisual` / dispatch 消息 `meta.status` | 实时 + persisted(消息后) | `agent.ts:265`；gap(meta.status) |
| ③ 展开·活动时间线 | `AgentActivityLog` ← activities[]（由 `useWorkspaceShellData` 累积的 `agentActivityById`） | live-only + 回填依赖同上 GET activity | `AgentActivityLog.tsx:25-87`；回填端点 `rest.rs:1125-1136` |
| ③ 展开·小团队树 | `deriveHandoffEdges`/`deriveSpawnEdges`(`depends_on`+`handoff_signal`+`parent_agent_id`) | persisted | `dagEdgeDerivation.ts:51-95`；`types.ts:92-95,107-109` |
| ③ 树·依赖 satisfied(◐/✓) | `writtenAt(blackboard key) >= spawned_at` | persisted（blackboard_ops 推导写入时刻） | `dagEdgeDerivation.ts`；`types.ts:92` |

**本规格依赖的后端新增（明确标注）**：
1. **依赖后端新增** `thought_trace.status` enum（thinking/dispatching/merging）—— typing 行 A2/A3 精确文案；缺则前端用 `live.state` + 成员 spawn/exit 时序推断（降级实现，仍可上线）。
2. **依赖后端新增** `GET /api/agent/:id/activity` 走 DB（新表 `agent_activities`）—— B7/C4 冷加载回填；P1 硬依赖。
3. **依赖后端新增** 派工消息 `kind='system' meta={subtype:'dispatch',...}` 落库 —— 派工卡 ③ 整体重放；缺则派工卡仅活在 live，刷新即失（不满足入流律）。
4. **依赖后端新增** spawn 同步 emit dispatch 占位 —— C0 首卡即时落地。
5. **依赖后端新增** 本轮 file count 聚合 —— two-signal 第二数字；缺则前端从 activity 流去重 Edit/Write 路径兜底（不精确但诚实）。

---

## 7. 复用 vs 新建（到具体文件 / 函数）

### 复用（不改或微改）
- `resolveMemberVisual` / `formatActivityLine` / `inferAgentStatus`（`web/src/lib/agent.ts:265,363,134`）—— 活态行的整套诚实层、stall/recovery/死即移除判定**已实现**，活态行**直接消费 `MemberVisual`**，B5「死即移除」= 当 `isError===true` 时不渲染该行（根因修复，无需新逻辑）。
- `roleColorClass` / `roleColorHex` / `roleInitial` / `resolveRole`（`agent.ts:42,47,56,68`）—— 头像色、左色条、稳定身份名，三组件全复用。
- `AgentActivityLog`（`web/src/components/agent/AgentActivityLog.tsx`）—— 派工卡展开区 C4 时间线**原样复用**（已含 running 转圈 / ok✓ / error✕ glyph、按 seq 滚动、空态文案）。**微改**：渲染每条前把 `s.label` 过 `verbFromLabel()`（保持 mono 字体不变）。
- `deriveHandoffEdges` / `deriveSpawnEdges`（`web/src/lib/dagEdgeDerivation.ts:51-95`）—— 小团队树边推导，**canonical 单一真相源**，直接喂内联树。
- PendingBubble 头像 + 三点结构（`MessagesPanel.tsx:1709-1729`）—— typing 行布局骨架复用。
- 500ms tick 模式（`MessagesPanel.tsx:1682-1686`）/ 1s tick（`AgentActivityLog.tsx:35-38`）—— 计时自增。

### 改
- **typing 行**：现 PendingBubble（`MessagesPanel.tsx:1670-1708`）只绑 `trigger.thought_trace`，**改**为同时订阅该队长的 `live.state` 与首个 `AgentActivity`，按 §2 A0-A7 切文案；**绑 `live.state==='error'\|'exited'` → 立即卸载**（修诊断 2）。
- **未读/活态共用**：无（活态行不进未读计数）。

### 新建
- `web/src/lib/activityVerb.ts` —— `verbFromLabel(label)` 映射表（§4 ★），+ 行话路径 strip。
- `web/src/components/messages/MemberHeartbeatRow.tsx` —— 成员 two-signal 活态行（消费 `MemberVisual` + `verbFromLabel` + two-signal 数字 + B4 降级 + B5 死即移除）。
- `web/src/components/messages/CaptainTypingRow.tsx` —— 队长 typing 行（A0-A7，从 PendingBubble 抽出并增强）。
- `web/src/components/messages/DispatchCard.tsx` —— 派工卡（折叠头 + 状态 chip + 展开区，展开区嵌 `AgentActivityLog` + 小团队树子组件）。
- `web/src/components/messages/InlineTeamTree.tsx` —— 内联小团队树（喂 `dagEdgeDerivation` 边 + `roleColor` 节点）。**承认成本**：这是 `Dag.tsx`(843 行) 的**内联重写**，非白捡，按 §6 P2 排期。
- 前端发射点（spawn 同步占位）：在 spawn 成员的前端调用处（`MessagesPanel.tsx` send→spawn 链）同步插入 C0 占位卡，不等后端 tailer 回流。

---

## 8. 交互与时序

### 事件 → 状态转换
- **队长 typing**：用户发消息 → 立即 A1（≤500ms，前端乐观，不等 WS）→ 收 `AgentState::Spawning`(`rest.rs:670`) 保持 → `thought_trace.status` 推进 A2/A3 → 队长气泡到达 → A7 淡出。**proof-of-life 1-2s 内必现**：A1 由前端在 send 瞬间乐观点亮，不依赖 WS 往返。
- **成员活态行**：每收一个 `AgentActivity`/`AgentState` → 重算 `resolveMemberVisual` → 重渲染。**死即移除**：收 `AgentState::Error/Exited` → 该成员从 in-flight 集移除 → 行卸载（无 60s 残留）。
- **派工卡首卡**：dispatch 触发瞬间前端同步落 C0（不等 700ms tailer）→ 后端 dispatch 消息到达替换为 C1 → 成员完成 → C2/C3。

### 阈值
- `STARTUP_GRACE_MS=45_000`（`agent.ts:127`）：成员零活动宽限，期内乐观显在跑。
- `NO_RESPONSE_MS=300_000` / `STALL_RUNNING_MS=300_000`（:126,128）：现状 5min 才报 stall。**本规格要求 45s 降级**——**改**：活态行 B4 用独立 `HEARTBEAT_STALE_MS=45_000` 判「45s 无输出」软降级（文案级，不改 `resolveMemberVisual` 的红/黄硬判，避免误报红）。即 45s→灰文软提示，5min→琥珀 stall（保留现有保守阈值不动）。
- typing 行 A5 慢提示：45s。

### 防抖 / 节流
- 计时 tick 统一 500ms（typing）/1000ms（时间线），不随 activity 频率刷新。
- WS `AgentActivity` 后端已"低频、每工具 running/ok 各一次"（`ws_swarm.rs:77`），前端无需额外节流；按 `(agent_id, seq)` 取每对最新（去重，:91）。
- 派工卡展开/收起：无防抖，纯本地状态。

### 键盘 / 可达性 aria
- 派工卡折叠头为 `<button>`，`aria-expanded`，`aria-controls` 指展开区 id；`Enter`/`Space` 切换；`▾/▴` 旁加 `aria-label`（`messages.dispatch.expand/.collapse`）。
- 活态行三点动画包 `aria-hidden`，行整体 `role="status" aria-live="polite"`，屏读出「测试成员 正在 写 refund.test.ts」。
- typing 行同 `role="status" aria-live="polite"`，A5/B4 降级文案靠文本传达（不靠纯色），满足色彩无障碍。
- 降级/异常**不只用颜色**：B4 带 `⚠` 字符 + 文字，C3 带 `✕` + 文字（色盲可读）。
- 焦点：派工卡展开后焦点留在折叠头；时间线 `overflow-y-auto` 容器 `tabindex=0` 可键盘滚动。

---

## 9. 验收标准（可勾选 + 诚实性断言）

### 功能
- [ ] 用户发消息后 **≤2s** 出现队长 typing 行（A1），无需等 WS 往返（前端乐观点亮）。
- [ ] typing 行随 `thought_trace.status` / `live.state` 在 思考→派工→合并 间切换文案（缺 status 时用 live.state+spawn/exit 时序降级推断，仍切换）。
- [ ] 成员活态行显示「正在 \<用户动词\>」，动词来自 `verbFromLabel`，**无** Edit/Bash/Read 等英文工具名裸露。
- [ ] two-signal 两个数字（计时 + 文件数/上次输出）会随时间/活动变化。
- [ ] 派工卡折叠态一行；点 `▾` 展开见活动时间线（复用 `AgentActivityLog`）+ 小团队树。
- [ ] 派工 spawn 瞬间同步落 C0 占位（`◐启动中`），不等 700ms tailer。
- [ ] WS 重连后，活态行/派工卡先 `GET /api/agent/:id/activity` 回填再接流（端点走 DB 后）。

### 行话防火墙
- [ ] 三组件所有用户可见字符串经 grep 确认**不含** mailbox/blackboard/wake/worktree/shim/spell/PTY/handoff（含 `verbFromLabel` strip 掉路径中的隔离目录/`.blackboard`/`*.wake`）。
- [ ] 依赖名（B3「在等 X 完成」）显示成员角色派生名，**非** blackboard key 字面（如非 `refund.schema.ready`）。

### 稳定身份
- [ ] 同一成员在活态行、派工卡左色条、小团队树节点用**同一色同一名**（全经 `roleColor*`），无 `worker_7` 裸 ID。

### 诚实性断言（不许撒谎）
- [ ] **首个真实输出前绝不出现绿色**：A4/C0 占位用 `◐`+`state-wake`（青），**非**绿点；B1 启动中用 `state-wake`。
- [ ] **死即移除**：成员 `AgentState::Error/Exited` 后，该活态行 **≤1 个渲染帧**内消失，**不挂 60s**（直接断诊断 2）。验证：手动 kill 一成员，活态行立即消失。
- [ ] **看门狗背书**：90s 无响应后（`rest.rs:731` 发 `AgentState::Error`），typing 行/活态行翻为移除态，失败由失败卡接管，**绿点不再转**。
- [ ] **单一真相源**：活态行/派工卡**不重复**失败错误全文（错误只在失败卡讲一次），B5 只负责"消失"，不显错误字符串。
- [ ] **冷加载不撒谎**：刷新页面后，无 live 数据时活态行显骨架/不渲染，**绝不**显假计时/假文件数/假绿点（B7）。
- [ ] **45s 降级诚实**：成员 45s 无输出 → 显「已 50s 无活动 · 上次输出 N 前」，**停三点动画**（不假装在跑）。
- [ ] typing 行 A5 队长慢（>45s）显「已思考 Ns…」灰文但**不判红**（队长不被 tail，软提示而非伪失败）。

---

## 关键文件锚点（绝对路径）
- 复用诚实层：`/Users/wdx/opc/flockmux-core/web/src/lib/agent.ts`（`resolveMemberVisual:265`、`formatActivityLine:363`、`roleColor*:42-56`、阈值 `:126-128`）
- 复用时间线：`/Users/wdx/opc/flockmux-core/web/src/components/agent/AgentActivityLog.tsx`（glyph/滚动/空态）
- 复用树边：`/Users/wdx/opc/flockmux-core/web/src/lib/dagEdgeDerivation.ts:51-95`
- 改造点：`/Users/wdx/opc/flockmux-core/web/src/components/MessagesPanel.tsx`（PendingBubble `:1670-1729`、tick `:1682`）
- 协议/状态源：`/Users/wdx/opc/flockmux-core/crates/flockmux-protocol/src/ws_swarm.rs`（AgentActivity `:81-97`、AgentState 七态 `:102-117`）
- 后端事实：`/Users/wdx/opc/flockmux-core/crates/flockmux-server/src/routes/rest.rs`（Spawning 同步发 `:670-673`、看门狗 `:33,709-734`、activity 端点读 ring `:1125-1136`、spawn_bootstrap_inject `:1161-1250`）
- label 构造：`/Users/wdx/opc/flockmux-core/crates/flockmux-server/src/transcript.rs`（`summarize:723-746`、`prettify_tool_name:713`）
- 持久化模式：`/Users/wdx/opc/flockmux-core/crates/flockmux-storage/src/store.rs`（`touch_agent_activity:438-452`、messages kind+meta `:1070-1081`）；wake 示范 `/Users/wdx/opc/flockmux-core/crates/flockmux-server/src/wake.rs:389-396`
- design token：`/Users/wdx/opc/flockmux-core/web/src/styles/global.css`（state/status `:70-95`、foreground/surface/accent `:23-58`）
- i18n：`/Users/wdx/opc/flockmux-core/web/src/i18n/locales/zh.json`（新增 `messages.live.*` / `messages.dispatch.*` / `messages.verb.*`，与 `messages.reasoning.*` 平级）

**新建文件**：`web/src/lib/activityVerb.ts`、`web/src/components/messages/{CaptainTypingRow,MemberHeartbeatRow,DispatchCard,InlineTeamTree}.tsx`
