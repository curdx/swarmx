I have everything. Now I'll write the complete, implementable spec. This is the deliverable — I return it as my final message verbatim.

---

# Composer 组件区详细规格（打断/排队/@/模型/优化/附件/键盘）

实现锚点主体：`/Users/wdx/opc/flockmux-core/web/src/components/MessagesPanel.tsx`（Composer 渲染块 `:1437-1565`，逻辑 `:258-1023`）。对齐文档：`chat-redesign.md §4.4`（`:152-170`）、原则 6「可打断可操控」（`:41`）。

---

## 1. 目的与边界

### 解决哪条诊断 / 原则

| 编号 | 内容 | 本组件的兑现方式 |
|---|---|---|
| 原则 6 | course-correction 永不被静默排队 | `Enter=排队`（可见 pending chip）/ `⌘Enter=打断`（确认后路由队长）两态显式分离；每个在跑成员独立 `■停` |
| 诊断遗留 | 模型切换是静默重启，用户不知发生了什么 | 切换前确认弹窗 + 切换后入流「模型已切换」系统卡 |
| 诊断遗留 | 上传失败用户仍以为带了图 | 失败缩略图翻红框「未上传 · 重试」+ 禁发 |
| 原则 7 | 稳定身份 | 成员 chip / `@`autocomplete / `■停` 全用 `roleColorHex` + 角色派生名，禁裸 `worker_7` |
| §4.6 | 无活队长不静默 bootstrap | 切到无活队长会话时 placeholder 显式「发送将启动队长」 |

### 不做什么（边界）

- **不做后端排队队列**。`Enter=排队` 的 pending chip 是 P0 纯前端可见态（基于现有 `pendingResponders` 推断 `MessagesPanel.tsx:672-706`）；「队长轮次结束才送达」的真排队门控属 P2（thread-as-workspace 已被否决为骨架，见 `chat-redesign.md:210`），**本规格只规定 P0 行为：Enter 立即 `sendMessage` 但 UI 呈现为「已排队 · 等队长接手」**，不引入后端 queue 表。
- **不做 diff/合并闸 / 审批卡**——属 §4.3，不在 Composer。
- **不重写 interrupt/resume**——`interrupt`/`resume`/`interrupt_all` 端点已存在（`rest.rs:1806/1824/1861`，client `http.ts:229/233/239`），只接线。
- **不动 ModelPicker 内部菜单结构**——只在 `onSet` 外面包确认弹窗。
- **优化 ✦ 不新建后端**——`/api/prompt/optimize` 已有（`http.ts:351`），就地改写+undo 逻辑已存在（`optimize/undoOptimize` `:827-859`），本规格只补缺口（按钮已在 `:1505-1519`）。

---

## 2. 完整状态枚举

Composer 是多个正交子状态的叠加。下面分轴枚举，每轴标注数据来源。

### A. 收件人轴 `recipient`（决定 placeholder + chip 颜色）

| 状态 | 触发 | 视觉 |
|---|---|---|
| `to-captain`（默认） | 无 `@`、有 `defaultRecipient`（`:719`） | placeholder=「发消息给队长，或 @成员…」，蓝队长底 |
| `to-member`（@定向） | body 含 `@<role>`，`explicitRecipient≠null`（`:731-740`） | 输入框左上挂成员色「→ 给 测试成员」标签 |
| `mention-typing` | body 末尾 `@<token>`（`mentionQuery≠null` `:744`） | autocomplete 浮层 `:1456-1475` |
| `no-captain-revive` | `defaultRecipient=null` 但 `onSend` 存在（`:771,1017`） | placeholder=「发送将启动队长…」（**改文案，见 §4**），灰底 |
| `dead-end` | `defaultRecipient=null` 且无 `onSend`（`canCompose=false` `:1008`） | textarea `disabled`，placeholder=「该会话还没有 AI 成员…」 |

### B. 提交模式轴 `submitMode`（键盘驱动）

| 状态 | 触发 | 行为 |
|---|---|---|
| `idle` | 默认 | 等待输入 |
| `queueing` | Enter（桌面，`onComposerKey` `:994`） | 发送，UI 标「已排队」灰 chip |
| `interrupting` | ⌘Enter / Ctrl+Enter | **先弹确认**，确认后 `interruptAgent(队长)` → 再 `sendMessage` |
| `interrupt-confirm` | 上一步弹窗中 | 内联确认条（非模态），`[确认打断并发送][取消]` |
| `sending` | `send()` in-flight（`sending=true` `:790`） | 发送按钮转 spinner，禁重复提交 |
| `send-error` | catch（`error≠null` `:817`） | 输入框下方红字错误，**草稿不清空** |

### C. 优化轴 `optimize`（已部分实现，补全枚举）

| 状态 | 触发 | 视觉 |
|---|---|---|
| `optimize-idle` | 默认 | ✦ 灰，body 为空时 disabled（`:1509`） |
| `optimizing` | `optimize()` in-flight（`:830`） | ✦→spinner `:1514` |
| `optimized-undoable` | 改写成功（`preOptimize≠null` `:836`） | 提示行左侧「已优化 · 点此撤销」chip `:1545` |
| `optimize-no-change` | server 返回 `changed=false`（`:841`） | 2.6s 瞬态提示「已经够清晰了，没改动」`:1555` |
| `optimize-error` | catch | 同 `send-error` 走 `error` |

### D. 附件轴 `attach`（**新增失败态，治诊断遗留**）

