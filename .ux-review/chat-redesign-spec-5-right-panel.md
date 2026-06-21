所有锚点已核验完毕。我注意到 i18n 文件里现有文案存在行话泄漏(如 `agent.injectPlaceholder` 里有 "wake"、`agent.confirm.wake.desc` 里有 "mailbox/blackboard"),但这些不在我本组件区范围内,我会在规格里把行话防火墙作为约束写明。

我已掌握全部事实:右面板五 tab 现状、`resolveMemberVisual`/`formatActivityLine` 的诚实降级逻辑、design token、i18n 结构、`AgentLiveState`/`AgentInfo` 字段及其持久化状态。现在产出规格。

---

# 组件区规格 ·「右面板三态（脉搏 / 乐队 / 焦点）+ 成员」

> 范围：右侧工作面板的三种形态与断点行为。对齐 `chat-redesign.md` §3（整屏布局：1280px 默认收脉搏条 / 1536+ 常驻）、§4.2（现在发生什么 two-signal）、§4.5（成员/活动面板）、§6（P0「右面板默认收为脉搏条 + 1536+ 召出 AgentDrawer 为面板」）。
> 本规格只管**右面板容器、三态形态、断点/转场、跨高亮**。对话主轴、Composer、消息流卡片不在本区（仅定义跨区接口）。

---

## 1. 目的与边界

### 1.1 这个组件解决哪条诊断 / 原则

| 来源 | 内容 | 本组件如何承接 |
|---|---|---|
| 诊断 2 | PendingBubble 在成员死亡后静默挂 60s（绿点撒谎） | live 区 two-signal 行**绑 `AgentState`，死即移除**；用已实现的 `resolveMemberVisual` 而非启发式 |
| 诊断 3 | 重连/冷加载后「现在发生什么」变空白 | 焦点模式活动 tab 重连先打 `GET /api/agent/:id/activity` 回填（**硬依赖 P1**：端点需走 DB） |
| 诊断 4 | firehose 结构性风险 | 三态物理隔离：脉搏条只剩态点+数字，机器细节退到焦点模式 tab |
| 原则 4 | 对话与活动分离 | 右面板是「机器细节的家」，**默认收起**（脉搏条），不是 cockpit 常驻税 |
| 原则 5 | 渐进披露（altitude control） | 三态 = 三个高度：脉搏（俯瞰点）→ 乐队/live（中层动作）→ 焦点（底层 PTY/活动） |
| 原则 7 | 稳定身份 | 三态全程复用 `roleColorClass` / 角色派生名，禁裸 `worker_7` |

### 1.2 不做什么（边界）

- **不做对话主轴 / Composer**：本区只通过「跨高亮」「派工卡点击 → 进焦点」两个接口与对话区耦合，不渲染气泡。
- **不重写 AgentDrawer 五 tab 内容面**：终端（`XtermPane`）、活动（`AgentActivityLog`）、录像、消息、上下文五个**内容面板原样复用**；本区只新建**外层容器 + 三态切换 + 乐队栏 + live 顶置区**。
- **不做 diff review 行级评论 / 合并闸**（§4.3 P2，独立组件区）。本区的「变更」tab 只复用现有 diff 渲染，不含评论桥。
- **不做后端持久化层**：AgentActivity 走 DB（P1）、系统事件落库（P1）是别的工单；本区**消费**这些信号，对 missing 信号按 §6 标注降级。
- **不改 1280px 以下移动端**：移动端右面板整体不渲染（成员入口走别处），本区只覆盖 `≥1280px`。

---

## 2. 完整状态枚举

### 2.1 容器形态（三态 × 断点）

| 形态 ID | 触发 | 断点 | 宽度 | 说明 |
|---|---|---|---|---|
| `pulse` 脉搏条 | 默认 | 1280–1535px | **54px** | 竖向窄条，只态点+数字徽章 |
| `band` 工作面板（乐队全队） | 用户召出 / 1536+ 常驻 | 1280+（覆盖滑出）/ 1536+（常驻） | **覆盖滑出 420px / 常驻 360–440px** | 乐队栏 + live 区 + 深度 tab，全队视角 |
| `focus` 单成员焦点 | 点脉搏点 / 点乐队 chip / 点流内派工卡 | 同 band | 同 band | 同容器，内容聚焦单成员；可逆回 band |

> `band` 与 `focus` 共用同一容器宽度与外框，区别仅在**内容**（全队 vs 单成员）与**顶部是否有返回控件**。这是「钻取 = 换面板内容，不新建」的物理体现。

### 2.2 每态内部的视觉状态（含空/加载/错误/降级）

