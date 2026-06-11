I now have all the verified anchors I need: design tokens, i18n namespace conventions (nested `chat.*`, `merge.*`, `agent.tabs.*`), the failure card structure, diff/merge types, MessageMeta shape, and agent color helpers. I have everything to write the spec.

# 组件区规格：计划卡 + 变更评审 + 合并闸

> 读者：接手实现的工程师。本规格覆盖 §4.3 表格中的 **计划 / 交付 / diff·变更 / 合并** 四行（审批卡与派工卡另有规格，本文只在交接处引用）。所有动态元素均标注数据来源、持久化状态与 `file:line` 锚点；凡数据缺失处明确写出"依赖后端新增 X"。所有路径为绝对路径。

---

## 1. 目的与边界

### 1.1 解决哪条诊断/原则

| 子组件 | 治哪条诊断 | 落哪条原则 |
|---|---|---|
| ① 计划置顶粘性卡 | 诊断 5（最佳教学时机浪费）、诊断 1（payload 不显眼，计划是 payload 中最重要的那块） | 原则 3 单声道主轴、原则 5 渐进披露、原则 1 诚实状态（队长不给计划就不假装有计划） |
| ② 交付卡 | 诊断 4（firehose——交付是"凝练结果冒泡"，不是逐 token 流） | 原则 4 对话/活动分离、原则 5 渐进披露 |
| ③ 变更评审 | 诊断 4（完整 diff 退到可展开层，不进流） | 原则 4 分离、原则 5 渐进披露、原则 7 稳定身份（行评论按文件归属成员色） |
| ④ 合并闸 | rCoding 共识"门从写前移到合并前"；诊断 1（收口动作是 payload，必须显眼且诚实） | 原则 1 诚实状态（四条件未全真不点亮）、原则 6 可操控 |

### 1.2 不做什么（边界）

- **不做 DAG 全图**：派工小团队树是另一份规格（§4.3 派工行 + `Dag.tsx` 内联重写）。本组件的计划卡只显示 checklist 项 → 拥有成员的**一对一映射**，不画依赖边。
- **不做 approve-before-run 自治级字段**：计划卡的 `[批准计划]` 闸门是**可选 UI**，依赖后端新增"会话级自治级 + spawn 挂起"（§6 P2 唯一 L 级后端）。本规格定义闸门**关闭时**（默认）的全部行为，闸门开启路径标注为「依赖后端」。
- **不做测试 runner**：合并闸的「测试通过」条件依赖后端新增 `test_runs` 表与 runner（数据地图标 missing）。本规格定义该条件为 **`unknown` 三态之一**，未接入时显示「未运行测试」灰态而非假绿，**不阻塞合并**（见 §4 合并闸阈值）。
- **不做 user→user 回复线程**：交付卡/变更卡的 `[查看变更]` 不创建子会话。
- **不重写 worktree diff 计算**：复用 `GET /api/workspaces/:id/threads/:tid/diff`（`crates/flockmux-server/src/routes/workspaces.rs:816-821`），仅在它返回的 `files: string[]` 之上叠加前端逐文件展开与新增的行级 hunk 端点。

---

## 2. 完整状态枚举

### 2.1 计划卡（`PlanStickyCard`）

| # | 状态 | 触发 | 关键视觉 |
|---|---|---|---|
| P-0 | **无计划（降级）** | 队长首条消息不含可解析的计划结构 | 不渲染粘性卡；轴顶一条 hairline 提示「队长尚未给出计划」，不阻塞流 |
| P-1 | **加载中** | 已请求台账但 markdown 未到 / 解析中 | 骨架卡（3 行灰条 skeleton），卡头健康度点为 `◐ 启动中`（amber） |
| P-2 | **进行中（活）** | 解析出 ≥1 checklist 项，队长 `AgentState` 非 error/exited | 完整 checklist，每项 `✓/◐/○` + 拥有成员徽章；卡头绿健康度点 |
| P-3 | **待批准（闸门开）** | `meta.subtype='plan_proposed'` 且会话自治级=需批准（依赖后端） | 卡底 `[批准计划][改]` 行动条，琥珀左边框；checklist 项全为 `○`（未派工） |
| P-4 | **队长不健康** | 队长 `AgentState=Error/Exited` 或 90s 看门狗触发 | checklist 冻结为最后已知态（灰化），卡头健康度点翻 `✕ 卡住` + 一行原因，链到失败卡 |
| P-5 | **全部完成** | 所有项 `✓` | 卡折叠为一行「计划 3/3 已完成」，可点开；不再粘顶（normal flow） |
| P-6 | **解析部分失败** | markdown 有计划块但某些行无法判定状态 | 能解析的项正常显示，无法判定的项显示为 `·` 中性点 + tooltip「状态未知」，不假装 `✓` |

### 2.2 交付卡（`DeliveryCard`）

| # | 状态 | 触发 | 关键视觉 |
|---|---|---|---|
| D-0 | **加载中** | 收到 `meta.subtype='delivery'` 但 diff 计数未回填 | 「正在汇总变更…」+ skeleton 计数 |
| D-1 | **常态（有变更）** | delivery 事件含 `files/insertions/deletions` | `N 文件 +x/−y`，测试输出折叠，`[查看变更]` 主按钮 |
| D-2 | **无文件变更** | delivery 事件 `files=[]` | 「未改动文件」灰态，`[查看变更]` 禁用 + tooltip「本次无变更」 |
| D-3 | **测试有输出** | `meta.test_output` 非空 | `▸ 测试输出` 折叠块默认收起；若 `test_status=fail` 折叠头红点 |
| D-4 | **diff 拉取失败** | `[查看变更]` 点击后 diff 端点 4xx/5xx | 卡内 inline 错误条「变更暂时打不开 · 重试」，不跳走 |

