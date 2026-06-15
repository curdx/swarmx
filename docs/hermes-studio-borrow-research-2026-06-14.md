# hermes-studio → flockmux 借鉴决策报告（全 18 域）

> 研究对象：hermes-studio（Vue3/Pinia/Naive UI + Koa/Socket.IO/SQLite + Electron，约 144K 行，核心 agent 推理在外部 Python `hermes-agent` 进程里，本仓库是控制平面+壳）。
> 借鉴目标：flockmux（Rust 多 crate 后端 + React/Tauri 前端，PTY 驱动 claude/codex CLI 的多 agent 编排器）。
> 方法：真读双方源码 + grep/读码核实 flockmux 现状（absent/partial/present），只借「概念/UX/架构/算法/协议」，不抄 TS 代码。

---

## 0. 先回答你点名的问题：hermes 有没有工作流？

**没有运行时工作流引擎。** hermes 仓库里「workflow」只对应两层，都不是 n8n/Temporal/Airflow 式的可视化/可编程运行时编排：

1. **CI/CD 发版工作流**（`.github/workflows/*.yml`）：`ci-workflows` 域明文写 *"hermes has no runtime workflow engine; workflow equals CI/CD only"*。亮点是 **幂等可重跑的分平台发版**（资产已存在则跳过、单平台失败可在既有 tag 上补产）+ **双架构 mac 更新清单合并修复**（`merge-mac-latest-yml.mjs` 把 arm64/x64 两份 `latest-mac.yml` union，避免互相覆盖把另一架构用户喂错 dmg）。**这两条值得借鉴**（见 rank 8）。

2. **Agent 开发 harness**（元方法论，约束开发流程不是产品运行时）：`AGENTS.md` + `docs/harness/` + `scripts/harness-check.mjs`（diff-aware 跨文件不变量校验）+ `docs/chat-chain-changes/`（每 PR 一条变更笔记）。

**关键反转**：真正接近「运行时编排」语义的反而是 flockmux——它的 **spell**（多 agent 编排 recipe：`[[agents]]` + `role_ref` + `depends_on` + 共享 workspace）+ orchestrator 角色 + 黑板台账，比 hermes 的 group-chat @mention 链式触发更显式、更像工作流图。**这一层 flockmux 是更强的一方，无可借。** hermes 的多 agent 协作只是 `mention-routing` 的自组织 @ 喊话 + `mentionDepth` 防循环，不是用户可定义的「触发器→步骤→分支」。

---

## 1. 总体判断：差距集中在「如实呈现」，不在「管道机制」

把 18 域读完，一个清晰的模式浮现：

- **凡是与 flockmux「PTY + 订阅计费 + SQLite 真相源 + tail 磁盘 transcript」架构对齐的子域，flockmux 已对等或更强**——流式 resume、断线重连、lag 自愈、kanban 状态派生、终端持久会话、MCP 自注入、计费分层、reasoning effort、agent harness、origin/DNS-rebind 防护、reaper 存活监控。这些**不应照抄 hermes**，硬抄反而引入它为「服务端拥有结构化聊天 / 多用户 / LAN / 自管上下文」设计的复杂度。

- **真正的高价值借鉴，几乎全部落在「把 CLI/进程真正干了什么，如实、完整地呈现给用户」这一层**：工具 diff 内联、cache 命中率、上下文占用条、sidecar 崩溃自愈、凭据脱敏、i18n 回归防线。这与 flockmux 的「诚实性红线」高度同向——flockmux 底层数据**全已采集到位**，只是上层没把它们派生/呈现出来。

- **少量大件**（应用内 OAuth、外发 IM 通知、多 provider、skills 注入）方向正确但需立项，多数有「需先建基座 / 撞打包痛点 / 需核实 CLI 能力」的前置条件。

---

## 2. 逐域速览（全 18 域，含 status 与一句话结论）