**A. `pulse` 脉搏条内每个态点（来自 `resolveMemberVisual.dotClass` / `typing` / `isError`）**

| 状态 | 视觉 | 数据条件 |
|---|---|---|
| 启动中 | 青点 `state-wake` | `!shim_ready` |
| 运行中（typing） | 角色色点 + 微脉冲动画 | `typing===true`（thinking/spawning/工具 running） |
| idle / 等你 | 绿点 `state-success` | semantic idle/awaiting_user |
| 等依赖 | 灰点 `state-idle` | `state==='waiting_dep'` |
| 可能卡住（降级） | 琥珀点 `state-warning` | 工具 running >300s 或 worker 无活动 >300s |
| 异常 / 失败 | 红点 `status-danger` + 顶到最上 | `state==='error'` 或 `last_error` 且未恢复 |
| 已终止 / 已下线 | 灰点 `state-idle` | `killed_at` / `shim_exit` |
| **空（无成员）** | 脉搏条不渲染任何点，仅留 54px 空条 + 顶部 `▣` 召出钮 | `agents.length===0` |
| **加载（首次拉 agents）** | 顶部 `▣` 钮可见，点区显骨架灰点 ×1 | `agentsLoading` |

**B. `band` 工作面板**

| 区域 | 正常 | 空 | 加载 | 错误/降级 |
|---|---|---|---|---|
| 乐队栏 | 横向 chip 列（角色色+态徽章+"正在<动作>"） | 「还没有成员在工作」自我说明 | 骨架 chip ×2 | 失败成员 chip 红边 `✕卡住` + tooltip（不展开整段错误） |
| live 区 | 每在跑成员一行 two-signal | 「此刻无成员在跑」+ 引导文案 | 骨架行 ×1 | 降级行：`⚠ 已 Ns 无活动`（灰） |
| 深度 tab | `[活动][变更N][终端]` | 各 tab 自有空态（复用） | tab 内骨架 | 重连时 tab 顶部「正在恢复活动…」 |

**C. `focus` 单成员焦点**

| 区域 | 正常 | 空 | 加载 | 错误 |
|---|---|---|---|---|
| 焦点头 | 返回钮 + 角色色头像 + 名 + 单成员 two-signal | — | 头部骨架 | 失败成员：头部红条 + 复用失败卡 |
| 五 tab 内容 | 复用 AgentDrawer 五 tab | tab 自有空态 | 复用 | 终端：成员已退出 → 复用 `agent.terminalExited` |
| 终端不可用 | — | — | `agent.terminalLoading` | `shim_exit!=null` → 终端禁用 |

### 2.3 转场状态（瞬态）

| 转场 | 中间态 |
|---|---|
| pulse → band（召出） | 覆盖滑出动画进行中（180ms），脉搏条淡出 |
| band → focus（钻取） | 内容横向滑移 160ms，乐队栏收为单 chip 面包屑 |
| focus → band（返回） | 反向滑移；保留滚动位置 |
| 断点跨越 1536（窗口 resize） | pulse 自动升 band 常驻（无动画，直接布局）；band/focus 状态保持 |

---

## 3. 逐状态 ASCII 线框

### 3.1 `pulse` 脉搏条（1280–1535px，宽 54px）

```
┌────┐  ← 右面板容器, width:54px, 左 1px border-subtle
│ ▣  │  ← 顶部召出钮 32×32, 居中, title「展开工作面板」
│────│  ← 1px border-subtle 分隔
│    │
│ ●  │  ← 成员态点 dot 10×10, 角色色; 失败/异常顶到最上
│ ❷  │  ← 数字徽章 badge: 该成员未读/待办计数(可选), 16px, 角色色 8% 底
│    │     行高 36px, 点+徽章纵向居中
│ ◐  │  ← 启动中(青脉冲)
│    │
│ ⚠  │  ← 可能卡住(琥珀)
│    │
│ ✕  │  ← 失败(红), 永远排第一(isError 置顶)
│    │
│ ⋮  │  ← 溢出: >6 成员时底部「+N」, 点击直接进 band
└────┘
区域: [召出钮 32px][分隔 1px][态点列 1行36px×N][溢出 +N]
空态: 召出钮保留, 态点列为空(不显「暂无」字, 窄条放不下)
```

### 3.2 `band` 工作面板 — 全队（1536+ 常驻 / 1280 召出覆盖）