### 2.3 变更评审（`ChangeReviewPanel` / `ChangeReviewExpander`）

| # | 状态 | 触发 | 关键视觉 |
|---|---|---|---|
| R-0 | **空（无变更）** | `diff.files=[]` | 「这条会话还没有改动」居中说明 |
| R-1 | **加载文件列表** | diff 请求 in-flight | 文件列表骨架（虚拟化容器 + 5 行 skeleton） |
| R-2 | **列表就绪·全部折叠** | `diff.files` 到达 | 虚拟化文件列表，每行 = 文件名 + `+x/−y` + per-file `accept` 复选 + `已看` 勾 |
| R-3 | **单文件展开** | 点击文件行 | 该文件 hunk 内联渲染（依赖后端新增逐文件 hunk 端点）；行级可点起评论 |
| R-4 | **行评论草稿** | 点某行 → `让队长改这里` | 行下浮出评论输入框，预填 `file:line`，`[发给 成员名][取消]` |
| R-5 | **评论已发·未解决** | 评论 POST 成功（依赖后端 `review_comments` 表） | 行旁琥珀气泡角标；评论计入合并闸「评论已解决」分母 |
| R-6 | **评论已解决** | 成员回应后标记 resolved（依赖后端 `resolved_at`） | 角标转灰勾 |
| R-7 | **base 脏（不可 rebase）** | `diff.base_dirty=true` | 顶部琥珀横幅「主线有未提交改动，先处理才能合并」；合并闸 rebase 条件红 |
| R-8 | **1280 流内大卡** | 1280px 档点 `[查看变更]` | 在流内**就地展开**为大卡（虚拟化 file list + 逐文件折叠），对话被推下但不跳视图 |
| R-9 | **1536+ 右面板 tab** | 视口 ≥1536 | 不进流，进右面板「变更」tab；对话不被推走 |
| R-10 | **hunk 端点缺失（降级）** | 后端逐文件 hunk 端点未上线 | 文件列表可见但行不可展开；每行右侧 `在终端看 diff ⌃▣` 兜底，不假装能行级评论 |

### 2.4 合并闸（`MergeGate`）

| # | 状态 | 触发 | 关键视觉 |
|---|---|---|---|
| G-0 | **未满足（默认）** | 四条件未全真 | `[合并回 main]` 灰/禁用；四条件 checklist 显示各自 `✓/○/✕/灰` |
| G-1 | **全满足·点亮** | 四条件全 `✓`（测试条件可为「跳过」白名单态） | `[合并回 main]` 主按钮高亮（accent 实底） |
| G-2 | **合并中** | 点击后 POST in-flight | 按钮 spinner「合并中…」，四条件锁定 |
| G-3 | **合并成功** | `MergeResult.status='merged'` | 绿 toast +「已合并 N 文件回 main」系统卡入流；评审面收起 |
| G-4 | **冲突·已派解决成员** | `MergeResult.status='resolving'` | 琥珀卡「有冲突，已派 成员名 解决」+ 冲突文件列表 + `⌃▣ 看终端` |
| G-5 | **合并失败（非冲突）** | POST 4xx/5xx | 红 inline 错误条 + `重试`，四条件解锁 |
| G-6 | **测试未接入（诚实）** | `test_runs` 表/runner 未上线 | 「测试通过」条件显示灰 `· 未运行测试`，**不阻塞**点亮（不假绿、不假门） |

---

## 3. 逐状态 ASCII 线框

### 3.1 计划卡 — P-2 进行中（粘顶，1280px 主轴内，max-w≈720）

```
┌─ 计划 1/3 ───────────────────────────────────  ● 队长 健康 ─┐  ← 卡头 28px:左标题/右健康度点
│                                                              │     圆角 --radius-xl(12)
│  ✓  抽出 validateRefundAmount              〔队长〕          │  ← 行高 28px,项间 --spacing-2(8)
│  ◐  补失败用例                              〔测 测试成员〕  │  ← ◐=amber旋转;成员徽章=角色色soft底
│  ○  跑全套测试                              〔测 测试成员〕  │  ← ○=idle灰描边圈
│                                                              │
└──────────────────────────────────────────────────────────────┘
        粘性:position sticky top:0,滚动时钉在主轴顶,投 --shadow 区分
```

### 3.2 计划卡 — P-0 无计划（降级，不阻塞流）

```
┄┄┄┄┄┄┄┄┄  队长尚未给出计划 · 先聊聊你想做什么  ┄┄┄┄┄┄┄┄┄   ← 居中 hairline,foreground-tertiary
                                                                11px,无边框无背景,可点→滚到Composer
```

### 3.3 计划卡 — P-3 待批准（闸门开，依赖后端）

```
┌─ 计划草案 0/3 ──────────────────────────────  ● 队长 待你确认 ─┐  ← 左边框 4px amber
│  ○  抽出 validateRefundAmount              〔队长〕            │
│  ○  补失败用例                              〔测 测试成员〕    │
│  ○  跑全套测试                              〔测 测试成员〕    │
│ ────────────────────────────────────────────────────────────  │
│  按此计划开始？派工后成员将各自动手。      [改计划]  [批准并开始]│ ← 行动条 36px;主按钮 accent
└────────────────────────────────────────────────────────────────┘
```