| 域 | flockmux 现状 | 一句话结论 |
|---|---|---|
| 聊天内核与流式 | partial | 管道层已对等/更强；差距在**工具 diff 内联**(P1)+上下文占用条(P1) |
| 群聊房间(多agent) | partial | 只缺**@all 群发+边界安全解析器**(P1)+agent 间链式 @(P2)；多人类社交外壳不适用 |
| 终端与文件浏览 | partial | 终端更强不借；缺**任意文件真下载**(P1)+可写文件浏览器(P2) |
| 写入审批门 | absent | 方向相反(故意 skip-permissions)；只借 **root 自适应权限兜底**(P2)+memory 写入门概念(P2) |
| 桌面打包与分发 | partial | include_str! 内置比 hermes 更抗事故；缺**sidecar 崩溃自愈+诚实横幅**(P0) |
| MCP 集成 | partial | 自注入更强不借；缺**外部 MCP 任意 CRUD 管理面**(P1，接上闲置 catalog) |
| 技能系统 Skills | absent | 概念错位；**用量统计 zero-instrumentation**(P2)+**复活 skills-lock 注入**(P2) |
| 用量分析与成本 | partial | 成本子域更强；缺**cache 命中率**(P1)+三段堆叠(P1)+补零(P2) |
| 记忆与上下文引擎 | partial | 架构性不对等(CLI 自管上下文)；只借**结构化交接 prompt 重写台账压缩**(P1) |
| Agent 开发 harness | present | 已对等/更深；只缺 **Rust→TS codegen**(P1)+脆弱链路变更笔记机检(P2) |
| CI/CD 工作流 | partial | 缺**幂等分平台发版+manifest 修复工作流**(P1) |
| 架构/鉴权/健康/UX | partial | 多数更强；只缺 **i18n 静态检查**(P0)+进程资源监控(P2) |
| 8 平台 IM 接入 | absent | 重型 adapter 在外部无从抄；借**外发 IM 通知**(P2)+凭据/行为分离抽象 |
| 语音 TTS/STT | absent | 完全空白；**TTS 朗读**(P2)+可复用 SSRF 防护(P2)，非核心刚需 |
| Coding Agent 运行器 | partial | 路线相反；只借**凭据脱敏管线**(P1)，代理/tee 是付费 transport 的存档 |
| 看板 Kanban | present | 比 hermes 更扎实；只借**事件流替代轮询**(P1)+可点过滤器(P2) |
| 模型管理与 OAuth | absent | 委托 CLI 是有意设计；只借**应用内 OAuth 登录**(P1)，其余条件性 |
| LAN 设备发现配对 | absent | 整体推翻 loopback 威胁模型不做；只摘**自证身份+防重放握手**(P2 地基) |

---

## 3. Quick Wins（S/M，立即可做）

以下 10 条都已用 grep/读码核实过 flockmux 现状，可直接排期：

### P0（直击头号原则/诚实红线，应最优先）

**① 前端消费 backend-sidecar-down 事件 + 诚实横幅（S）**
- 已核实：`web/src-tauri/src/lib.rs:117/129` 已 `emit("backend-sidecar-down", ...)`，但 `web/src` **零 listener**——事件被白白丢弃。
- 做法：app 级 `listen("backend-sidecar-down")` → 持久横幅显示 stderr tail（Rust 侧已带 20 行尾巴）+「重启后端」按钮。`@tauri-apps/api` 的 `listen()` 在 `web/src/lib/updater.ts` 已是熟路。
- 价值：现在 sidecar 崩了前端只表现为各种 fetch 失败/loading 卡住，用户不知道是后端进程没了——直接消费已有事件即可诚实化。

**② i18n key 覆盖 + 英文回退静态检查（S）**
- 已实测：`web/src/i18n/locales/` en=**1174** key / zh=**1173** key。zh 缺 `home.workspaceCount_one`/`_other`（en 已拆复数、zh 没跟 → zh 复数渲染异常），en 缺 `home.workspaceCount`；另有约 **28 条** zh 值与 en 逐字相同且含拉丁字母的疑似回退（`nav.debug`、`settings.plugins.installTitle`、`dag.handoff`、`agent.stat.*` 等看着是真漏翻，少数如 `repoUrl`/`appName` 是品牌名可白名单）。
- 做法：Node 脚本 walk `web/src` 抽 `t('...')` 字面量 key，断言 en/zh 齐全 + 报告疑似回退，挂进 `scripts/harness-check.mjs` 作 CI 红线。react-i18next 的 `t(..., {defaultValue})` 形态要覆盖、对有 defaultValue 的可降级为 warning。
- 价值：flockmux i18n 大债靠反复手工批次修（`f49f610`/`ec6b758`/`6056544`）却**无任何回归防护**，`app-qa.spec.ts` 只查 overflow/a11y 不查 key 齐全。一次投入换永久红线，正治「英文用户大面积看中文」那类事故。