| 状态 | 触发 | 视觉 |
|---|---|---|
| `attach-none` | 无图 | 无缩略图区 |
| `attach-uploading` | `handleImageFiles` in-flight（`uploadingImage` `:874`） | 虚线框 + spinner `:1447` |
| `attach-ok` | 上传成功，path 入 body（`:879`） | 实缩略图 + ✕（`ComposerThumb` `:1440`） |
| **`attach-failed`（新建）** | upload throw（server 失败/500，`http.ts:370`） | **红框缩略图 + 「未上传 · 重试」+ 禁发** |
| `attach-broken-preview` | 图片可读但渲染失败（已有 `imageBroken`） | 「图片不可用」占位（区别于上传失败） |

### E. 草稿轴 `draft`

| 状态 | 行为 |
|---|---|
| `draft-restored` | 切会话/收件人时按 `draftKey` 还原（`:267,282`） |
| `draft-saved` | 切走 / beforeunload 时持久化（`:283-305`） |
| `draft-cleared` | send 成功后 `removeItem` + 清空（`:776,809`） |

### F. 成员停止轴 `member-stop`（新建）

| 状态 | 触发 | 视觉 |
|---|---|---|
| `stop-none` | 无在跑成员（按 `AgentLiveState.state ∈ {spawning,thinking}`，types.ts:429） | `[打断▾]` 隐藏或灰 |
| `stop-available` | ≥1 成员 `state∈{thinking,spawning,waiting_dep}` 且 `shim_exit==null` | `[打断▾]` 实色，下拉列各成员 `■停` |
| `stop-confirm` | 点某成员 `■停` | 行内确认「打断 测试成员？会中止当前轮 [确认][取消]」 |
| `stopping` | `interruptAgent(id)` in-flight | 该行 spinner |
| `stop-all` | 下拉底部「全部打断」 | `interruptAllInWorkspace`（`http.ts:239`），亦先确认 |

### 边缘 / 空 / 降级

- **空 body**：发送/优化按钮 `disabled`（`sendDisabled` `:1009`）。
- **全部成员已死**：`[打断▾]` 消失（`state∈{exited,error}` 或 `killed_at≠null` 过滤）；不挂幽灵停止键。
- **⌘Enter 但无在跑队长**：降级为普通发送（无可打断对象时不弹确认，避免「打断了个寂寞」）。
- **移动端**：⌘Enter / 打断确认走显式按钮，不绑 Enter（见 §8 键盘）。
- **离线/WS 断**：`[打断▾]` 灰、tooltip「连接中断，暂不可打断」；发送仍走 HTTP（不依赖 WS）。

---

## 3. 逐状态 ASCII 线框

约定：单卡宽 = 对话主轴宽（`max-w≈720`）；外层 `px-3 py-2`；输入框圆角 `rounded-2xl`（=`--radius` 16px 量级）。

### 3.1 默认态（to-captain · idle）

```
┌──────────────────────────────────────────────────────────────┐  外层卡: bg-surface-primary
│  opus·中▾                              [打断▾]                  │  ← 工具行 高36px(min-h-8)
│ ╭──────────────────────────────────────────────────────────╮ │
│ │ 发消息给队长，或 @成员…                                    │ │  ← Textarea rounded-2xl
│ │                                          ✦   ↑           │ │     ✦优化 size-8 圆 / ↑发送 size-8 圆
│ ╰──────────────────────────────────────────────────────────╯ │     右内边距 pr-[7.25rem]
│                              Return 发送 · Shift+Return 换行    │  ← 提示行 10px text-foreground-tertiary
└──────────────────────────────────────────────────────────────┘
```

### 3.2 排队态（queueing → pending chip 浮在 Composer 上方）

```
┌──────────────────────────────────────────────────────────────┐
│ ╭ 灰 chip ─────────────────────────╮                          │  ← pending chip
│ │ ⏳ 已排队 · 队长接手后送达          │                          │     bg-surface-tertiary
│ ╰──────────────────────────────────╯                          │     高24px, 10px 字
│  opus·中▾                              [打断▾]                  │
│ ╭──────────────────────────────────────────────────────────╮ │
│ │ |                                                         │ │  ← 已清空, 焦点回输入框
│ │                                          ✦   ↑           │ │
│ ╰──────────────────────────────────────────────────────────╯ │
│                              Return 发送 · Shift+Return 换行    │
└──────────────────────────────────────────────────────────────┘
```
> chip 来源：复用 `pendingResponders`（`:672-706`）。文案随收件人变（队长/成员名）。**绑 `AgentState`：成员死（`state∈{error,exited}`）→ chip 即移除**（治诊断 2）。

### 3.3 打断确认态（interrupting / interrupt-confirm）

按 ⌘Enter，**不发送**，先弹内联确认（非模态、占输入区上方一行）：

```
┌──────────────────────────────────────────────────────────────┐
│ ╭ 琥珀 confirm 条 ─────────────────────────────────────────╮  │  ← bg-status-warning-soft
│ │ ⚡ 打断队长当前回复，并把这条作为新指令？                  │  │     左 4px status-warning 条
│ │ 会中止在跑成员的重规划。      [取消]  [打断并发送]         │  │
│ ╰──────────────────────────────────────────────────────────╯  │
│ ╭──────────────────────────────────────────────────────────╮ │
│ │ 改用 zod 重写这段校验                                      │ │  ← 草稿保留, 等确认
│ │                                          ✦   ↑           │ │
│ ╰──────────────────────────────────────────────────────────╯ │
│                              Return 发送 · Shift+Return 换行    │
└──────────────────────────────────────────────────────────────┘
```