### 3.4 计划卡 — P-4 队长不健康

```
┌─ 计划 1/3 ───────────────────────────────  ✕ 队长卡住,点查看 ─┐ ← 健康度点红,整条可点→失败卡
│  ✓  抽出 validateRefundAmount              〔队长〕    (灰化)  │  ← 全卡 opacity .55,冻结最后态
│  ◐  补失败用例                              〔测 测试成员〕    │
│  ○  跑全套测试                              〔测 测试成员〕    │
└────────────────────────────────────────────────────────────────┘
```

### 3.5 交付卡 — D-1 / D-3（常态 + 测试输出）

```
┌▏交付  测试成员 完成了「补失败用例」 ────────────────────────┐  ← 左色条4px=成员角色色
│                                                              │     居中卡,max-w≈680
│   3 文件   +47 −12        ▸ 测试输出 (12 passed) ●           │  ← 计数 mono;折叠头;red点=有fail
│                                                              │
│                                          [查看变更]          │  ← 主按钮,右对齐
└──────────────────────────────────────────────────────────────┘
```

### 3.6 变更评审 — R-2/R-3 1536+ 右面板「变更」tab

```
┌ [活动] [变更 3] [终端] ─────────────────────────⤢─┐  ← tab 行,复用 AgentDrawer
│ 主线有未提交改动,先处理才能合并         ⚠         │  ← R-7 base_dirty 时显示,否则隐藏
│ ──────────────────────────────────────────────────│
│ ☑已看 ☐采纳  src/refund.ts            +31 −8   ▾  │  ← 文件行 32px,虚拟化
│ ┌──────────────────────────────────────────────┐ │  ← R-3 展开:hunk
│ │  12 │  export function validateRefundAmount(  │ │
│ │  13 │+   if (amount <= 0) throw new Error(…)   │ │  ← +绿底/−红底,行号 mono dim
│ │  14 │+   if (amount > MAX) throw …            ●│ │  ← 行尾 ● = 点起评论热区(hover显)
│ │  ┌ 让队长改这里 ─────────────────────────────┐ │ │  ← R-4 行评论草稿(预填 refund.ts:14)
│ │  │ 这里漏了精度校验…                          │ │ │
│ │  │                          [取消] [发给 测试]│ │ │  ← 发送=带 file:line 发成员
│ │  └────────────────────────────────────────────┘ │ │
│ └──────────────────────────────────────────────┘ │
│ ☑已看 ☑采纳  src/refund.test.ts       +16 −4   ▸  │
│ ☐已看 ☐采纳  src/types.ts             +0  −0   ▸  │  ← R-2 折叠态
│ ──────────────────────────────────────────────────│
│ ┌ 合并闸 ─────────────────────────────────────┐  │  ← MergeGate,见 3.8
└──────────────────────────────────────────────────┘
```

### 3.7 变更评审 — R-8 1280px 流内就地展开（大卡）

```
        你：把校验抽成独立函数
        队长：好,我拆两步…
┌──────────────────────────────────────────────────────────┐  ← 流内大卡,撑满主轴宽
│ 变更 · 退款流程  3 文件 +47 −12                  [收起 ▴] │  ← 卡头,[收起]还原为交付卡
│ ──────────────────────────────────────────────────────── │
│ (同 3.6 的文件列表 + hunk + 行评论,虚拟化滚动 max-h:60vh) │  ← 虚拟化防大diff撑爆(诊断风险点)
│ ──────────────────────────────────────────────────────── │
│ [合并闸]                                                  │
└──────────────────────────────────────────────────────────┘
        ┌ 发消息给队长… ┐  (Composer 仍在卡下方,不被顶走)
```

### 3.8 合并闸 — G-0 未满足 / G-1 点亮 / G-6 测试未接入

```
┌ 合并闸 ────────────────────────────────────────────────┐
│  ✓  全部 3 文件已看                                      │  ← ✓ success / ○ idle / ✕ danger
│  ✓  可干净 rebase 到 main                                │
│  ·  未运行测试                                  跳过 ▾   │  ← G-6:灰中性点,非红非绿;可手动「跳过」
│  ○  2 条评论待解决                                       │  ← 未解决数>0 时 ○;=0 时 ✓
│ ──────────────────────────────────────────────────────  │
│                                    [ 合并回 main ]       │  ← G-0:禁用灰 / G-1:accent实底
└──────────────────────────────────────────────────────────┘

G-4 冲突态:
┌ 合并遇到冲突 ──────────────────────────────────────────┐  ← 琥珀左边框
│  已派 修复成员 解决冲突 · 涉及 src/types.ts             │
│                                      [⌃▣ 看终端]        │
└──────────────────────────────────────────────────────────┘
```

---

## 4. 精确中文文案（含 i18n key）

i18n 约定：嵌套对象，沿用现有 `chat.*` / `merge.*` / `agent.tabs.*` 命名（见 `web/src/i18n/locales/zh.json`）。本组件统一前缀 `chat.review.*`。所有字符串严守行话防火墙——已逐条检查无 mailbox/blackboard/wake/worktree/shim/spell/PTY/handoff。