**③ Tauri sidecar 退避重生监管（M）**
- 已核实：`lib.rs` 只在 `CommandEvent::Terminated` 分支 emit 事件，**无 respawn/spawn_supervised/attempts** 任何监管逻辑。
- 做法：移植 hermes `gateway-runner.ts` 算法——Terminated 后延迟 2s 重生、封顶 3 次、连续稳定跑满 30s 清零失败计数、重生前清 pending 计时器防与手动 relaunch 竞态导致 7777 双开、封顶后升级为「后端永久 down」。约 40 行状态机：`Mutex<RespawnState{attempts,last_started,timer_handle}>` + `tauri::async_runtime::spawn` + `tokio::time::sleep`。注意把 `CommandChild` 句柄在重生时替换进 `ServerSidecar(Mutex)`。
- 价值：现状 sidecar 崩溃后 app 永远连不上后端、只能手动退出重开（`en.json:809` 已坦白），违反「打开→立刻能用、零命令」头号原则。「稳定运行重置」区分崩溃循环与偶发抖动。**①③ 配套做最佳。**

### P1（高价值、低成本）

**④ Cache 命中率指标（S）**——`cache_read/(input+cache_read)`，加 1 张 headline 卡 + by_model/by_day 各加一列。已核实数据全到客户端（`api/types.ts:563/578/596` 都带 `cache_read_tokens`）却从未派生（grep `cacheHit/hitRate` 全仓 0 命中）。纯前端零后端。诚实性：分母 0 时显「—」非 0%。