### 3.4 @定向态（to-member）+ autocomplete

```
┌──────────────────────────────────────────────────────────────┐
│  opus·中▾                              [打断▾]                  │
│ ╭ 绿 chip(成员色) ──────╮                                       │  ← roleColorHex(test)=green
│ │ → 给 测试成员          │                                       │
│ ╰───────────────────────╯                                       │
│ ╭──────────────────────────────────────────────────────────╮ │
│ │ @test 这个用例覆盖边界了吗                                 │ │
│ ╰──────────────────────────────────────────────────────────╯ │
│ ╭ autocomplete 浮层 (输入 @te 时) ─────────────────────╮       │  ← rounded-lg border shadow-lg
│ │ ● @test          a1b2c3d4                            │       │     hover bg-surface-tertiary
│ │ ● @tester2       e5f6...                             │       │     成员色点 + 角色名 + id前8
│ ├─────────────────────────────────────────────────────┤       │
│ │ 📄 选文件作为上下文…  refund.test.ts                 │       │  ← @也能选文件(行级上下文)
│ ╰─────────────────────────────────────────────────────╯       │
│                              Return 发送 · Shift+Return 换行    │
└──────────────────────────────────────────────────────────────┘
```

### 3.5 附件态（ok / uploading / **failed**）

```
┌──────────────────────────────────────────────────────────────┐
│ ┌─────┐ ┌─────┐ ┌╌╌╌╌╌┐ ┌━━━━━┓                               │  缩略图区 64×64, gap-2
│ │login│ │err  │ │ ◌   │ │ ✕   ┃ 未上传               │       │
│ │.png✕│ │red  │ │spin │ │[重试]┃                      │       │
│ └─────┘ └─────┘ └╌╌╌╌╌┘ ┗━━━━━┛                               │
│  ↑ok     ↑failed红框      ↑uploading  ↑failed: 红框+重试       │
│  opus·中▾                              [打断▾]                  │
│ ╭──────────────────────────────────────────────────────────╮ │
│ │ 看这个报错截图                            ✦   ↑(灰禁用)    │ │  ← 有failed附件 → 发送禁用
│ ╰──────────────────────────────────────────────────────────╯ │
│ ⚠ 1 张图未上传，移除或重试后才能发送        Return 发送…        │  ← aria-describedby 指向此句
└──────────────────────────────────────────────────────────────┘
```

### 3.6 成员停止下拉（stop-available）

```
                                      ┌ [打断▾] 展开 ───────────────┐
                                      │ 在跑成员                     │  ← popover w-56
                                      ├─────────────────────────────┤
                                      │ ● 测试成员  写 refund.test ■停│  ← 成员色点+动词+■停
                                      │ ● 后端成员  跑测试         ■停│
                                      ├─────────────────────────────┤
                                      │ ⏹ 全部打断 (2)              │  ← interrupt_all
                                      └─────────────────────────────┘
```

### 3.7 无活队长（no-captain-revive）

```
┌──────────────────────────────────────────────────────────────┐
│  opus·中▾                                                      │  ← 无在跑成员, [打断▾]隐藏
│ ╭──────────────────────────────────────────────────────────╮ │
│ │ 发送将启动队长，AI 随后上线开工…                          │ │  ← placeholder 改: 显式告知
│ │                                          ✦   ↑           │ │
│ ╰──────────────────────────────────────────────────────────╯ │
│ ⓘ 这条会先启动队长引擎，可能需要几秒        Return 发送…        │  ← aria-describedby
└──────────────────────────────────────────────────────────────┘
```

### 3.8 移动端（isMobileLike）

```
┌──────────────────────────────────────────────┐
│ opus·中▾                          [打断]       │
│ ╭──────────────────────────────────────────╮ │
│ │ 发消息给队长…                             │ │  ← Enter=换行
│ ╰──────────────────────────────────────────╯ │
│ [✦]  [📎]                         [ 发送 → ]   │  ← 显式发送键, 长按发送=打断
│              回车换行 · 点发送按钮发送          │
└────────────────────────────────────────────────┘
```

---

## 4. 精确中文文案 + i18n key

行话防火墙：禁 mailbox/blackboard/wake/worktree/shim/spell/PTY/handoff；用 队长/成员/会话/计划/变更/推进。命名空间沿用现有 `messages.*` / `model.*`（`zh.json`）。**已存在的复用，标注 [已有]；新增标注 [新]。**

### Placeholder（三态，绑 `aria-describedby`）

| key | 文案 | 状态 |
|---|---|---|
| `messages.composerPlaceholder` [已有] | `发消息给队长，或 @成员…` | to-captain（**改**：原「向 AI 描述你的需求…」→ 点明收件人=队长） |
| `messages.composerPlaceholderRevive` [改] | `发送将启动队长，AI 随后上线开工…` | no-captain-revive（**改**：原「发条消息，AI 会自动上线开工…」→ 显式「启动队长」治静默 bootstrap） |
| `messages.composerPlaceholderEmpty` [已有] | `该会话还没有 AI 成员…` | dead-end |

### aria-describedby 说明句（解释为何不可发 / 会发生什么）