```jsonc
"chat": {
  "review": {
    // —— 计划卡 ——
    "plan": {
      "title": "计划 {{done}}/{{total}}",                 // P-2
      "titleDraft": "计划草案 {{done}}/{{total}}",         // P-3
      "titleDone": "计划 {{total}}/{{total}} 已完成",      // P-5
      "captainHealthy": "队长 健康",                       // P-2 卡头
      "captainProposing": "队长 待你确认",                 // P-3
      "captainStuck": "队长卡住，点查看",                  // P-4(整条链到失败卡)
      "noPlan": "队长尚未给出计划 · 先聊聊你想做什么",     // P-0 hairline
      "ownerSelf": "队长",                                 // 拥有成员=队长本人
      "stateUnknown": "状态未知",                          // P-6 tooltip
      "approvePrompt": "按此计划开始？派工后成员将各自动手。", // P-3 行动条
      "editPlan": "改计划",                                // P-3
      "approveRun": "批准并开始"                           // P-3
    },
    // —— 交付卡 ——
    "delivery": {
      "summarizing": "正在汇总变更…",                      // D-0
      "title": "{{member}} 完成了「{{task}}」",            // D-1 左色条标题
      "fileCount": "{{count}} 文件",                       // D-1
      "diffStat": "+{{ins}} −{{del}}",                     // D-1 (mono)
      "testOutput": "测试输出",                            // D-3 折叠头
      "testPassed": "{{count}} 项通过",                    // D-3
      "noChanges": "未改动文件",                           // D-2
      "viewChanges": "查看变更",                           // D-1 主按钮
      "openFailed": "变更暂时打不开",                      // D-4
      "retry": "重试"                                      // 通用
    },
    // —— 变更评审 ——
    "changes": {
      "tab": "变更",                                       // 右面板 tab 名(badge 用 count)
      "empty": "这条会话还没有改动",                       // R-0
      "loadingFiles": "正在读取改动文件…",                 // R-1
      "headerStat": "{{count}} 文件 +{{ins}} −{{del}}",    // R-8 流内卡头
      "collapse": "收起",                                  // R-8
      "baseDirty": "主线有未提交改动，先处理才能合并",     // R-7 横幅
      "reviewed": "已看",                                  // R-2 复选
      "accept": "采纳",                                    // R-2 复选(per-file)
      "askCaptainHere": "让队长改这里",                    // R-3 行 hover 起评论
      "commentPlaceholder": "说说这一行哪里要改…",         // R-4 输入框
      "sendTo": "发给 {{member}}",                         // R-4 发送(带 file:line)
      "cancel": "取消",
      "commentSent": "已发给 {{member}}",                  // R-5 toast
      "viewInTerminal": "在终端看 diff",                   // R-10 降级兜底
      "hunkUnavailable": "逐行变更暂不可用，可在终端查看"  // R-10 tooltip
    },
    // —— 合并闸 ——
    "gate": {
      "title": "合并闸",                                   // 闸标题
      "condReviewed": "全部 {{count}} 文件已看",           // 条件1 ✓
      "condReviewedPartial": "还有 {{remain}} 个文件没看",  // 条件1 ○
      "condRebase": "可干净 rebase 到 main",               // 条件2 ✓
      "condRebaseBlocked": "主线有改动，暂时合不了",       // 条件2 ✕(base_dirty)
      "condTests": "测试通过",                             // 条件3 ✓
      "condTestsNone": "未运行测试",                       // G-6 灰中性
      "condTestsFail": "测试未通过",                       // 条件3 ✕
      "condTestsSkip": "跳过",                             // G-6 手动跳过开关
      "condComments": "评论已解决",                        // 条件4 ✓
      "condCommentsOpen": "{{count}} 条评论待解决",         // 条件4 ○
      "mergeButton": "合并回 main",                        // 主按钮
      "merging": "合并中…",                                // G-2
      "merged": "已合并 {{count}} 文件回 main",            // G-3 系统卡/toast
      "conflictTitle": "合并遇到冲突",                     // G-4
      "conflictBody": "已派 {{member}} 解决冲突 · 涉及 {{files}}", // G-4
      "viewTerminal": "看终端",                            // G-4
      "mergeFailed": "合并没成功，再试一次"                // G-5
    }
  }
}
```

复用现有 key（不新建）：`common.copy`/`common.copied`、`merge.*`（已存在合并相关命名空间，链接复用）、`agent.tabs.terminal`/`agent.tabs.activity`。en.json 需同步补齐相同 key 结构。

---

## 5. 尺寸/间距/色彩 token

全部引用 `web/src/styles/global.css` 的现有 token，**不新增颜色**。

### 5.1 色彩映射