```
┌──────────────────────────────────────┐ width: 常驻360–440 / 覆盖420
│ 工作面板            [全部▾]  ⤢   ✕    │ ← 顶栏 48px: 标题 + 视图切换 + 全屏 + (覆盖态)关闭
├──────────────────────────────────────┤
│ 乐队栏 (横向 chip, 可横滚, gap 8)      │ ← 高 auto, py 10
│ ┌──────┐┌──────────┐┌────────┐        │
│ │●队长 ││◉测试·写测试││◐后端·启动│  ...  │   chip: 角色色点 + 名 + 五态徽章
│ └──────┘└──────────┘└────────┘        │        + "正在<动作>"小字(11px)
├──────────────────────────────────────┤
│ 现在                              ⌃▣  │ ← live 顶置区标题 13px + 一键全终端
│ ┌──────────────────────────────────┐ │
│ │◉测试 写 refund.test.ts           │ │ ← two-signal 行 high 44px:
│ │  14s · 上次输出 2s前         ⌃▣  │ │   角色色点 + 动词 / 计时·上次输出 + 看终端
│ ├──────────────────────────────────┤ │
│ │◐后端 启动队长引擎…  8s       ⌃▣  │ │
│ ├──────────────────────────────────┤ │
│ │⚠前端 已 50s 无活动           ⌃▣  │ │ ← 降级行(灰)
│ └──────────────────────────────────┘ │
├──────────────────────────────────────┤
│ [活动]  [变更 3]  [终端]              │ ← 深度 tab 40px (复用 AgentDrawer 内容)
├──────────────────────────────────────┤
│  ✓ 队长 派工 → 测试         02:55     │
│  ✓ 测试 编辑 refund.ts +12  02:56     │ ← tab 内容区 flex-1 滚动
│  ◉ 测试 跑测试…             02:57     │   (此处为 [活动] tab: AgentActivityLog)
│                                      │
└──────────────────────────────────────┘
区域高度分配: 顶栏48 + 乐队栏auto + live区auto(max 3行可滚) + tab栏40 + 内容flex-1
```

### 3.3 `focus` 单成员焦点（同容器，内容聚焦）

```
┌──────────────────────────────────────┐
│ ‹ 全部   ◉ 测试成员              ✕    │ ← 焦点头 48px: 返回‹ + 角色色头像 + 名 + 关闭
│          正在 写 refund.test.ts 14s   │ ← 单成员 two-signal (头下一行)
├──────────────────────────────────────┤
│ [终端] [活动] [录像] [消息] [上下文]   │ ← 五 tab (AgentDrawer 原五 tab 全保留)
├──────────────────────────────────────┤
│                                      │
│   (当前 tab 内容: XtermPane /         │ ← 内容区 flex-1
│    AgentActivityLog / Recordings /    │
│    Messages / Context)                │
│                                      │
├──────────────────────────────────────┤
│ [⚡唤醒]  [⏸暂停]                     │ ← ActionRow (复用 AgentDrawer Header 下半)
└──────────────────────────────────────┘
返回‹ 点击 → 回 band 全队视图(转场反向滑移)
失败成员: 焦点头下方嵌入 OrchestratorFailureCard(泛化)
```

### 3.4 失败成员在三态的呈现（单一真相源纪律）

```
pulse:  ✕ 红点(置顶) + tooltip「测试成员 卡住: 未登录」  ← 不展开整段
band:   乐队栏 chip 红边「✕测试·卡住」 + live 区该行不显, 详情在焦点
focus:  焦点头红条 + 嵌入失败卡(完整错误 + 行动按钮, 权威唯一源)
```

---

## 4. 精确中文文案（含 i18n key）

> **行话防火墙**：禁用 mailbox / blackboard / wake / worktree / shim / spell / PTY / handoff / mailbox。用「队长 / 成员 / 会话 / 计划 / 变更 / 推进 / 共享区 / 唤起」。
> **注意现存泄漏**：`agent.injectPlaceholder`（"wake"）、`agent.confirm.wake.desc`（"mailbox/blackboard"）等已泄漏，本区新增 key 严守，且建议把焦点模式复用到的旧 key 一并清洗（见 §7 改造项）。

### 4.1 容器 / 顶栏

| key（建议新建于 `workspace.panel.*`） | 中文 | 备注 |
|---|---|---|
| `workspace.panel.title` | 工作面板 | band 顶栏标题 |
| `workspace.panel.expand` | 展开工作面板 | 脉搏条召出钮 aria-label / title |
| `workspace.panel.collapse` | 收起工作面板 | band → pulse |
| `workspace.panel.viewAll` | 全部 | 视图切换（全队） |
| `workspace.panel.fullscreen` | 全屏查看 | `⤢` title |
| `workspace.panel.close` | 关闭面板（Esc） | 覆盖态关闭 |
| `workspace.panel.empty.title` | 还没有成员在工作 | 乐队栏空态 |
| `workspace.panel.empty.hint` | 这里会实时显示每个成员在做什么。 | 教学（对齐 §4.6 右面板自我说明） |
| `workspace.panel.overflow` | 还有 {{count}} 名 | 脉搏条溢出 +N |