| key | 文案 | 触发 |
|---|---|---|
| `messages.describe.captain` [新] | `按 Return 发送给队长；Shift+Return 换行。` | to-captain |
| `messages.describe.member` [新] | `这条会直接发给 {{role}}；按 Return 发送。` | to-member |
| `messages.describe.revive` [新] | `这条会先启动队长引擎，可能需要几秒。` | no-captain-revive |
| `messages.describe.attachFailed` [新] | `有 {{count}} 张图未上传，移除或重试后才能发送。` | attach-failed → 禁发原因 |
| `messages.describe.empty` [新] | `该会话还没有 AI 成员，无法发送。` | dead-end |

### 排队 / 打断

| key | 文案 |
|---|---|
| `messages.queuedToCaptain` [新] | `已排队 · 队长接手后送达` |
| `messages.queuedToMember` [新] | `已排队 · 发给 {{role}}` |
| `messages.interruptConfirmTitle` [新] | `打断队长当前回复，并把这条作为新指令？` |
| `messages.interruptConfirmBody` [新] | `会中止在跑成员的重规划。` |
| `messages.interruptConfirmYes` [新] | `打断并发送` |
| `messages.interruptCancel` [新] | `取消` |
| `messages.interruptHint` [新] | `{{cmd}}+Return 打断队长` （cmd = ⌘ / Ctrl，按平台） |

### 成员停止

| key | 文案 |
|---|---|
| `messages.stopMenuLabel` [新] | `打断` |
| `messages.stopMenuHeading` [新] | `在跑成员` |
| `messages.stopMember` [新] | `停` |
| `messages.stopMemberConfirm` [新] | `打断 {{role}}？会中止它当前这轮。` |
| `messages.stopAll` [新] | `全部打断（{{count}}）` |
| `messages.stopUnavailableOffline` [新] | `连接中断，暂不可打断` |
| `messages.stopNoneRunning` [新] | `当前没有在跑的成员` |

### 模型切换（确认弹窗 + 入流系统卡）

| key | 文案 |
|---|---|
| `messages.modelConfirmTitle` [新] | `换模型会重启队长` |
| `messages.modelConfirmBody` [新] | `当前在跑的回复会被打断，从 {{from}} 切到 {{to}}。` |
| `messages.modelConfirmYes` [新] | `确认切换` |
| `messages.modelConfirmCancel` [新] | `取消` |
| `messages.modelChangedCard` [新] | `模型已切换：{{from}} → {{to}}` （系统卡正文，`meta.subtype='model_changed'`） |

### 优化（复用已有）

| key | 文案 | 状态 |
|---|---|---|
| `messages.optimize` [已有] | `优化输入` | aria-label |
| `messages.optimizeTooltip` [已有] | `用 AI 把你的输入改写得更清晰（保留技术细节，不臆造需求）` | |
| `messages.optimizeUndo` [已有] | `已优化 · 点此撤销` | |
| `messages.optimizeNoChange` [已有] | `已经够清晰了，没改动` | |

### 附件

| key | 文案 |
|---|---|
| `messages.attachImage` [已有] | `添加图片` |
| `messages.removeImage` [已有] | `移除` |
| `messages.attachFailed` [新] | `未上传` |
| `messages.attachRetry` [新] | `重试` |
| `messages.attachFailedAria` [新] | `{{name}} 上传失败，点击重试` |

### 发送提示（复用）

| key | 文案 |
|---|---|
| `messages.sendHintDesktop` [已有] | `{{enter}} 发送 · Shift+{{enter}} 换行` |
| `messages.sendHintMobile` [已有] | `回车换行 · 点发送按钮发送` |
| `messages.send` / `messages.sending` [已有] | `发送` / `发送中…` |

> 模型 pill 文案直接复用 `model.*`（`opus`/`sonnet`/`haiku` + `effort.*`）。**不引入新模型行话**。

---

## 5. 尺寸 / 间距 / 色彩 token

全部用 `global.css` 已定义的 token（`:21-133`），明暗双主题自动适配。

### 布局尺寸

| 元素 | 值 | 依据 |
|---|---|---|
| 外层卡 padding | `px-3 py-2`（12/8px） | 复用 `:1033` |
| Textarea 圆角 | `rounded-2xl`（16px，介于 `--radius-xl:12` 与 full 之间，沿用现状 `:1501`） | 已有 |
| Textarea 字号/行高 | `text-[13px] leading-snug`（`:1501`） | 已有 |
| Textarea 自增高 | min 1 行，max `140px`（`autoGrow` `:985`） | 已有 |
| 右内边距留按钮位 | `pr-[7.25rem] pb-12`（`:1501`） | 已有 |
| ✦/↑ 按钮 | `size-8`（32px）圆 `rounded-full`（`:1512,1530`） | 已有 |
| 缩略图 | `64×64`（`h-16 w-16` `:1448`） | 已有 |
| 工具行高 | `min-h-8`（32px，同 ModelPicker `:64`） | 已有 |
| pending chip | 高 24px，`px-2 py-0.5`，`text-[10px]` | 同优化 chip `:1549` |
| autocomplete 浮层 | `rounded-lg border shadow-lg`，行 `px-3 py-1.5 text-[12px]` | 复用 `:1457-1466` |
| 提示行字号 | `text-[10px] font-caption`（`:1561`） | 已有 |
| 确认条 | 左色条 `border-l-4`，`px-3 py-2`，`text-[12px]` | 同系统卡语法 §4.1 |

### 色彩 token