| 用途 | token | 值 |
|---|---|---|
| 卡正文（payload 高对比，治诊断1） | `--color-foreground-primary` | #0F172A |
| 次要文案/计数标签 | `--color-foreground-secondary` | #334155 |
| hairline 提示/dim 行号 | `--color-foreground-tertiary` | #64748B |
| `✓` 完成 / 健康点 / 合并成功 | `--color-status-success` (`-soft` 底) | #15803D / #DCFCE7 |
| `◐` 进行中 / `◐启动中` 健康点 | `--color-status-busy` (`-soft` 底) | #B45309 / #F59E0B1F |
| `○` 未开始 / 中性条件 | `--color-status-idle` / `--color-state-idle` | #15803D / #94A3B8 |
| `✕` 卡住 / 失败 / rebase 阻塞 | `--color-status-danger` (`-soft` 底) | #B91C1C / #FEE2E2 |
| 待批准/冲突/base_dirty 左边框与横幅 | `--color-status-warning` (`-soft` 底) | #B45309 / #FEF3C7 |
| 合并主按钮/查看变更主按钮实底 | `--color-accent-primary` (hover `-deep`) | #2563EB / #1D4ED8 |
| diff `+` 行底 | `--color-status-success-soft` | #DCFCE7 |
| diff `−` 行底 | `--color-status-danger-soft` | #FEE2E2 |
| 卡背景 | `--color-surface-elevated` | #FFFFFF |
| 卡边框 | `--color-border-subtle` | #E2E8F0 |
| 成员徽章/左色条 | `--color-agent-{role}` + `roleColorClass()` | 见 `web/src/lib/agent.ts:42` |

`·` 中性点（P-6 状态未知 / G-6 测试未运行）= `--color-foreground-tertiary`，**故意区别于** `✓`/`✕`，杜绝假绿。

### 5.2 尺寸/间距/字体

| 属性 | token / 值 |
|---|---|
| 卡圆角 | `--radius-xl` (12px) |
| 折叠块/行内元素圆角 | `--radius-lg` (8px) / 徽章 `--radius-full` |
| 卡内边距 | `--spacing-4` (16px) 水平 / `--spacing-3` (12px) 垂直 |
| checklist 项间距 | `--spacing-2` (8px) |
| 卡头高度 | 28px；合并闸/批准行动条 36px |
| 文件行高（虚拟化） | 32px（固定，喂给虚拟化 `itemSize`） |
| 标题字体 | `--font-heading` (Inter)，14px / `font-semibold` |
| 正文 | `--font-body` (Geist)，14px / `line-height:1.6` |
| 计数/行号/命令 | `--font-mono` (Geist Mono)，12–13px |
| 卡头副文案/徽章 | `--font-caption` (Funnel Sans)，11px |
| 流内大卡最大高 | `max-h:60vh`，超出虚拟化滚动（防大 diff 撑爆，§7 未决问题 2 的缓解） |

---

## 6. 数据绑定表

每个动态元素 ← 信号/端点；标注 persisted / live-only / **missing（依赖后端新增）**。

| UI 元素 | 数据源 | 持久化 | 锚点 (file:line) |
|---|---|---|---|
| 计划 checklist 项文本 | 台账 markdown `${keyPrefix}task.ledger.md` | persisted（纯 markdown，**无结构化字段**） | `web/src/routes/workspace/views/Ledger.tsx:67-68`；`web/src/routes/workspace/views/Chat.tsx:681-683` |
| 计划项 `✓/◐/○` 状态 | **缺失** — 需前端 markdown parser 或 orchestrator 改结构化格式 | **missing** | 无；**依赖后端新增**：约定 orchestrator 写结构化计划（建议 `task.plan.json` blackboard key，含 `[{text, owner_role, status}]`），或 P1 前端 parser（regex 提 `- [x]/[~]/[ ]`） |
| 计划项「拥有成员」徽章 | **缺失** — 无成员→任务映射字段 | **missing** | 无；**依赖后端新增**：上述 `task.plan.json` 的 `owner_role` 字段；徽章渲染复用 `roleColorClass`/`resolveMemberVisual`（`web/src/lib/agent.ts:42,265`） |
| 卡头队长健康度点 | `AgentInfo` 队长的 `AgentState` + `last_error*` | persisted | `crates/flockmux-protocol/src/ws_swarm.rs:102-118`；`crates/flockmux-storage/src/models.rs:52-65`（迁移 0022 `last_error*`） |
| P-0 降级判定（首条不符计划结构） | 队长首条 `message.body` parser 失败 → P-0 | persisted（消息已落表） | `web/src/components/MessagesPanel.tsx:1278-1380`（队长气泡渲染） |
| 计划卡入流持久化 | **缺失** — 应落 `kind=system, meta.subtype='plan'` | **missing** | **依赖后端新增** plan 系统卡；复用 `messages` kind+meta（`crates/flockmux-storage/src/store.rs:1070-1081`） |
| approve-before-run 闸门（P-3） | **缺失** — 会话级自治级 + spawn 挂起 | **missing** | 无；**依赖后端新增**（§6 P2 唯一 L 级后端）；闸门关闭时本组件不依赖它 |
| 交付卡 `N 文件 +x/−y` | `meta.subtype='delivery'` 的 `files/insertions/deletions` | **missing**（delivery 事件无落库） | **依赖后端新增** delivery 系统卡；diff 数据现状来自 worktree（非 DB）：`web/src/api/http.ts:321-323`（`threadDiff`） |
| 交付卡测试输出折叠 | `meta.test_output` / `test_status` | **missing** | **依赖后端新增** delivery meta 字段（同上）；UI 复用 `MessagesPanel.tsx:1606` 折叠块 |
| 变更文件列表 | `GET …/threads/:tid/diff` → `ThreadDiff.files: string[]` | persisted（live 计算，每次拉取） | `crates/flockmux-server/src/routes/workspaces.rs:816-821`；`web/src/api/types.ts:366-375` |
| 文件级 `+x/−y` | **缺失** — `diff_summary` 只回文件名，无逐文件增删数 | **missing** | **依赖后端新增**：扩展 `/diff` 或新端点返回 per-file `{path, insertions, deletions, hunks}` |
| 逐文件 hunk（行展开 R-3） | **缺失** — 无 hunk 端点 | **missing** | **依赖后端新增** `GET …/threads/:tid/diff/file?path=`（返回 hunks）；缺失时降级 R-10 |
| 行级评论（R-4/R-5） | **缺失** — 无 `review_comments` 表/端点 | **missing** | **依赖后端新增** `review_comments` 表 + `POST …/threads/:tid/comments` + 转成 user→成员消息（复用 `messages` kind+meta） |
| 评论解决态（R-6） | **缺失** — 无 `resolved_at` | **missing** | **依赖后端新增** `review_comments.resolved_at` + 标记端点 |
| per-file accept 复选（R-2） | **缺失** — 无 `file_accept` 表 | **missing** | **依赖后端新增** `file_accept(thread_id, file_path, accepted_by, accepted_at)` |
| per-file「已看」勾（合并闸条件1） | **缺失** — 无 `reviewed_at` | **missing** | **依赖后端新增** `file_accept.reviewed_at` 或 `review_files_read` 表 |
| 合并闸·rebase 条件 | `diff.base_dirty`（preflight 调 `/diff`） | live-only（仅 POST 时算，需 UI 预检调 /diff） | `crates/flockmux-server/src/routes/workspaces.rs:860-865`；`ThreadDiff.base_dirty` (`types.ts:374`) |
| 合并闸·测试条件 | **缺失** — 无 `test_runs` 表/runner | **missing**（G-6 诚实降级，不阻塞） | **依赖后端新增** `test_runs(thread_id, status, output, run_at)` + runner |
| 合并闸·评论解决条件 | 派生自上面 `review_comments` 未解决计数 | **missing**（同评论） | 同评论端点 |
| 合并执行结果 | `POST …/threads/:tid/merge` → `MergeResult` | persisted | `crates/flockmux-server/src/routes/workspaces.rs:882-896`；`web/src/api/types.ts:377-382` |
| 冲突态（G-4）成员/文件 | `MergeResult.status='resolving'` 的 `agent_id`/`files` | persisted | 同上 `types.ts:380-382` |
| 合并成功系统卡（G-3 入流） | **缺失** — 应落 `kind=system, meta.subtype='merged'` | **missing** | **依赖后端新增**；复用 `messages` kind+meta（`store.rs:1070`） |
| 侧栏 dirty/ahead/behind（链接显示） | `ThreadInfo.dirty/ahead/behind` | live-only（列表时算） | `crates/flockmux-server/src/routes/workspaces.rs:51-53`；`types.ts:354-360` |