### 4.2 乐队栏 chip（五态徽章 + 动作小字）

| key（建议 `workspace.band.*`） | 中文 | 数据条件 |
|---|---|---|
| `workspace.band.state.starting` | 启动中 | `!shim_ready` |
| `workspace.band.state.running` | 进行中 | typing |
| `workspace.band.state.waiting` | 等依赖 | `waiting_dep` |
| `workspace.band.state.stalled` | 可能卡住 | 降级 |
| `workspace.band.state.error` | 卡住 | error/last_error |
| `workspace.band.state.idle` | 待命 | idle |
| `workspace.band.state.exited` | 已结束 | killed/exit |
| `workspace.band.doing` | 正在 {{verb}} | chip 小字，verb 来自 activity.label 翻译 |

> 五态徽章 = {启动中, 进行中, 等依赖, 卡住, 待命}，加「已结束」终态共 6 个文案，复用 §4.3 已有的 `resolveMemberVisual` labels 入参（`exited/shimExit/starting/stalled/noResponse/<SwarmAgentState>`）——**不新建状态机，只补文案映射**。

### 4.3 live 区（two-signal）

| key（建议 `workspace.live.*`） | 中文 | 备注 |
|---|---|---|
| `workspace.live.title` | 现在 | live 区标题 |
| `workspace.live.empty` | 此刻没有成员在跑 | live 空态 |
| `workspace.live.emptyHint` | 成员一旦开始干活，这里会逐行显示它正在做什么。 | 教学 |
| `workspace.live.doing` | 正在 {{verb}} | two-signal 第一信号 |
| `workspace.live.elapsed` | {{delta}} | 计时（如「14s」「2m5s」，复用 `formatDelta`） |
| `workspace.live.lastOutput` | 上次输出 {{delta}}前 | 第二信号 |
| `workspace.live.stalled` | 已 {{delta}} 无活动 | 45s 降级（灰，前缀 ⚠） |
| `workspace.live.viewTerminal` | 看终端 | `⌃▣` aria-label |
| `workspace.live.viewAllTerminals` | 查看全部终端 | live 区标题旁 `⌃▣` |

### 4.4 焦点模式

| key（建议 `workspace.focus.*`） | 中文 |
|---|---|
| `workspace.focus.back` | 全部 | 返回钮（‹ 全部） |
| `workspace.focus.backAria` | 返回全队视图 |

> 焦点模式五 tab 标签**复用现有** `agent.tabs.terminal/activity/recordings/messages/context`（已存在，§3.5 验证），ActionRow 复用 `agent.wake/pause/resume`（已存在）。

### 4.5 动词翻译表（`AgentActivity.label` → 用户动词，严守防火墙）

> `label` 现状是英文工具名如 `"Edit src/foo.rs"`、`"Bash npm test"`。新建纯函数 `activityVerb(label)` 映射：

| label 前缀 | 中文动词 |
|---|---|
| `Edit`/`Write`/`MultiEdit` | 写 {{file}} |
| `Read` | 读 {{file}} |
| `Bash` + test 关键词 | 跑测试 |
| `Bash` + install | 装依赖 |
| `Bash`（其它） | 运行命令 |
| `Grep`/`Glob` | 查代码 |
| `system` kind | 推进中 |
| 未知 | 工作中 |

i18n key 建议 `workspace.verb.write/read/test/install/run/search/advance/work`。

---

## 5. 尺寸 / 间距 / 色彩 token

> 全部用 §global.css 已验证的 token，禁硬编码 hex。圆角用 `--radius-*`。

### 5.1 容器尺寸

| 项 | 值 | 备注 |
|---|---|---|
| pulse 宽 | `54px` | 固定 |
| band/focus 常驻宽 | `clamp(360px, 26vw, 440px)` | 1536+ |
| band/focus 覆盖宽 | `420px` | 1280–1535 滑出，带 `box-shadow` 投影 |
| 容器左边框 | `1px solid var(--color-border-subtle)` | |
| 容器底色 | `var(--color-surface-secondary)` | 与侧栏一致 |
| 顶栏高 | `48px` | |
| tab 栏高 | `40px` | 复用 AgentDrawer TabBar |
| live 行高 | `44px` | two-signal |
| 乐队 chip 高 | `28px` | |
| 脉搏点行高 | `36px` | |

### 5.2 色彩 token（态点 / 徽章）