| 用途 | token |
|---|---|
| 输入框文字 | `--color-foreground-primary` |
| placeholder / 提示 / id | `--color-foreground-tertiary` |
| 卡背景 | `--color-surface-primary` |
| pending chip 背景 | `--color-surface-tertiary` |
| 发送按钮 enabled | `--color-accent-primary`（白图标 `--color-foreground-on-accent`），`shadow-sm`（`:1533`） |
| 发送按钮 disabled | `!bg-surface-tertiary !text-foreground-tertiary`（`:1532`，对比明确，治「禁用看着像可点」） |
| ✦ hover | `hover:text-accent-primary`（`:1512`） |
| 优化 undo chip | `bg-accent-primary-soft text-accent-primary-deep`（`:1549`） |
| 打断确认条 | 背景 `--color-status-warning-soft`(#FEF3C7)，左条 `--color-status-warning`(#B45309)，⚡图标同 |
| **附件失败红框** | 边框 `--color-status-danger`(#B91C1C)，背景 `--color-status-danger-soft`(#FEE2E2)，「未上传」字 `--color-status-danger` |
| `@`成员 chip / `■停` 成员点 | `roleColorHex(role)`（`agent.ts:47`，映射 `--color-agent-*`：test=green#16A34A / backend=violet / frontend=cyan…） |
| autocomplete hover | `--color-surface-tertiary`（`:1466`） |
| 模型确认弹窗 | 复用 `Dialog`/`AlertDialog` 现有皮肤（`surface-elevated` + `border-subtle`） |

### 圆角

`--radius-sm:4 / -md:6 / -lg:8 / -xl:12 / -full:9999`（`:129-133`）。chip/按钮用 full，浮层用 lg，卡用 xl。

---

## 6. 数据绑定表

每个动态元素 ← 信号 / 端点；persisted（DB）/ live-only（WS/内存）/ missing（需后端新增）。引用 file:line。

| UI 元素 | 数据来源 | 持久化 | 锚点 | 缺口 |
|---|---|---|---|---|
| 默认收件人=队长 | `defaultRecipient`（orchestrator→scout→first alive） | derived | `MessagesPanel.tsx:719-726` | — |
| `@成员` 定向 | `explicitRecipient`（body 正则匹配 `activeMembers`） | live-only（内存） | `:731-740` | — |
| `@`autocomplete 成员 | `mentionMatches`（`activeMembers` 过滤） | live-only | `:744-757` | — |
| `@`选文件（行级上下文） | **missing** | missing | — | **依赖后端新增**：文件列表来源（建议 `GET /api/workspaces/:id/files` 或复用 worktree 文件树端点），当前 Composer 无文件枚举源；P1 |
| 成员色 / 角色名 | `roleColorHex` / `resolveRole` | persisted（agents 表 role） | `agent.ts:47,68` | — |
| 发送（排队） | `api.sendMessage` + `api.wakeAgent` | persisted（messages 表） | `:792-808`，`http.ts:221` | — |
| pending chip（已排队） | `pendingResponders`（last_received>last_sent，60s 窗） | live-only（前端推断） | `:672-706` | **持久化缺口**：pending 占位刷新即丢；P1 需落 `messages kind='pending'` 才能重连恢复（数据图已标 missing） |
| **pending chip 死即移除** | `AgentLiveState.state ∈ {error,exited}` | live-only（WS） | `types.ts:429-441`，状态由 `/ws/swarm` 累积 | **当前 `pendingResponders` 只看 `shim_exit/killed_at`（`:675`），未绑 `AgentState`**；P0 需在过滤里加 `liveState.state∈{error,exited}` 排除（治诊断 2 根因） |
| ⌘Enter 打断 | `api.interruptAgent(队长id)` | persisted（paused flag） | `http.ts:229`，`rest.rs:1806` | — |
| 每成员 `■停` | `api.interruptAgent(memberId)`；列表来自在跑成员 | persisted | `http.ts:229` | 在跑判定需 `AgentLiveState.state∈{thinking,spawning,waiting_dep}`（live-only WS）；无 DB 兜底，重连前 `[打断▾]` 可能空一瞬，可接受 |
| 全部打断 | `api.interruptAllInWorkspace(wsId)` | persisted | `http.ts:239`，`rest.rs:1861` | 需 `workspaceId` prop（`:97` 已有 `activeThreadId`，需确认 `workspaceId` 传入；否则**依赖父组件透传 workspaceId**） |
| 模型 pill | `modelTier`/`reasoningEffort` props + `onSetModel` | persisted（thread 行） | `MessagesPanel.tsx:1060-1066`，`ModelPicker.tsx` | — |
| 模型切换确认弹窗 | 纯前端（包在 `onSet` 外） | — | 新建 | — |
| 「模型已切换」系统卡 | `messages kind='system' meta={subtype:'model_changed',old,new}` | **missing→persisted** | `store.rs:1070`（kind+meta 列已有） | **依赖后端新增**：`ModelPicker.onSet`/重启路径需 inject 一条系统消息；schema 已就绪，缺 emit 点（数据图 reusable: messages kind+meta）；P1 |
| 优化 ✦ | `api.optimizePrompt` | live-only（单次，无历史） | `:827-850`，`http.ts:351`，`rest.rs:2392` | undo 仅前端 `preOptimize`（`:310`），无服务端历史，符合设计 |
| 附件上传 | `api.uploadAttachment` | persisted（文件落盘，path 入 body） | `:871-889`，`http.ts:361`，`rest.rs:2838` | — |
| **附件失败态** | upload `throw`（res !ok） | **missing（前端状态机）** | `http.ts:370` throw | **当前无 per-attachment 失败态**：`handleImageFiles` 失败只 set 全局 `error`（`:884`），图根本没进 body 故无红框可显；P0 需新建 per-file 上传状态机（pending/ok/failed），见 §7 |
| 草稿持久化 | `localStorage[draftKey]`，key=`(workspaceSlug,activeThreadId)` | persisted（本地） | `:267-305` | key 当前不含**收件人**；设计要求每(会话,**收件人**)keyed，**需扩 key 含 recipient**（见 §7） |
| placeholder 三态 | `defaultRecipient`/`onSend`/`canCompose` | derived | `:1015-1019,1008` | — |
| 键盘 Enter/⌘Enter | `getClientPlatformInfo().isMobileLike` | derived（UA） | `:992`，`platform.ts:79` | — |

---

## 7. 复用 vs 新建（到文件/函数）

### 直接复用（不改）

| 复用物 | 文件:行 |
|---|---|
| 草稿持久化骨架 | `MessagesPanel.tsx:263-306`（仅扩 key，见下） |
| `@`autocomplete（成员） | `:744-761`（pickMention）+ `:1456-1475`（浮层渲染） |
| 优化 ✦ + undo | `:827-859`（optimize/undoOptimize）+ `:1505-1519,1545-1559`（UI） |
| 附件粘贴/拖拽/路径/缩略图 | `:861-926`，`ComposerThumb`（`:1786`附近），`api.uploadAttachment`（`http.ts:361`） |
| 键盘平台分支 | `:988-998` + `platform.ts:33-94`（`isMobileLike`/`enterKeyLabel`） |
| ModelPicker pill+菜单 | `ModelPicker.tsx` 全文（只在 `onSet` 外包确认） |
| 发送/wake | `:763-821` |
| 成员色/角色名 | `agent.ts:47(roleColorHex)/68(resolveRole)/265(resolveMemberVisual)` |
| interrupt/resume/all 端点 | `http.ts:229/233/239` → `rest.rs:1806/1824/1861` |
| pending 推断 | `:672-706`（只加一处 AgentState 过滤） |

### 改（小改）

| 改什么 | 位置 | 改动 |
|---|---|---|
| placeholder 文案 | `zh.json messages.composerPlaceholder/Revive` | 改成点明「队长」+「启动队长」 |
| draftKey 加收件人维度 | `:267` | `...:${activeThreadId ?? "main"}:${explicitRecipient?.role ?? "captain"}` —— @定向草稿不串味 |
| pending chip 绑 AgentState | `:672-706` | 过滤加：`&& liveState(id)?.state` 不属 `{error,exited}`（需把 `agentLiveStateById` 传入或在父层算好，**依赖 prop**） |
| 键盘加 ⌘Enter 分支 | `:994` | `if (e.key==='Enter' && (e.metaKey||e.ctrlKey)) → openInterruptConfirm()` |
| Textarea aria | `:1497` | 补 `aria-describedby` 指向当前状态说明句 id |
| 发送禁用条件 | `:1009` | `|| hasFailedAttachment`（新态参与） |
| ModelPicker `onSet` | `:1060-1066` 调用处 | 包一层：先弹 `messages.modelConfirm*`，确认后才执行原 `onSetModel`，再触发系统卡 inject |

### 新建（组件/逻辑）

| 新建 | 落点 | 说明 |
|---|---|---|
| `<InterruptConfirmBar>` | MessagesPanel 内联（Composer 上方） | 琥珀确认条，§3.3；⌘Enter 与 ⌘Enter 后 send 的中间态 |
| `<StopMenu>` | Composer 工具行右侧 `[打断▾]` | popover，列在跑成员 + `■停` + 全部打断；每项 `interruptAgent`/`interruptAllInWorkspace`，**用 `roleColorHex`** |
| per-attachment 上传状态机 | 替换 `handleImageFiles`（`:871-889`） | `useState<Map<localId,{file,status:'uploading'|'ok'|'failed',path?}>>`；失败保留条目+红框+重试，**不把失败 path 写进 body**；`hasFailedAttachment` 派生 |
| `<ComposerThumb>` 失败变体 | `ComposerThumb`（`:1786`） | 加 `failed` prop：红框 + 「未上传」+ 「重试」按钮（重试 = 重调 upload） |
| pending chip 渲染 | Composer 上方 | 复用 `pendingResponders`，渲染 §3.2 chip，文案按收件人 |
| `<ModelChangeConfirm>` | ModelPicker 调用处包装 | `AlertDialog`，确认后 `onSetModel` + post-switch inject 系统卡 |
| 模型切换系统卡 emit | **后端** | `messages kind='system' meta={subtype:'model_changed',old_model,new_model}`（schema 已有 `store.rs:1070`，缺 emit 点）；P1 |
| `@`文件 autocomplete 分支 | `:744-757` 扩展 | token 以 `@` 开头但匹配不到成员时，查文件列表（**依赖后端文件枚举端点**）；P1 |

---

## 8. 交互与时序（事件→转换、阈值、防抖、键盘、aria）

### 键盘矩阵

| 平台 | Enter | Shift+Enter | ⌘/Ctrl+Enter |
|---|---|---|---|
| 桌面（`isMobileLike=false`） | 发送（=排队 chip）`send()` | 换行（默认） | **打断**：开 InterruptConfirmBar；条内 Enter=确认、Esc=取消 |
| 移动（`isMobileLike=true`） | 换行（`onComposerKey` 提前 return `:993`） | 换行 | 不绑；打断走 `[打断]` 显式键 |

实现：`onComposerKey`（`:988`）补 `metaKey||ctrlKey` 优先分支，置于现有 `!e.shiftKey` 之前。`enterKeyLabel`（mac=`Return`）用于提示与 `messages.interruptHint` 的 `{{cmd}}`（mac=⌘，其它=Ctrl）。

### 状态转换时序

**Enter（排队）**：
1. `send()` → `sendMessage` + `wakeAgent`（`:792-806`）→ 清草稿、focus 回输入框（`:809-815`）。
2. 立即出现 pending chip（`pendingResponders` 因 `lastReceived/lastSent` 更新而点亮）。
3. chip 存活直到：收到队长回复（`lastReceived` 翻新）/ 超 60s（`PENDING_TIMEOUT_MS` `:698`）/ **目标成员死（AgentState→error/exited，新增过滤）**。死即移除，**不挂 60s 幽灵**（治诊断 2）。

**⌘Enter（打断）**：
1. 拦截，不发送 → 开 `InterruptConfirmBar`（草稿保留）。
2. 无在跑队长时降级为普通 `send()`（无对象不弹）。
3. 「打断并发送」→ `await interruptAgent(队长id)` → 成功后 `send()`；失败 → `error` 红字，草稿保留，不发。
4. 「取消」/Esc → 关条，焦点回输入框，草稿原样。

**成员 `■停`**：点 → 行内确认（`stopMemberConfirm`）→ `interruptAgent(memberId)`（spinner）→ 成功该成员从在跑列移除（由 AgentState 驱动）。

**模型切换**：点菜单项 → 弹 `ModelChangeConfirm`（`from`=当前 pill，`to`=新选）→ 确认 → 原 `onSetModel`（重启队长，`ModelPicker.tsx:12-14`）→ 成功后入流系统卡（P1 后端 emit）。取消 → 无副作用。

**优化 ✦**：点 → spinner（`optimizing`）→ server `changed=true` 则 `setPreOptimize(原文)`+`setBody(优化文)`+显 undo chip；`changed=false` 则 2.6s 瞬态「没改动」（`:842`）。用户编辑即清 undo（`:1491`）。无防抖（按钮单击），但 body 为空时 disabled。

**附件**：粘贴/拖拽触发 `handleImageFiles`（改为 per-file）：每 file 立即占一个 `uploading` 缩略图 → 成功 `ok`+path 入 body → 失败 `failed`+红框（**path 不入 body**）。重试 = 重调单 file upload。任一 `failed` → `sendDisabled`。

### 阈值 / 防抖

| 项 | 值 |
|---|---|
| pending chip 存活上限 | `PENDING_TIMEOUT_MS`（60s，`:698`），但 AgentState 死优先移除 |
| pending 重算节拍 | `setInterval 5000ms`（`:669`）刷新 now 比较 |
| 优化「没改动」提示 | 2600ms 自动消（`:842`） |
| autoGrow 上限 | 140px（`:985`） |
| 上传大小 | 服务端 25MB cap（`rest.rs:2838` 注释，前端只接 throw） |
| 草稿持久化 | 即时（切 key cleanup + beforeunload `:283-305`），无防抖 |

### 可达性（aria）

- Textarea：`aria-label="messages.composerLabel"`（`:1497` 已有）+ **新增** `aria-describedby={describeId}`，`describeId` 随状态绑到对应说明 `<span id>`（§4 describe.*）。**禁发时说明句解释「为什么」**（治「按钮灰了不知为啥」）。
- 发送按钮：`aria-label` 在 `send`/`sending` 间切（`:1524` 已有）；`aria-disabled` 同步 `sendDisabled`。
- InterruptConfirmBar：`role="alertdialog"` + `aria-label=interruptConfirmTitle`，焦点入「打断并发送」，Esc 关。
- StopMenu：`role="menu"`，各项 `role="menuitem"` + `aria-label="停止 {{role}}"`。
- 附件失败缩略图：`aria-label=attachFailedAria`，重试键可聚焦。
- 模型确认：`AlertDialog`（已带 a11y）。
- pending chip：`aria-live="polite"`，让屏幕阅读器播报「已排队」。
- autocomplete：`role="listbox"`/`option`，↑↓ 键导航 + Enter 选（当前用 `onMouseDown`，补键盘）。

---

## 9. 验收标准（checklist · 含诚实性断言）

### 收件人 / @

- [ ] 默认收件人解析顺序 orchestrator→scout→first-alive（`:719`），无活成员且有 `onSend` 时 placeholder 显示 revive 文案。
- [ ] body 含 `@<role>` 时收件人切到该成员，输入框挂成员色「→ 给 X」标签，send 实际发给 `explicitRecipient`（`:767`）。
- [ ] `@`autocomplete 列出成员用**角色派生名 + 成员色**，禁裸 agent_id 作主标识（id 仅作 8 字符副标）。
- [ ] `@`匹配不到成员时降级为文件选择（P1；P0 阶段可不实现但不得报错）。

### 排队 / 打断（原则 6）

- [ ] 桌面 Enter 发送后**立即**出现可见 pending chip，文案随收件人（队长/成员）。
- [ ] **诚实性断言**：pending chip 在目标成员 `AgentState→error/exited` 时**立即消失**，绝不静默挂满 60s（直接验证诊断 2 根因已修：`pendingResponders` 过滤含 AgentState）。
- [ ] ⌘Enter（mac）/Ctrl+Enter 弹确认条，**不静默发送**；确认后先 `interruptAgent(队长)` 再 send；取消则草稿原样。
- [ ] 无在跑队长时 ⌘Enter 降级为普通发送（不弹空确认）。
- [ ] `[打断▾]` 只列**真正在跑**的成员（`state∈{thinking,spawning,waiting_dep}`），死的不出现；每个 `■停` 调 `interruptAgent(该id)`；「全部打断」调 `interruptAllInWorkspace`。
- [ ] **诚实性断言**：成员全死时 `[打断▾]` 消失/灰，不展示可点但实际无效的停止键。

### 模型

- [ ] 点模型菜单项**先弹确认**，文案含 `{{from}}→{{to}}` 与「会重启队长、打断在跑回复」；取消则模型不变、无重启。
- [ ] 确认后执行 `onSetModel`，**入流一张「模型已切换 X→Y」系统卡**（`meta.subtype='model_changed'`），刷新后可重放（P1 后端 emit；P0 阶段至少前端乐观插一条本地系统消息并标注待持久化）。

### 优化

- [ ] ✦ 在 body 非空时可点；优化为**就地改写 + 一键 undo**，不静默覆盖（`preOptimize` 保原文 `:836`）。
- [ ] **诚实性断言**：server 判定「无需改」时显式提示「没改动」，**不伪造一次编辑**（`:840-842`）。

### 附件（治诊断遗留）

- [ ] 上传中显 spinner 缩略图；成功显实缩略图 + path 入 body。
- [ ] **诚实性断言**：上传失败的图**翻红框 + 「未上传 · 重试」**，且**该 path 不写进 body、发送被禁用**——用户绝不会误以为带了图发出去。
- [ ] aria-describedby 在禁发时说明原因（「N 张图未上传…」），重试键可键盘聚焦。

### 键盘 / 草稿 / a11y

- [ ] 桌面 Enter=发送/Shift+Enter=换行；移动端 Enter=换行 + 显式发送键（`isMobileLike` 分支 `:993`）。
- [ ] Composer 下方常驻 `sendHint`（`{{enter}} 发送 · Shift+{{enter}} 换行`），`enterKeyLabel` 随平台（mac=Return）。
- [ ] 草稿按 **(会话, 收件人)** keyed：切会话或切 @定向收件人时草稿不串味、不丢（扩 `draftKey`）；send 成功清草稿。
- [ ] **诚实性断言**：切到无活队长会话时 placeholder 显式「发送将启动队长」，**不静默 bootstrap**（用户知道这条会触发启动）。
- [ ] 三态 placeholder + `aria-describedby` 完整；屏幕阅读器能读到「为何不可发」。

### 数据诚实 / 缺口标注

- [ ] 标注为 missing 的能力（`@`文件、pending 持久化、模型系统卡持久化）在代码注释或 PR 描述中明确「依赖后端新增 X」，P0 不假装已持久化（pending chip 注明刷新即丢，待 P1 落 `messages kind='pending'`）。
- [ ] 所有用户可见字符串走 i18n key（无硬编码），且**无行话泄漏**（grep 确认 Composer 区无 mailbox/blackboard/wake/worktree/shim/spell/PTY/handoff）。

---

### 关键缺口（需后端新增，规格已内联标注）

1. **`@`选文件**：无文件枚举端点供 Composer autocomplete，需新增（P1）。
2. **pending 排队持久化**：现仅前端推断（`:672-706`），刷新即丢，需 `messages kind='pending'`（P1，schema 已就绪 `store.rs:1070`）。
3. **模型已切换系统卡**：`ModelPicker.onSet`/重启路径缺 inject 点，需 emit `kind='system' meta.subtype='model_changed'`（P1）。
4. **pending chip 绑 AgentState**：当前 `pendingResponders` 只看 `shim_exit/killed_at`，需把 `AgentLiveState`（WS `state`）传入 Composer 才能「死即移除」——**这是 P0 必做、治诊断 2 的硬接线点**，依赖父组件透传 `agentLiveStateById`。

实现起点文件（绝对路径）：
- `/Users/wdx/opc/flockmux-core/web/src/components/MessagesPanel.tsx`（Composer 全部逻辑 + 渲染）
- `/Users/wdx/opc/flockmux-core/web/src/components/ModelPicker.tsx`（包确认弹窗）
- `/Users/wdx/opc/flockmux-core/web/src/api/http.ts`（`interruptAgent:229` / `interruptAllInWorkspace:239` / `uploadAttachment:361` / `optimizePrompt:351`）
- `/Users/wdx/opc/flockmux-core/web/src/lib/agent.ts`（`roleColorHex:47` / `resolveRole:68`）
- `/Users/wdx/opc/flockmux-core/web/src/lib/platform.ts`（`getClientPlatformInfo:33`）
- `/Users/wdx/opc/flockmux-core/web/src/i18n/locales/zh.json` / `en.json`（新增 `messages.*` key）
- `/Users/wdx/opc/flockmux-core/web/src/styles/global.css`（token 来源，勿改值）
- 后端 emit 点：`/Users/wdx/opc/flockmux-core/crates/flockmux-storage/src/store.rs:1070`（messages kind+meta）、`/Users/wdx/opc/flockmux-core/crates/flockmux-server/src/routes/rest.rs:1806/1861`（interrupt）