**缺口汇总（实现前必须确认后端排期）**：计划结构化状态、delivery 系统卡、per-file `+x/−y`、逐文件 hunk 端点、`review_comments`、`file_accept`(+`reviewed_at`)、`test_runs`、合并/计划系统卡。其中**计划卡（P1）、交付卡（P2）、变更评审 + 合并闸（P2）** 的后端依赖均为 missing，前端需以「空/降级态优先」策略实现：先落 R-10/G-6/D-2 这类诚实降级，端点上线后再点亮完整路径。

---

## 7. 复用 vs 新建

| 能力 | 复用 | 改 | 新建 |
|---|---|---|---|
| 卡容器 + 左色条 + 行动按钮布局 | `web/src/components/workspace/OrchestratorFailureCard.tsx`（左 icon + 标题 + 副文 + 按钮行） | 泛化为 `SystemCard` 通用容器（接受 `accentRole`/`leadIcon`/`actions`） | 计划卡 `PlanStickyCard`、交付卡 `DeliveryCard` 各自薄壳 |
| 折叠块（思考/测试输出/hunk） | `web/src/components/MessagesPanel.tsx:1606`（`ReasoningDisclosure`）；`web/src/components/ChatMarkdown.tsx` | 提取为独立 `Disclosure`，默认收起 | — |
| 成员色/名/头像 | `web/src/lib/agent.ts:42`(`roleColorClass`)、`:47`(`roleColorHex`)、`:265`(`resolveMemberVisual`) | — | 计划项 owner 徽章 / 交付卡左色条 / 行评论归属沿用同一色同一名（原则7） |
| 右面板 tab 框架 | `web/src/components/agent/AgentDrawer.tsx:82-92`（5 tab） | 把「变更」作为 `activity` 同级 tab 注入（视口 ≥1536 时挂） | `ChangeReviewPanel` 作为该 tab 内容 |
| diff 数据拉取 | `web/src/api/http.ts:321-323`(`threadDiff`)、`:327-329`(`mergeThread`) | 扩展 http client 增 `diffFile(path)` / `fileAccept` / `postComment` / `mergeGateStatus`（待后端端点） | — |
| 模型切换确认弹窗模式（合并/批准确认复用其交互骨架） | `web/src/components/ModelPicker.tsx:12` 的确认 dialog 模式 | — | 合并前确认 / 批准计划确认套用同一 confirm 组件 |
| 虚拟化文件列表 | 若仓库已有 `@tanstack/react-virtual`，复用；否则 | — | **新建** `VirtualFileList`（固定行高 32px；先用现成虚拟化库，避免手写） |
| 行级评论 widget | `web/src/components/MessagesPanel.tsx:728-766`（@autocomplete，选成员） | 复用成员选择器填 `发给 {{member}}` | **新建** `LineCommentDraft`（预填 file:line） |
| 合并闸状态机 | 现状 `merge_thread_handler` 全有或全无 | — | **新建** `MergeGate` 组件 + `useMergeGate` hook（聚合四条件，建议后端补 `GET …/merge-gate-status` 预检端点，否则前端逐条聚合） |