| 用途 | token |
|---|---|
| 角色色点/头像/chip 边 | `--color-agent-{role}`（经 `roleColorClass`，缺省 `bg-state-idle`） |
| 启动中点 | `--color-state-wake` `#06B6D4` |
| 运行中点 | 角色色 + 脉冲 |
| idle/待命点 | `--color-state-success` |
| 等依赖点 | `--color-state-idle` |
| 降级/可能卡住点 | `--color-state-warning` `#F59E0B` |
| 失败点/边 | `--color-status-danger` |
| 失败 chip 软底 | `--color-status-danger-soft` |
| chip 软底（正常） | `--color-surface-tertiary` |
| 主文字 | `--color-foreground-primary` |
| 小字/计时 | `--color-foreground-tertiary` |
| 降级文字 | `--color-foreground-tertiary`（灰，非红） |
| 分隔线 | `--color-border-subtle` |

### 5.3 字号 / 行高 / 圆角

| 元素 | 字号 | 圆角 |
|---|---|---|
| 顶栏标题 | `text-sm`(13px) font-bold | — |
| 乐队 chip 名 | `12px` | `--radius-full`(点) / `--radius-md`(chip) |
| chip 动作小字 | `11px` foreground-tertiary | — |
| live 动词 | `13px` foreground-primary | — |
| live 计时/上次输出 | `11px` foreground-tertiary | — |
| 态点 | 10×10px | `--radius-full` |
| 数字徽章 | `10px`, min 16px | `--radius-full` |
| 焦点头角色头像 | 32px | `--radius-full` |
| 卡/面板圆角 | `--radius-lg`(8px) | |

---

## 6. 数据绑定表

> 每个动态元素 ← 信号 / 端点 + 持久化状态 + file:line。`missing` 标注「依赖后端新增 X」。

| UI 元素 | 数据来源 | 持久化 | 引用 file:line | 缺口/降级 |
|---|---|---|---|---|
| 脉搏点颜色/typing/置顶 | `resolveMemberVisual(agent, live, messages, labels)` | derived（实时） | `web/src/lib/agent.ts:265-358` | **已实现**，本区直接调；含 45s/300s 降级与死即移除 |
| 数字徽章计数 | 精确未读 per-(from,thread) | persisted | `crates/swarmx-storage/src/store.rs:1531-1577`（现 COUNT 无 GROUP BY） | **降级**：未补精确端点前，徽章可先隐藏或显总未读（依赖后端新增 `GROUP BY from_agent,thread_id` 端点，P1） |
| 乐队 chip 态徽章 | `AgentLiveState.state`（七态） | live-only | `web/src/api/types.ts:429-472`；`crates/swarmx-protocol/src/ws_swarm.rs:102-118` | 冷加载用 `resolveMemberVisual` 兜底（已含 `last_activity_at`/`last_error` 持久化兜底） |
| 乐队 chip "正在<动作>"小字 | `AgentLiveState.activity.label` → `activityVerb()` | live-only | `web/src/api/types.ts:454-462` | 冷加载无 activity → 显态徽章文案，不显动作 |
| live 行动词（第一信号） | `formatActivityLine(live).label` → `activityVerb()` | live-only | `web/src/lib/agent.ts:363-372` | **已实现** formatActivityLine |
| live 行计时 | `formatActivityLine(live).elapsedMs` → `formatDelta` | derived | `web/src/lib/agent.ts:363-372`；`AgentDrawer.tsx:96-104`（formatDelta） | running 用 at→now，settled 用 duration_ms |
| live 行「上次输出 Ns前」 | `AgentInfo.last_activity_at` | **persisted** | `web/src/api/types.ts:118-123`；`crates/swarmx-storage/src/store.rs:438-452`（touch_agent_activity）；migration 0013 | **已持久化**，冷加载可用 |
| 45s 无活动降级 | `last_activity_at` / `activity.at` vs now（前端阈值） | derived | `web/src/lib/agent.ts:126-128`（STALL/STARTUP/NO_RESPONSE 常量） | **missing 后端信号**：纯前端时间比较，无服务器 heartbeat 事件（P2 增强可选） |
| 死即移除（agent 死） | `AgentState::Error/Exited` + `killed_at`/`shim_exit` | persisted（硬字段）/ live（state） | `web/src/api/types.ts:429-441`；`crates/swarmx-server/src/routes/rest.rs:731-774` | **已实现**于 resolveMemberVisual（killed/exit 硬态优先） |
| 焦点 [活动] tab | `AgentActivity[]` via `GET /api/agent/:id/activity` | **live-only（缺口）** | `crates/swarmx-server/src/routes/rest.rs:1125-1136`（in-memory ring） | **missing：依赖后端新增 agent_activity 表 + 端点走 DB**（诊断 3 硬依赖，P1）。未补前：重连后活动 tab 可能空，需显「正在恢复活动…」而非空白 |
| 焦点 [终端] tab | `XtermPane`（PTY 流） | live-only | `AgentDrawer.tsx:62`（XtermPane）；migration sessionStorage lastSeq | **已实现**（gap-replay 重连） |
| 焦点 [变更] tab | `threadDiff` 端点 | live-only | `web/src/api/http.ts:321-325`；`crates/swarmx-server/src/routes/workspaces.rs:816-821` | 复用现有 diff（仅文件名，无 hunk）；行级评论是别的工单 |
| 焦点 [录像]/[消息]/[上下文] | AgentDrawer 原五 tab 数据 | persisted | `AgentDrawer.tsx:82-93` | **原样复用** |
| 失败 chip / 焦点失败卡 | `last_error`/`last_error_kind`/`last_error_at` | **persisted** | `web/src/api/types.ts:124-134`；migration 0022；`OrchestratorFailureCard.tsx` | **已持久化**，冷加载可重放失败卡 |
| 角色色 / 角色名 | `AgentInfo.role` → `roleColorClass` | persisted | `web/src/api/types.ts:84`；`web/src/lib/agent.ts:42-58` | **已实现**，三态全程同色同名 |
| 召出钮派工卡跨高亮 | 共享 `focusedAgentId` 状态 | live-only（UI 态） | 新建（见 §7） | 纯前端 UI 状态 |