**⑤ 凭据脱敏管线（S）**——移植 hermes `sanitizeCodingAgentTerminalOutput` 的正则（`Bearer xxx`、`sk-ant-/sk-proj-/sk-or-`、`api_key=`），挂在 PTY 输出 → 落库/transcript/前端推流/**asciicast 录制** 四个边界。agent 可能 echo token，落进 SQLite 或录制就永久留痕、回放可见。纯函数（regex/once_cell），易移植。

**⑥ 结构化交接式 prompt 重写台账压缩（S）**——把 `rest.rs:2757` 的一行 `COMPACT_META_PROMPT` 升级成 hermes 结构化模板（`## 未完成任务 / 关键决策 / 产出物路径 / 阻塞与错误`）+ 头部加「这是只读背景、别把已完成任务再执行一遍」守则。orchestrator 断点续跑全靠读 `task.ledger.md`+`progress.ledger.md`，当前自由文本摘要易丢「哪些已做/没做」边界甚至重做。纯 prompt + 中文本地化。建议配真实 live 验证。

**⑦ 任意文件真下载端点 + blob 客户端（S）**——新增 `GET /api/files/download`，复用 `files.rs` 的 canon+is_sensitive+jail 校验，按扩展名查 MIME + `Content-Disposition: attachment`（含 RFC5987 `filename*`）流式下发；前端复用已有 `lib/download.ts` 的 fetch→blob→a.download（flockmux 已为录像下载写过）。当前非图片文件（zip/csv/pdf/日志）在 UI 看得到名字却拿不下来，唯一二进制端点 `/api/file` 限定仅图片。对 Tauri 跨域 webview 尤其必要。

**⑧ 任务板接入 /ws/swarm 事件流替代 4s 轮询（S）**——`/tasks` 自述「/ws/swarm 暂不带 task 事件故用轮询」是明确 scope 债。复用已有 `/ws/swarm`（已推 AgentState/AgentActivity/BlackboardChanged，正是状态派生上游）做 100ms 去抖 refetch + 退避重连，保留 30s 轮询兜底。无 Rust 改动。注意与「自己刚 optimistic 写完别被事件回刷盖掉」协调（hermes 的 seq/generation 守卫已示范）。

**⑨ 边界安全 @mention 解析器 + @all 群发（M）**——移植 `mention-routing.ts`（前后边界字符校验 + `escapeMentionName` + `@all` 保留字）替换 `MessagesPanel.tsx:651` 朴素正则，`@all` 升级成真群发。hermes 有现成 vitest 用例可作 Rust `#[test]` 黄金集。先做前端把 `@all` 展开成对每个活成员各发一条（后端零改）；注意 flockmux role 可重名，命中后按 `agent_id` 去重并排除发送者。

**⑩ 幂等分平台发版 + manifest 修复工作流（M/S）**——纯 YAML + gh CLI。①资产已存在则跳过、单平台失败可在既有 tag 补产（避 macOS notarize 全量重跑）；②独立 repair 工作流从已发布资产重建 `latest.json` 不重新签名（`gen-updater-manifest.mjs` 接受 assets-dir）。直接修复 v0.1.1 dropped-platform 那类事故。

---

## 4. Big Bets（L/XL，需立项）

**rank 6 工具结果内联 unified diff 渲染（L，P1）**——本研究 **flockmux 最实在的体验差距**。当前 `transcript.rs:723-803` 只抽 48 字符 label，`ThoughtTraceStep` 不携带 input/output 更没 diff，用户在聊天/轨迹里看不到 Edit 改了什么，必须切终端/files 页。做法：`transcript.rs` 为 Edit/Write/MultiEdit 捕获 old_string/new_string（claude transcript 已含）或 codex patch，用 `similar` crate 生成 unified diff，扩 `ThoughtTraceStep` 一个可选 payload；前端引轻量 diff 渲染（react-diff-view）。难点：跨 5 处（解析→SQLite thought_trace→协议→前端类型→渲染）+ payload 体积要设上限（hermes 有 tool-output-storage-limit/tool-payload-boundaries 两 PR 专门处理）+ 与虚拟化 MessagesPanel 的行高/折叠测量集成。

**rank 15 应用内 OAuth 登录 device flow/PKCE（L，P1）**——消灭「auth 失败→去终端跑 /login」违反「零命令」头号原则的痛点（现在 `OrchestratorFailureCard` 只能引导用户开终端）。借鉴 `copilot-device-flow.ts`（RFC 8628 状态机，约 150 行直译 reqwest 轮询）+ `xai-auth.ts`（本地 127.0.0.1 回调，复用已有 axum）。**最大不确定性**：claude OAuth client_id/endpoint 需自己确认可用性（hermes 注释说是「暂时复用」上游，直接抄可能违 ToS）+ token 写入位置（claude 走 macOS Keychain 格式未公开）。**建议先做 codex 的 device flow PoC（文件式 auth.json 可控）。** 配套需 JWT-exp 探针主动续期（codex-auth.ts:201）。

**rank 16 外发 IM 通知（实质 M+基座，P2）**——长跑多 agent 编排任务动辄几十分钟，目前必须盯屏才知道「orchestrator 在等审批/swarm 死锁」。给现有通知系统（`notif.ts` 事件源现成）加外发出口：Telegram `sendMessage` / Slack/Discord incoming webhook，单个 `reqwest::Client` POST，**不需任何平台 SDK**（重型 adapter 本在外部 Python gateway）。难点不在协议在基座：flockmux 现无「用户级 secret 存储」抽象，要新建（建议借 hermes `.env`+0600 思路或 OS keyring）+ 事件→渠道路由去重限流（借 hermes require_mention/allow-list 的「行为」那半）。

**rank 17 复活 skills-lock 注入 worker 能力库（L，P2）**——flockmux 已有 `skills-lock.json`（锁 GitHub 上 SKILL.md）但**全仓零代码读它**（孤儿配置）。借鉴 hermes `skill-injector` 的 sha256 三态同步（装/更新/skip 不覆盖用户改动）：spawn 前按 lock 把 skill 注入 agent 的 `.claude/skills/`（`pre_spawn` 已在往 `.claude` 写 MCP/hook，同构）。现在 agent 只能用用户机上碰巧存在的 skill、能力看天吃饭。难点：lock 源是 GitHub repo，**编译期 include_str 嵌不进**，须发版预拉进 `bundle.resources`（撞 CLAUDE.md 打包痛点）。

**rank 18 外部 MCP 任意 CRUD 管理面（L，P1）**——flockmux 当前只能开关 2 个硬编码 server，`mcpCatalog.ts` 里 13 条（github/playwright/postgres/sentry/linear/figma）**全接不上**（McpPanel 只用了 2 条，catalog 躺着没接），用户想加 Playwright/Postgres MCP 只能命令行手敲。借鉴 hermes `McpManagerView` 的任意 CRUD + 活体连接状态/工具计数。难点：Rust 侧**拿不到** hermes agent bridge 那样的活体状态——需用 ctx7 核实 `claude mcp list` 输出格式是否稳定/可 `--json`，否则降级为「只读配置不显活体」（仍比现状强）。**务必保留 flockmux 已有的 MCP 安全硬化**（防 argv 注入 / env 不进 argv / 打码 / 卸载 allowlist），这点 flockmux 比 hermes 更严谨。

---

## 5. 明确不必借鉴（flockmux 已对等或更好）

**1. 流式机制本身**——`pty_stream.rs` 字节级 seq 游标 resume + Gap 检测 + SQLite 真相源 + 重连重拉 REST，比 hermes 内存事件回放/cursor 双游标更贴 PTY 架构、更稳健。hermes 的上下文 LLM 压缩 / tiktoken 精确计量 / bridge 子进程 reattach / goal LLM 判官续跑，要么不适用（flockmux 不拥有上下文、CLI 自管 session），要么与 orchestrator 多 agent 范式冲突。

**2. 成本/计费子域**——flockmux 的可编辑分层定价（default + `pricing.json` + LiteLLM 1000+ 模型兜底）、≥ 诚实标注、per-agent 归因+可跳转、真实采集而非估算，**全面强于** hermes（其控制平面根本不算成本，cost 由外部 Python agent 写库）。

**3. reasoning effort + per-agent 模型配置**——flockmux 是 per-direction 持久化进 DB（migration 0015）+ 抽象档位经 CLI manifest `effort_levels` 映射 + ModelPicker UI，远胜 hermes 的 client-only localStorage；per-CLI tier→model + per-role default_model_tier + per-spawn override 比 hermes 按 profile 取默认那层抽象度更高。

**4. 终端**——`terminal_ws.rs` 进程级持久会话注册表 + scrollback reattach 续传 + idle reaper + env allowlist，全面强于 hermes 断开即死的终端。只有 14 套 xterm 主题选择器是零风险小糖（P3）。

**5. MCP 自注入**——`pre_spawn` 比 hermes 更强：per-agent 隔离（`--mcp-config` 每 agent 独立文件 + `--strict-mcp-config` 绕开共享 `~/.claude.json` 竞争）+ local-scope 避信任弹窗 + 原子写 + lock 互斥 + mcp-ready 活体握手（hermes 没有）。

**6. 鉴权安全 / 健康监控 / 分层 / kanban 工程质量 / LAN / harness**——flockmux 是 loopback 单用户 Tauri 应用，有意无 token auth，`require_local_origin`（含 DNS-rebind/Host/IPv6 字面量防护）比 hermes 更强；`reaper.rs`（确定性 waitpid 清扫根治界面撒谎）+ `wake.rs`（事件驱动）比 hermes 轮询式 ops-monitor 更先进。hermes 的 JWT/多用户/per-profile/三计数器 login-limiter 是为它 LAN/多用户/远程配对服务，移植到 flockmux 是过度工程。kanban 的 `effective_status` 纯函数派生+单测+optimistic+回滚比 hermes 更扎实，其多板/RBAC/capability-negotiation 对单操作员桌面应用价值低。LAN 设备发现整体推翻 loopback 威胁模型、代价 XL，不做（只摘自证身份+防重放握手作未来 auth 地基，rank 22）。agent harness 已对等或更深（7 条规则全针对 Rust/Tauri 真实静默坑 + pre-commit 主闸 + CI 隔离 smoke）。

---

## 6. 落地建议

**第一波（一个迭代内清掉，全 S/M）**：① backend-sidecar-down 横幅 + ③ sidecar 自愈（配套）→ ② i18n 静态检查 → ⑤ 凭据脱敏 → ④ cache 命中率 + ⑦ 文件真下载 + ⑥ 台账 prompt + ⑧ 任务板事件流。这一波全是「已有数据/已有事件/纯前端派生/纯 prompt」，风险极低，且每一条都正中诚实性红线或头号原则。

**第二波（选 1-2 个 big bet 立项）**：优先 rank 6（工具 diff 内联，最实在体验差距）和 rank 18（外部 MCP 管理面，接上闲置 catalog 是直接能力缺口）。rank 15（应用内 OAuth）建议先 codex PoC 验证可行性再投入。

**需用 ctx7/联网核实的前置**（按你的全局规则）：rank 15 claude/codex OAuth client_id/endpoint/scope + token 写入位置；rank 18 `claude mcp list --json` 输出格式；rank 20 claude `--permission-mode` 当前取值；rank 14 ts-rs vs typeshare 对 serde flatten/enum tag 的支持。这些都不要凭训练数据下结论。