**新建文件清单（建议路径）**：
- `web/src/components/review/PlanStickyCard.tsx`
- `web/src/components/review/DeliveryCard.tsx`
- `web/src/components/review/ChangeReviewPanel.tsx`（含 `ChangeReviewExpander` 流内变体）
- `web/src/components/review/MergeGate.tsx`
- `web/src/components/review/VirtualFileList.tsx`、`LineCommentDraft.tsx`、`Disclosure.tsx`
- `web/src/components/SystemCard.tsx`（从 `OrchestratorFailureCard` 泛化）
- `web/src/hooks/useMergeGate.ts`、`useThreadDiff.ts`
- `web/src/lib/parsePlan.ts`（P1 前端台账 parser，后端结构化前的桥）

---

## 8. 交互与时序

### 8.1 事件 → 状态转换

**计划卡**
- 队长首条到达 → `parsePlan(body)`：解析出 ≥1 项 → P-2；解析失败 → P-0（hairline，**不阻塞**，不渲染卡）。
- 台账 markdown 变更事件（复用 `Chat.tsx:69-114` 的 breadcrumb 监听）→ 防抖 **300ms** 后重解析 → diff 更新 checklist（项状态翻转有 150ms 颜色过渡）。
- 队长 `AgentState` → `Error/Exited` 或 90s 看门狗 fire → P-4（冻结灰化，健康点链失败卡）。
- 闸门开（依赖后端）：`meta.subtype='plan_proposed'` → P-3，`[批准并开始]` 点击 → **先弹一行确认**「按此计划开始？成员将各自动手 [取消][确认]」→ POST approve 端点（依赖后端）→ 派工开始 → 转 P-2。

**交付卡**
- `meta.subtype='delivery'` 到达 → D-0 → 异步拉 diff 计数回填 → D-1（无文件 → D-2）。
- `[查看变更]` 点击：视口 <1536 → R-8 流内就地展开（交付卡原地变大卡，平滑高度过渡 200ms）；视口 ≥1536 → 激活右面板「变更」tab（R-9），交付卡 `[查看变更]` 变 `[已在右侧打开]` 弱化态。

**变更评审**
- 进入 R-2 时**预拉 `/diff`**（计算 `base_dirty` 供合并闸预检，避免点合并才发现 base 脏）。
- 点文件行 → R-3 展开：若 hunk 端点可用 → 拉 `diffFile(path)` 渲染；否则 R-10 降级（行不可展开，显 `在终端看 diff`）。
- 行 hover **≥120ms** 显示行尾 `●` 评论热区（防抖，避免滚动闪烁）；点 `●` 或 `让队长改这里` → R-4 草稿，输入框 autofocus，预填 `path:line`。
- `[发给 {{member}}]` → POST 评论（依赖后端）→ 乐观 UI 立即显 R-5 角标 + toast；失败回滚草稿不丢。
- per-file `☑采纳` / `☑已看` → 即时 POST `file_accept`（依赖后端），乐观更新，**防抖 400ms** 合并连续勾选。

**合并闸**
- 四条件任一变化 → `useMergeGate` 重算点亮态。点亮规则（G-1）：
  - `已看` = 全部文件 `reviewed_at` 非空（依赖后端）；
  - `rebase` = `diff.base_dirty===false`；
  - `测试` = `test_status==='pass'` **或** 用户手动「跳过」**或** G-6 未接入（诚实降级，**此条不阻塞**，但 UI 显灰中性 `·` 而非假绿）；
  - `评论` = 未解决评论数 === 0。
- **阈值/诚实性**：测试未接入时不得渲染绿 `✓`，必须 `·` 中性；rebase 阻塞渲染 `✕` 红，**禁用** `[合并回 main]`（这是硬闸，base 脏合并必失败）。
- `[合并回 main]` 点击 → **弹确认**「合并会把这条会话的改动并回 main [取消][合并]」→ G-2 → POST merge → `merged` → G-3（绿 toast +「已合并 N 文件」系统卡入流，依赖后端落 `kind=system,subtype=merged`）；`resolving` → G-4（不报错，告知已派成员解决）；4xx/5xx → G-5。

### 8.2 键盘与可达性（aria）

- 计划卡：`role="region"` `aria-label="计划 {{done}}/{{total}}"`；checklist `<ul>`，每项 `<li>` `aria-label="{{text}}，{{statusText}}，由{{owner}}负责"`（statusText = 已完成/进行中/未开始/状态未知，**给屏读器明确说出状态，不靠颜色**）。
- 文件行：`<button>` `aria-expanded` 切换 R-3；复选框真 `<input type=checkbox>` 带 `<label>`，`aria-label="采纳 {{file}}"` / `"已看 {{file}}"`。
- 行评论草稿：输入框 `aria-label="对 {{file}} 第 {{line}} 行的修改意见"`；`Esc` 关闭草稿，`⌘Enter` 发送，`Enter` 换行。
- 合并按钮：禁用时 `aria-disabled` + `aria-describedby` 指向未满足条件文本（如「还有 2 个文件没看」），让屏读器/键盘用户知道**为何不可合并**（原则1诚实，对应 §4.4 Composer 的「为何不可发」同款手法）。
- 焦点管理：流内展开（R-8）后焦点移到大卡标题；右面板 tab 切换 `⌘1/2/3`（沿用设计文档 §3 约定）。
- 所有状态点（`✓◐○✕·`）配文字 label，不做纯色编码（色盲可达）。