---

## 7. 复用 vs 新建（到具体文件/函数）

### 7.1 直接复用（不改）

| 复用物 | 文件 | 用途 |
|---|---|---|
| `AgentDrawer` 五 tab 内容面 | `web/src/components/agent/AgentDrawer.tsx:82-93` | 焦点模式五 tab 内容**原样搬入**，不重写 |
| `AgentActivityLog` | `web/src/components/agent/AgentActivityLog.tsx` | 焦点 [活动] tab + band 深度 [活动] tab |
| `XtermPane` | `web/src/components/XtermPane.tsx` | 焦点 [终端] tab |
| `resolveMemberVisual` / `formatActivityLine` | `web/src/lib/agent.ts:265-372` | 三态全部态点/降级/two-signal 决策——**这是承重复用** |
| `roleColorClass` / `roleInitial` | `web/src/lib/agent.ts:42-58` | 全程角色色/头像 |
| `formatDelta` | `AgentDrawer.tsx:96-104` | 计时格式化（建议提取到 `lib/time.ts` 共享） |
| `OrchestratorFailureCard` | `web/src/components/workspace/OrchestratorFailureCard.tsx` | 焦点模式失败成员卡（泛化为任意 agent_id） |
| `useSwarmFeed` | `web/src/hooks/useSwarmFeed.ts` | 订阅 AgentState/AgentActivity 实时流 |
| `Tabs/TabsList/TabsTrigger`、`Sheet` | `@/components/ui/*` | band 深度 tab + 1280 覆盖滑出 |

### 7.2 改造

| 改造物 | 文件 | 改动 |
|---|---|---|
| `AgentDrawer` Header 拆分 | `AgentDrawer.tsx:350-459` | 把 Header（头像+状态+ActionRow）提取为 `<AgentFocusHeader>`，供焦点模式复用；移除 880px Sheet 外框依赖 |
| `OrchestratorFailureCard` 泛化 | `OrchestratorFailureCard.tsx` | 参数化 agent_id（现仅 orchestrator），按 `last_error_kind` 切按钮组（auth/rate_limit/watchdog） |
| 旧 i18n key 清洗 | `web/src/i18n/locales/zh.json` `agent.injectPlaceholder`/`agent.confirm.wake.desc` | 焦点模式 ActionRow 复用到的 key 去行话（wake/mailbox/blackboard → 唤起/共享区） |
| `formatDelta` 提取 | `AgentDrawer.tsx:96` → `web/src/lib/time.ts` | 多处共享，避免重复 |

### 7.3 新建

| 新建物 | 建议文件 | 职责 |
|---|---|---|
| `<WorkPanel>` 容器 | `web/src/components/workspace/WorkPanel.tsx` | 三态状态机 + 断点 + 转场 + 跨高亮 props |
| `<PulseRail>` | `web/src/components/workspace/PulseRail.tsx` | 54px 脉搏条（态点列 + 召出钮 + 溢出） |
| `<BandView>` | `web/src/components/workspace/BandView.tsx` | 乐队栏 + live 区 + 深度 tab 组合 |
| `<MemberBand>` | `web/src/components/workspace/MemberBand.tsx` | 横向 chip 列（角色色+态徽章+动作小字） |
| `<LiveNow>` | `web/src/components/workspace/LiveNow.tsx` | live 顶置区（two-signal 行 + ⌃▣） |
| `<FocusView>` | `web/src/components/workspace/FocusView.tsx` | 单成员焦点（复用 AgentFocusHeader + 五 tab） |
| `activityVerb(label)` | `web/src/lib/agent.ts`（追加） | label → 用户动词（防火墙翻译，§4.5） |
| `usePanelMode()` hook | `web/src/hooks/usePanelMode.ts` | `pulse`/`band`/`focus` + `focusedAgentId` + 断点观察，跨高亮事件总线 |

---

## 8. 交互与时序

### 8.1 事件 → 状态转换

| 事件 | 从 | 到 | 时序/动画 |
|---|---|---|---|
| 点脉搏条 `▣` 召出钮 | pulse | band | 滑出 180ms ease-out，脉搏条淡出 100ms |
| 点脉搏条某态点 | pulse | focus(该成员) | 召出 + 直接进焦点（跳过 band），160ms |
| 点乐队 chip | band | focus(该成员) | 内容横移 160ms，乐队栏收为面包屑 |
| 点流内派工卡（跨区） | 任意 | focus(该成员) | 若 pulse 先升 band 再焦点；`focusedAgentId` 经事件总线 |
| 点焦点头 `‹ 全部` | focus | band | 反向横移 160ms，恢复滚动位 |
| 点 band 顶栏 `✕`（仅覆盖态） | band/focus | pulse | 滑回 180ms |
| 窗口 resize 跨 1536↑ | pulse | band（常驻） | 无动画，布局直切；focus 保持 |
| 窗口 resize 跨 1536↓ | band（常驻） | pulse | 同上；若在 focus 则降为覆盖 focus |
| AgentState→error | 任意 | 该成员置顶红点；live 行移除 | 即时，无动画跳变；防抖见 8.3 |

### 8.2 阈值

| 阈值 | 值 | 来源 |
|---|---|---|
| 工具 running 卡死判定 | 300_000ms | `lib/agent.ts:126` STALL_RUNNING_MS |
| 启动宽限期 | 45_000ms | `lib/agent.ts:127` STARTUP_GRACE_MS |
| 无响应判死 | 300_000ms | `lib/agent.ts:128` NO_RESPONSE_MS |
| live 行「45s 无活动」降级文案 | 45_000ms | §4.2 设计要求（注：与 STARTUP_GRACE 同值，复用） |
| 断点常驻 | ≥1536px | §3 设计档 |
| 脉搏点溢出折叠 | >6 成员 | 本规格定（54px 容纳约 6 行 36px） |

### 8.3 防抖 / 节流

- **态点闪烁防抖**：AgentActivity 高频时（WS drops frames 风险），live 行计时用 `requestAnimationFrame` 节流到每秒 1 次重渲染，不每帧。
- **死即移除防抖**：收到 `error` 后延迟 **300ms** 再从 live 区移除该行（避免 error→recovered 抖动；`resolveMemberVisual` 的 `recoveredSinceError` 守卫已处理恢复窗口）。
- **resize 防抖**：断点观察 `ResizeObserver` 节流 150ms。

### 8.4 键盘