---

## 9. 验收标准（可勾选 checklist）

**功能正确性**
- [ ] 计划卡粘顶（sticky）随主轴滚动钉在顶部；全部完成（P-5）后取消粘顶并折叠为一行。
- [ ] 每个 checklist 项渲染 `✓/◐/○` 之一 + 拥有成员徽章（成员色/名复用 `agent.ts`，不出现裸 `worker_7`）。
- [ ] 交付卡显示 `N 文件 +x/−y`；测试输出默认折叠；`[查看变更]` 在 <1536 流内展开、≥1536 进右面板 tab。
- [ ] 变更面文件列表**虚拟化**（≥50 文件不卡顿）；逐文件可折叠展开。
- [ ] 行级评论可点行 → 预填 `file:line` → 发给指定成员（消息带 file:line）。
- [ ] per-file `采纳`/`已看` 可独立勾选并持久化（端点上线后）。
- [ ] 合并闸四条件全满足才点亮 `[合并回 main]`；`merged`/`resolving`/失败三结果分别正确呈现。

**诚实性断言（不许撒谎 — 核心）**
- [ ] **测试未接入时**「测试通过」条件显示灰中性 `·`「未运行测试」，**绝不**显示绿 `✓`，且不伪造「测试通过」阻塞或放行（G-6）。
- [ ] **base 脏时**「可干净 rebase」显示红 `✕` 并禁用合并按钮（不让用户点了才失败）。
- [ ] **队长卡住时（P-4）** 计划卡冻结为最后已知态并灰化，健康点显 `✕`，**不继续显示假「进行中」**；链到失败卡单一真相源。
- [ ] **hunk 端点缺失时（R-10）** 行不可展开，明示「逐行变更暂不可用，可在终端查看」，**不假装能行级评论**。
- [ ] **队长无计划时（P-0）** 显「尚未给出计划」hairline，**不渲染空计划卡假装有计划**，且**不阻塞**用户继续发消息。
- [ ] **状态无法判定的计划项（P-6）** 显 `·` 中性 + tooltip「状态未知」，**不默认填 `✓`**。
- [ ] 合并/批准/模型切换全部**前置确认弹窗**，无静默执行破坏性动作（原则6）。
- [ ] 合并成功后入流一张可重放的 `kind=system,subtype=merged` 系统卡（端点上线后），刷新/重连后变更历史完整（入流律，原则2）。

**可达性**
- [ ] 所有状态点配文字 label，屏读器能读出状态（不纯靠颜色）。
- [ ] 合并禁用按钮 `aria-describedby` 说明为何不可合并。
- [ ] 行评论草稿 `Esc` 关、`⌘Enter` 发；流内展开后焦点正确转移。

**行话防火墙**
- [ ] 全部用户可见字符串无 mailbox/blackboard/wake/worktree/shim/spell/PTY/handoff；统一用 队长/成员/会话/计划/变更/合并/采纳/已看。

---

**关键依赖前置（实现排期必读）**：本组件区三块的后端数据**几乎全为 missing**（数据地图确证）。建议实现顺序——(1) P1 计划卡：先做前端 `parsePlan` + P-0/P-6 降级，端点是台账 markdown（已有）；(2) P2 交付卡 + 变更评审 + 合并闸：必须等后端 `delivery 系统卡`/`per-file diff`/`hunk 端点`/`review_comments`/`file_accept`/`test_runs` 至少落库到可读，否则只能交付 R-10/G-6/D-2 诚实降级骨架。落地前请与后端确认这些表/端点的排期，**不要为了 demo 用假数据点亮绿态**——那正是本规格要根除的诊断 1/2/6。

相关文件锚点（绝对路径）：
- 复用模板：`/Users/wdx/opc/flockmux-core/web/src/components/workspace/OrchestratorFailureCard.tsx`、`/Users/wdx/opc/flockmux-core/web/src/components/agent/AgentDrawer.tsx:82-92`、`/Users/wdx/opc/flockmux-core/web/src/components/MessagesPanel.tsx:1606`、`/Users/wdx/opc/flockmux-core/web/src/lib/agent.ts:42`
- diff/merge：`/Users/wdx/opc/flockmux-core/web/src/api/http.ts:321-329`、`/Users/wdx/opc/flockmux-core/web/src/api/types.ts:366-382`、`/Users/wdx/opc/flockmux-core/crates/flockmux-server/src/routes/workspaces.rs:816-896`
- 计划数据源：`/Users/wdx/opc/flockmux-core/web/src/routes/workspace/views/Ledger.tsx:67-68`、`/Users/wdx/opc/flockmux-core/web/src/routes/workspace/views/Chat.tsx:681-683`
- 系统卡持久化复用：`/Users/wdx/opc/flockmux-core/crates/flockmux-storage/src/store.rs:1070-1081`
- token 定义：`/Users/wdx/opc/flockmux-core/web/src/styles/global.css`；i18n：`/Users/wdx/opc/flockmux-core/web/src/i18n/locales/zh.json`