| 键 | 动作 |
|---|---|
| `⌘1/2/3` | band 深度 tab 切 [活动]/[变更]/[终端]（对齐 §3 设计档） |
| `Esc` | focus → band；band(覆盖态) → pulse |
| `⌃▣`（鼠标）等价键 | `⌘\` 焦点该成员终端（可选） |
| Tab 焦点序 | 召出钮 → chip 列 → live 行 → tab → 内容 |

### 8.5 可达性 aria

| 元素 | aria |
|---|---|
| 脉搏条容器 | `role="navigation"` `aria-label="成员状态"` |
| 召出钮 | `aria-label="展开工作面板"` `aria-expanded` |
| 态点 | `role="button"` `aria-label="{{role}} {{stateText}}"`（状态读出，非颜色） |
| live 行降级 | `aria-live="polite"`：「{{role}} 已 {{delta}} 无活动」 |
| 失败 chip | `aria-label="{{role}} 卡住"` + `aria-describedby` 指 tooltip |
| band/focus 切换 | `role="tablist"`（深度 tab）；焦点头返回钮 `aria-label="返回全队视图"` |
| 覆盖态滑出 | focus trap（Sheet 已内建），Esc 关闭 |
| 态色不单独承载信息 | 每个色点必带文字 aria-label（满足色盲可达性） |

---

## 9. 验收标准

### 9.1 形态与断点

- [ ] 1280–1535px 默认渲染 54px 脉搏条；无成员时仅留召出钮，不渲染「暂无」字。
- [ ] 1536px+ 默认渲染 band 常驻（宽 `clamp(360,26vw,440)`），对话主轴不被推到全宽。
- [ ] 1280 点召出 → band **覆盖滑出**（420px + 投影），不挤压对话；Esc / `✕` 收回。
- [ ] resize 跨 1536 双向切换，band↔pulse 布局正确，focus 状态不丢。
- [ ] band → focus → band 可逆，返回后滚动位置保留。

### 9.2 跨高亮

- [ ] 点脉搏点 / 乐队 chip / 流内派工卡，三条路径都进入**同一成员**焦点。
- [ ] 焦点中的成员在脉搏条/乐队栏有视觉高亮（同 `focusedAgentId`）。

### 9.3 数据绑定与复用

- [ ] 三态态点全部来自 `resolveMemberVisual`，**无任何新启发式**重复实现。
- [ ] live two-signal 来自 `formatActivityLine` + `last_activity_at`，不自己解析 worktree 日志。
- [ ] 焦点五 tab 是 `AgentDrawer` 内容面复用，非重写；终端走 `XtermPane` gap-replay。
- [ ] 角色色/名在脉搏/乐队/live/焦点四处**完全一致**，无裸 `worker_7` ID 出现。

### 9.4 诚实性断言（不许撒谎）

- [ ] **首个真实输出前绝不出现绿色点**：`!shim_ready` 显青「启动中」，非绿。
- [ ] **agent 死（error/exited/killed）后，对应 live 行在 ≤300ms 内消失**，态点转红/灰；脉搏条不再有该成员的运行中脉冲。
- [ ] **45s 无活动 → 灰色 `已 Ns 无活动`**，绝不继续显示运行中 typing（直接验诊断 2）。
- [ ] **失败信息只在焦点失败卡讲一次完整版**；脉搏/乐队只显点+短 tooltip，**不重复整段错误、不截断**（验诊断 6）。
- [ ] **重连后活动 tab 不显示假数据**：DB 端点未就绪时显「正在恢复活动…」而非空白冒充无活动（依赖后端新增 agent_activity 表，缺口已标注，验诊断 3）。
- [ ] 态点颜色不单独承载状态：每个点带文字 aria-label，色盲可读。

### 9.5 缺口显式声明（必须在 PR 描述写明）

- [ ] **依赖后端新增** `agent_activity` 表 + `GET /api/agent/:id/activity` 走 DB（P1）——未就绪时焦点活动 tab 走「恢复中」降级。
- [ ] **依赖后端新增** 精确未读端点（`GROUP BY from_agent,thread_id`，P1）——未就绪时数字徽章隐藏或显总未读，不显假精确数。
- [ ] **missing 后端信号**：45s 无活动为纯前端时间比较，无服务器 heartbeat 事件（P2 可选增强）。

---

**关键文件锚点（绝对路径）**：
- 复用核心：`/Users/wdx/opc/swarmx-core/web/src/lib/agent.ts`（`resolveMemberVisual` :265-358、`formatActivityLine` :363-372、`roleColorClass` :42-58、阈值常量 :126-128）
- 右面板支点：`/Users/wdx/opc/swarmx-core/web/src/components/agent/AgentDrawer.tsx`（五 tab :82-93、Header/ActionRow :350-459、`formatDelta` :96-104）
- 类型/信号：`/Users/wdx/opc/swarmx-core/web/src/api/types.ts`（`AgentInfo` :81-135、`AgentLiveState`/`SwarmAgentState` :429-472）
- 失败卡：`/Users/wdx/opc/swarmx-core/web/src/components/workspace/OrchestratorFailureCard.tsx`
- 后端缺口：`/Users/wdx/opc/swarmx-core/crates/swarmx-server/src/routes/rest.rs:1125-1136`（activity 端点服务 in-memory ring，P1 硬依赖）、`/Users/wdx/opc/swarmx-core/crates/swarmx-storage/src/store.rs`（`touch_agent_activity` :438-452、未读 :1531-1577）
- design token：`/Users/wdx/opc/swarmx-core/web/src/styles/global.css`（agent 色 :103-110、state 色 :69-75、status 色 :82-100、radius :129-133、foreground/surface/border :21-44）
- i18n：`/Users/wdx/opc/swarmx-core/web/src/i18n/locales/zh.json`（现有 `agent.tabs.*` / `agent.wake/pause/resume` 复用；`agent.injectPlaceholder`/`agent.confirm.wake.desc` 存行话泄漏需清洗）
