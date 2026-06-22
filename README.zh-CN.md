# swarmx

<p align="center">
  <strong>一个浏览器仪表盘:在 PTY 下拉起真实的 <code>claude</code> / <code>codex</code> / <code>opencode</code> / <code>reasonix</code> CLI,把它们组成一个 swarm,让它们互相发消息、协作完成一个任务。</strong>
</p>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-MIT-blue.svg" alt="MIT License"></a>
  <img src="https://img.shields.io/badge/Rust-1.83%2B-orange.svg" alt="Rust 1.83+">
  <img src="https://img.shields.io/badge/Node-22%2B-brightgreen.svg" alt="Node 22+">
  <img src="https://img.shields.io/badge/desktop-Tauri-9cf.svg" alt="Tauri">
  <a href="README.md"><img src="https://img.shields.io/badge/Lang-English-blue" alt="English"></a>
</p>

swarmx 在一个浏览器标签页(或 Tauri 桌面应用)里跑**真实的订阅模式 CLI** ——
就是你磁盘上已有的 `claude`、`codex`、`opencode`、`reasonix` 二进制本身。
每个 agent 有自己独立的、PTY 支撑的终端面板(xterm.js,WebGL 加速)。
上面薄薄一层协调机制,给这些 agent 加上了它们单独运行时没有的能力:

1. **共享收件箱。** 任何 agent 都能调 `swarm_send_message` 按 id 给另一个 agent 发消息;
   收件方在自己下一个回合边界、通过 Stop hook 驱动的 wake-check 看到这条消息。
2. **共享黑板。** 一个带 FTS5 全文检索、版本化历史的 markdown KV 存储,任何 agent
   改动它都会通过 `/ws/swarm` 实时推送通知。
3. **唯一的对话入口 —— orchestrator。** 每个 workspace 会启动一个常驻 orchestrator agent。
   你用自然语言跟它说话,它按任务规模自己决定:直接回答、自己动手干、还是用
   `swarm_spawn_worker` 派 worker 出去。团队规模随任务伸缩(Magentic-One 模型),
   而不是预先声明一套固定拓扑。
4. **推送式唤醒。** 当某个 role `depends_on` 的黑板 key 被写入时,服务端既投递一条
   mailbox note,又往订阅者的 PTY 注入 `\x15…\r` —— 这样一个已经空闲停下的 agent,
   能在它的上游一落地就被当场唤醒。无轮询,无死锁。

仪表盘把每个会话都录成 asciicast v2 `.cast` 文件,并用官方的 `asciinema-player`
(WASM 渲染,完整键盘控制)在浏览器里回放。

swarmx **从不读取、也从不持久化你的 OAuth token。** 它只把 `HOME` 透传给子进程,
让每个工具复用你已经存在 `~/.claude/`、`~/.codex/` 里的凭据 —— 这跟 `tmux` 管理
会话凭据的模型完全一样。详见 [安全与凭据](#安全与凭据)。

---

## 目录

- [为什么做这个](#为什么做这个)
- [功能特性](#功能特性)
- [快速开始](#快速开始)
- [核心概念](#核心概念)
- [架构](#架构)
- [配置参考](#配置参考)
- [REST 与 WebSocket API](#rest-与-websocket-api)
- [安全与凭据](#安全与凭据)
- [打包桌面应用](#打包桌面应用)
- [常见问题](#常见问题)
- [贡献](#贡献)
- [致谢](#致谢)
- [License](#license)

---

## 为什么做这个

大多数「agent 编排」项目,要么从头重写一个 LLM 客户端(丢掉了订阅用户真正付费买到的
官方 CLI 订阅鉴权),要么在错误的层去包 CLI(比如 ACP,没法复用订阅会话)。
swarmx 刻意做成「能加协调、又不替换任何东西」的最简单的一层:

- **最底层是 PTY。** 每个 agent 都是未经修改的 `claude` / `codex` / `opencode` /
  `reasonix` 二进制,跑在 `portable-pty` 下。OAuth、限流、套餐限制的行为,跟你在
  自己终端里敲这个命令完全一致。
- **中间一层薄 shim。** `swarmx-shim` 是约 70 行 Rust,`execvp` 真 CLI 并发两个 OSC
  序列(`ready` / `exit`)。CLI 根本不知道它存在。
- **最上层是 MCP。** swarm 消息以原生 MCP 工具的形式暴露给 LLM,走 stdio JSON-RPC ——
  `swarmx-mcp` 是个很小的二进制,每个 agent 的 CLI 把它作为子进程拉起、用 stdio 通信。

整个抽象就这么多。其余的一切 —— WebSocket 桥接、录制管线、wake-check、spell 加载器 ——
都建在这三块之上,对 CLI 侧零新增要求。

各引擎的差异都吸收在服务端的 per-CLI 适配器里
(`crates/swarmx-server/src/cli/{claude,codex,opencode,reasonix}.rs`):
opencode 当队长时走全屏 TUI + 它官方的 `/tui` HTTP 控制接口;reasonix 走
`reasonix serve` 的 HTTP/SSE 而非 PTY。从仪表盘视角看,它们都只是 agent。

## 功能特性

| | |
|---|---|
| **真实订阅 CLI** | 拉起的就是你 `$PATH` 上那个 `claude` / `codex` / `opencode` / `reasonix` 二进制。OAuth 用你已有的 `~/.claude/`、`~/.codex/`… 凭据 —— swarmx 从不读取或持久化 token。 |
| **多 agent 网格** | 可同时拉任意多个 agent,每个有自己的面板,xterm.js + WebGL 加速。一个冷却池把浏览器控制在 WebGL 上下文上限内,溢出时静默降级到 DOM。 |
| **Orchestrator 派活** | 每个 workspace 启动一个常驻 orchestrator,在黑板上维护 `task.ledger.md` + `progress.ledger.md`,按需用 `swarm_spawn_worker` 派 worker(Magentic-One 模型 —— 团队随任务伸缩,无预声明拓扑)。 |
| **swarm 消息** | `POST /api/message` 或 CLI 内的 `swarm_send_message` 工具,发出带 `from`、`to`、`kind`、`body` 和可选 `in_reply_to` 线程父级的消息。全部持久化到 SQLite + FTS5。 |
| **共享黑板** | `~/.swarmx/blackboard/` 下的 markdown 文件,带 FTS5 检索、版本化历史(每次写都是一行),改动时 `/ws/swarm` 推事件。 |
| **回合边界 wake-check** | Stop hook 调 `swarmx-mcp wake-check`;agent 若有未读则发 `decision:block` 续跑,让它在下一回合读收件箱 —— 零轮询。opencode 没有阻塞式 Stop hook,用插件给等价的唤醒。 |
| **黑板写入触发的推送式唤醒** | `WakeCoordinator` 订阅 `SwarmEvent::BlackboardChanged`。某 key 被写时,所有 role 声明了 `depends_on=["<key>"]` 的 agent 在同一 tick 被唤醒:既落一条 `kind="wake"` mailbox note,又往订阅者 PTY 注入 `\x15<msg>\r` —— 把已经停下空闲的 agent 重新拉起。 |
| **Directions(git worktree)** | 一个 workspace 可以把一条隔离的 *direction* fork 进它自己的 git worktree,让并行的工作互不冲突;ledger key 按 `workspace_id` + direction slug 命名空间隔离。 |
| **会话录制** | 每个 PTY 会话录成 asciicast v2(`~/.swarmx/recordings/*.cast`),在浏览器里用 `asciinema-player` 回放。 |
| **桌面应用** | 以 Tauri 应用形式发布,把 server、shim、MCP 三个二进制作为 sidecar 打进包。下载 → 安装 → 打开 → 直接用,全程零命令。 |

## 快速开始

### 前置依赖

| 工具 | 版本 | 用途 |
|---|---|---|
| Rust | 1.83+ | workspace 工具链(`rust-toolchain.toml` 钉死) |
| Node | 22+ | Vite 开发服务器 / 生产构建 |
| `claude` | 任意较新版本 | 跑一次 `claude` 完成浏览器 OAuth 登录 |
| `codex` | 0.132+ | 用 `codex login` 登录。**必须 0.132**,因为它才有 `--dangerously-bypass-hook-trust`,wake-check 循环要靠它才能自动触发。 |
| `opencode` / `reasonix` | 可选 | 只有当你想拉这两种引擎时才需要。 |

### 编译 & 启动(开发)

```bash
# 克隆
git clone https://github.com/curdx/swarmx.git
cd swarmx

# 一次构建所有 crate(服务端启动需要 shim 二进制在场)
cargo build --workspace
cd web && npm install && cd ..

# 终端 1：后端(必须从仓库根目录跑,才能就近找到 spells/roles/cli-plugins)
cargo run -p swarmx-server      # 监听 127.0.0.1:7777

# 终端 2:前端(开发模式,带热更新)
cd web && npm run dev           # vite 在 5173,代理 /api + /ws → 7777

# 打开面板
open http://localhost:5173
```

想要生产式的单端口部署(axum 自己 serve 构建好的 bundle),先 `cd web && npm run build`,
然后下次 `cargo run` 后直接访问 `http://127.0.0.1:7777`。

### 第一次 spawn

1. 点击头部的 **+ Claude Code**。出现一个新面板;若是第一次,在内嵌终端里完成 OAuth,
   跟你在 shell 里跑 `claude` 一模一样。
2. 点击 **+ Codex CLI**。首次 codex 会弹一个 `Hooks need review` 对话框 —— swarmx 的
   自动应答在约 500ms 内介入,你直接到达提示符。(可在服务端日志里看到
   `auto-answered codex Hooks-need-review dialog`。)
3. 在任一面板里敲个提示,确认 agent 能回话。

### 跟你 workspace 的 orchestrator 对话

创建一个指向真实项目目录的 workspace。swarmx 会跑内置的 `spells/init.md`,在那个目录里
spawn 一个 **orchestrator** agent(claude)。它扫描你的项目(约 30 秒),往黑板写
`task.ledger.md` + `progress.ledger.md`,然后跟你打招呼。

从此你只用自然语言跟它说话。orchestrator 按任务决定:直接回答、自己动手干、还是用
`swarm_spawn_worker` 派一个或一群 worker —— 团队随任务伸缩(Magentic-One 模型),
而不是预先分配一套固定拓扑。worker 在 swarm 抽屉里来来去去,orchestrator 常驻。

> 这里没有「从下拉框选一个 spell」这一步。早先那批预声明的多 agent spell
> (`critic-loop` / `fullstack-feature*` / `auto-dispatch`)已被这套运行时伸缩派活取代。
> 现在唯一还发的 spell 是 `spells/init.md`;它的 prompt 见 `roles/orchestrator.md`。
> 多 agent 机制(`role_ref` / `allow_cycles` / `shared_workspace`)仍完整实现并带单测,
> 留作未来用。

## 核心概念

| 概念 | 一句话定义 | 在哪 |
|---|---|---|
| **Agent** | 一个跑在 PTY + shim + recorder 下的订阅 CLI 进程,id 形如 `<plugin>-<8hex>`(如 `claude-de332d7b`)。 | `swarmx-server::spawn`、`swarmx-pty` |
| **Plugin** | `cli-plugins/<id>.toml`,声明怎么拉一种 CLI:二进制、默认参数、就绪检测、MCP 注入方式、hook 开关。内置 `claude`、`codex`、`opencode`、`reasonix`。 | `cli-plugins/`、`swarmx-server::plugins` |
| **Workspace** | swarm 操作的一个项目,承载 orchestrator、ledger 和派出的 worker。per-agent 的 CLI 配置覆盖落在 `~/.swarmx/`。 | `swarmx-server::routes::workspaces` |
| **Direction** | workspace 内一条可选的隔离工作分支,背后是独立的 git worktree,让并行 direction 互不踩踏。 | `swarmx-server::worktree` |
| **Orchestrator** | 每个 workspace 唯一的常驻 agent。跑 Magentic-One 双 ledger 循环:扫描 → 打招呼 → 在 workspace 生命周期内持续派活 / 干活 / 对话。 | `roles/orchestrator.md`、`spells/init.md` |
| **swarm 消息** | `messages` 表(SQLite)里一行,寻址 `from_agent → to_agent`,可选 `in_reply_to`。经 `POST /api/message` 或 `swarm_send_message` 工具发出,在 `/ws/swarm` 广播。 | `swarmx-swarm`、`swarmx-storage` |
| **黑板** | `<root>/<path>.md` 的 markdown KV,带完整历史。读用 `swarm_read_blackboard` / `GET /api/blackboard`,写用其反向接口。notify-debouncer 监听文件系统的直接编辑。 | `swarmx-swarm::watcher`、`swarmx-storage` |
| **wake-check** | `swarmx-mcp wake-check` 子命令。读 Stop hook 传来的 stdin JSON,解析 `agent_id`,查未读数,有信则发 `{decision:"block", reason:"…"}`。每个 Stop 事件单次触发 —— 不重启已停下的 agent(那是 WakeCoordinator 的活)。 | `swarmx-mcp::wake_check` |
| **WakeCoordinator** | role 用 `depends_on` 声明要订阅的黑板 key。`BlackboardChanged{key}` 时,给每个订阅者(写入者除外)落一条 `kind="wake"` mailbox note,**并**往其 PTY 注入 `\x15<msg>\r`。spawn 前先做环路检测。 | `swarmx-server::wake` |
| **Spell** | `spells/<name>.md`,TOML front-matter 声明 `[[agents]]`。每个块要么内联 `role/cli/system_prompt`,要么用 `role_ref="<id>"` 继承一个 `roles/<id>.md` 模板。`shared_workspace = true` 把 spawn 从 per-agent 目录切到一个共享 cwd。今天只发 `init.md`。 | `spells/`、`swarmx-server::spells` |
| **Role** | `roles/<id>.md` —— 可复用的 SOP 模板,被 spell 引用。带 `default_cli`、`artifact_paths`、`handoff_signal`、`depends_on`,以及含 `{task}` / `{<role>_id}` 占位符的 `system_prompt_template`。内置:orchestrator、frontend、backend、reviewer、test-runner、docs-writer、researcher、fixer。 | `roles/`、`swarmx-server::roles` |
| **Shim** | `swarmx-shim` —— 约 70 行的二进制,`execvp` 真 CLI 并发 OSC `ready` / `exit` 序列,让 swarmx 无需轮询就能感知生命周期。 | `swarmx-shim` |
| **MCP** | `swarmx-mcp` —— stdio JSON-RPC server,暴露各 `swarm_*` 工具。自动装进每个 agent 的 CLI 配置,让 LLM 当原生工具调。claude 拿到的是 per-agent 的 `--mcp-config` 文件,免得共享 workspace 的 agent 互相覆盖身份。 | `swarmx-mcp` |

## 架构

```
┌─────────────────────────────────────────────────────────────────────┐
│ 浏览器 / Tauri webview(Vite + React 18,xterm.js + WebGL 池)        │
│                                                                     │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────────────┐    │
│  │ Pane #1  │  │ Pane #2  │  │ Pane #N  │  │ swarm 抽屉 +     │    │
│  │ xterm.js │  │ xterm.js │  │ xterm.js │  │ 录制 +           │    │
│  └────┬─────┘  └────┬─────┘  └────┬─────┘  │ DAG / 黑板       │    │
│       │             │             │        └────────┬─────────┘    │
└───────┼─────────────┼─────────────┼─────────────────┼──────────────┘
        │ /ws/pty/    │             │                 │ /ws/swarm
        │ <agent_id>  │             │                 │ + /api/*
        ▼             ▼             ▼                 ▼
┌─────────────────────────────────────────────────────────────────────┐
│ swarmx-server(axum,127.0.0.1:7777,仅 loopback)                     │
│                                                                     │
│   /api/agent  /api/message  /api/blackboard  /api/recording         │
│   /api/spells /api/spell/run /api/plugins   /api/roles  /api/worker │
│                                                                     │
│   ┌─ AppState ────────────────────────────────────────────────┐    │
│   │ PluginRegistry · SpellRegistry · RoleRegistry · Registry  │    │
│   │ Store (SQLite) · Swarm · BlackboardWatcher · WakeCoord    │    │
│   └────────────────────────────────────────────────────────────┘   │
│   per-CLI 适配器:cli/{claude,codex,opencode,reasonix}.rs           │
└──────────────┬──────────────────────────────────────────────────────┘
               │ stdin / stdout (PTY)
               ▼
┌─────────────────────────────────────────────────────────────────────┐
│ swarmx-shim(每 agent 一个,极薄的 Rust 包装)                       │
│   - execvp("claude" | "codex" | "opencode" | "reasonix")            │
│   - 发 OSC ready / exit 序列                                        │
└──────────────┬──────────────────────────────────────────────────────┘
               │
               ▼
┌─────────────────────────────────────────────────────────────────────┐
│ 真实 CLI                                                            │
│   拉起 ─►  swarmx-mcp (stdio)  ◄─►  /api/message 等                 │
│           wake-check (Stop hook)                                    │
└─────────────────────────────────────────────────────────────────────┘
```

### Crate 布局

| Crate | 职责 |
|---|---|
| `swarmx-protocol` | WebSocket 帧 schema、REST DTO。server 与各 client 共享。 |
| `swarmx-shim` | `execvp` 真 CLI 的 OSC 包装(每 agent 一个)。harness-check 校验它进了 Tauri 的 `externalBin`。 |
| `swarmx-pty` | `portable-pty` 包装 + 双线程桥接 + 单调 seq 环形缓冲。 |
| `swarmx-server` | axum HTTP/WS 网关。路由、生命周期、pre-spawn 补丁、spell 执行器、role 注册表、`WakeCoordinator`、reaper、billing、engine-probe。per-CLI 适配在 `src/cli/{claude,codex,opencode,reasonix}.rs`。 |
| `swarmx-swarm` | 每 agent 收件箱、黑板 CRUD、notify-debouncer 文件监听。 |
| `swarmx-mcp` | stdio JSON-RPC MCP server。同时托管被 Stop hook 调用的 `wake-check` 子命令。 |
| `swarmx-storage` | SQLite + FTS5。迁移、agents/messages/recordings/blackboard 表。 |
| `swarmx-recorder` | asciicast v2 写入器,EOF 时 finalize。 |
| `swarmx-cli` | 极薄的 `swarmx up` 入口(目前是 stub —— 请跑 `cargo run -p swarmx-server`)。 |

### 运行时资源(随包发,非 crate)

这些通过 `include_str!` 编译**进**服务端二进制,所以打包后的应用在 `CWD=/`、无环境变量
的情况下也能跑:

- `spells/init.md` —— 唯一发的 spell(spawn orchestrator)。
- `roles/*.md` —— 8 个 role SOP 模板(含 orchestrator)。
- `cli-plugins/*.toml` —— 4 个引擎清单(claude / codex / opencode / reasonix)。

唯一必须作为文件随包发(它是 JS、要交给 opencode/node 执行,没法 embed)的资源是
`cli-plugins/opencode/swarmx-wake.js`,通过 Tauri `bundle.resources` 打包。

## 配置参考

每个 `SWARMX_*` 环境变量都在 [`docs/configuration.md`](docs/configuration.md) 里
有文档(CI 的 harness 检查守住它的完整性)。要点:

| 变量 | 用途 |
|---|---|
| `SWARMX_PORT` | 服务端口(默认 `7777`)。 |
| `SWARMX_DB_PATH` | SQLite 路径(默认 `~/.swarmx/swarmx.db`)。 |
| `SWARMX_MAX_LIVE_AGENTS` | 并发存活 agent 上限。 |
| `SWARMX_RETENTION_DAYS` | 录制 / 活动的保留窗口。 |
| `SWARMX_SHIM_PATH` / `SWARMX_MCP_PATH` | 覆盖打包二进制的位置(由 Tauri sidecar 设置)。 |
| `SWARMX_{SPELLS,ROLES,CLI_PLUGINS}_DIR` | 在编译进去的 builtin 之上做磁盘 overlay(可选)。 |

### `cli-plugins/<id>.toml`

```toml
id                       = "codex"          # 用作 `<id>-<8hex>` agent 前缀
display_name             = "Codex CLI"
binary                   = "codex"          # 经 $PATH 解析
default_args             = ["--dangerously-bypass-approvals-and-sandbox"]
ready_detect             = "shim_osc"       # 或 "prompt_pattern" | "none"
mcp_inject               = "codex_global_toml"
home_env                 = "HOME"

# 每个 `auto_*` 开关切换一项 pre-spawn 补丁。全置 false 表示 swarmx 只是裸 spawn
# CLI,你得自己手动信任 workspace、装 MCP 等。
auto_inject_mcp          = true
auto_trust_workspace     = true   # 写 `[projects.<ws>] trust_level = "trusted"`
auto_dismiss_update      = true   # 设 dismissed_version = latest(仅 codex)
auto_inject_stop_hook    = true   # 写 workspace .codex/hooks.json Stop hook
auto_answer_hooks_dialog = true   # 监听 PTY 的 "Hooks need review" 并发 "2\r"
```

## REST 与 WebSocket API

### REST(仅 loopback)

| 方法 | 路径 | 用途 |
|---|---|---|
| `GET` | `/api/plugins` | 列出已加载的 CLI 插件。 |
| `GET` | `/api/roles` | 列出已加载的 role。 |
| `GET` | `/api/spells` | 列出已加载的 spell 清单。 |
| `POST` | `/api/spell/run` | 跑一个 spell。Body:`{ name, task, workspace_dir? }`。 |
| `POST` | `/api/worker` | spawn 一个 worker agent(`swarm_spawn_worker` 用)。 |
| `GET` `DELETE` | `/api/agent` · `/api/agent/:id` | 列出 / 杀死 agent。 |
| `POST` | `/api/agent/:id/{interrupt,resume,wake}` | 生命周期控制。 |
| `POST` | `/api/message` *(经 swarm 工具)* | 发一条 swarm 消息。 |
| `POST` | `/api/message/read` | 标记消息已读。 |
| `GET` `PUT` | `/api/blackboard` | 列出 / 读 / 写黑板文件。 |
| `GET` | `/api/recording` · `/api/recording/:id` | 列出 / 流式读 `.cast` 文件。 |
| `GET` | `/api/usage` | 每 agent 的 token / 计费用量。 |
| `GET` | `/api/files/list` · `/api/files/read` | 沙箱文件浏览器(凭据路径走 denylist)。 |

### WebSocket

| 路径 | 用途 |
|---|---|
| `/ws/pty/:agent_id` | 双向 PTY 桥。二进制帧是 `[4B BE seq][bytes…]`;文本帧是控制 JSON(`resize`、`ack`、`hello`、`shim_ready`、`shim_exit`)。 |
| `/ws/swarm` | server → client 事件流:`agent_state`、`message`、`message_read`、`blackboard`、`shim_event`、`mcp_health`。 |

## 安全与凭据

swarmx 遵循 **PTY-only 凭据模型**,和 `tmux`、`screen`、`ttyd` 以及官方 CLI 本身用的
是同一套:

- swarmx **从不读取** `~/.claude/`、`~/.codex/` 等目录下的文件。
- swarmx **从不持久化** OAuth token、refresh token、API key。
- swarmx **只**把 `HOME`(和 `PATH`)透传给子进程,让它读*自己*的配置,就跟你在 shell
  里跑它一样。

swarmx 实际写的东西(里面没有任何凭据):

- `~/.swarmx/` 下的 per-agent CLI 配置覆盖(MCP server 条目、Stop hook 配置、workspace
  信任标记)。
- `~/.swarmx/recordings/*.cast` 录制(只有终端输出字节)。
- `~/.swarmx/swarmx.db` 的 SQLite 库(agent 元数据、消息、黑板镜像、录制元数据)。
- `~/.swarmx/wake/<agent_id>.json` 一个小的 wake-check 节流文件。

服务端**只**绑 `127.0.0.1:7777`。没有鉴权,因为没有远程访问 —— 跟 `cargo run` 或
`vite dev` 同样的姿态。DNS-rebind 防御:无 Origin 的请求还要求 Host 是 loopback。
文件浏览器在每次请求上硬拒凭据路径(`~/.ssh`、`~/.aws`、`*.pem`/`*.key`、
`~/.claude.json`…)。

## 打包桌面应用

swarmx 以 Tauri 应用形式发布,把三个服务端二进制(`swarmx-server`、`swarmx-shim`、
`swarmx-mcp`)作为 sidecar 打进包,用户下载 → 安装 → 打开 → 直接用,全程零命令。

```bash
cd web
npm run sidecar:release   # 编译 release 后端 + 拷成 Tauri sidecar
npm run tauri:build       # 出真实安装包(.app / .dmg / …)
npm run tauri:dev         # Tauri 开发(debug 模式不会自动拉后端)
```

> **打包不变量:** 运行时要读的任何东西,要么通过 `include_str!` / `sqlx::migrate!`
> 编译进去,要么通过 Tauri `bundle.resources` 随包发、并在启动 sidecar 时用 `SWARMX_*`
> 环境变量把绝对路径注进去。打包后的应用以 `CWD=/`、且(除非显式设置)无 `SWARMX_*`
> 环境变量运行,所以任何「在仓库根目录跑得好好的相对路径查找」在装好的应用里都会失败。
> 完整发版清单见 `CLAUDE.md`。

## 常见问题

<details>
<summary><b>「我的 codex agent 不理 swarm 消息。」</b></summary>

查 codex 版本:`codex --version` 必须报 **0.132 或更高**。codex 0.132 才有
`--dangerously-bypass-hook-trust`;更早的版本会静默拒绝触发 swarmx 的 Stop hook。
修法:`brew upgrade codex` 或 `npm install -g @openai/codex@latest`,然后重启服务端
(swarmx 每进程探测一次这个 flag)。可在服务端日志确认:
`binary flag probe result … flag="--dangerously-bypass-hook-trust" supported=true`。
</details>

<details>
<summary><b>「codex 每次都弹 'Hooks need review' 对话框。」</b></summary>

那是 codex 0.130+ 正常的信任门。swarmx 的 `auto_answer_hooks_dialog` 开关
(`cli-plugins/codex.toml` 里默认开)会武装一个服务端 watcher,在约 500ms 内合成
`2 + Enter`。若没自动消失,查服务端日志的 `auto-answered codex Hooks-need-review
dialog`;缺这一行通常是 codex 启动时间超过了 watcher 的窗口。
</details>

<details>
<summary><b>「claude 说 'I don't have a swarm_send_message tool available'。」</b></summary>

这发生在 agent 的第一回合早于 MCP 子进程完成握手时。swarmx 在 `ShimReady` 之后已经
等了一段时间来缓解;如果你在 `POST /api/agent` 之后立刻自己注入提示,请加同样的延迟。
</details>

<details>
<summary><b>「明明有 agent 在跑,录制抽屉却是空的。」</b></summary>

录制只在 agent 的 PTY EOF(CLI 退出)时才 finalize。活动中的录制一旦有字节刷出,会在
抽屉里显示 `● live`。如果整行都缺,`tail -f ~/.swarmx/recordings/*.cast` 看文件是否在涨。
</details>

## 贡献

欢迎 PR 和 issue。CI 硬门禁是:`node scripts/harness-check.mjs`(跨文件不变量静态检查)、
`cargo build/test --workspace --locked`、`web` 的 `npm run build`(tsc 类型检查),以及
隔离后端的 `directions-smoke.mjs`。需要真实登录 CLI 的 swarm/resume/stress 烟测是手动的
(`scripts/golden-cli-test.sh`)。

提议新 CLI 插件时,请附一段录好的 OAuth 验证(asciicast 或视频),证明它在全新 checkout
上端到端能跑通。

commit 身份用 per-repo 的 local git config 设置;**commit message 用英文**写:

```bash
git config user.name  "your-name"
git config user.email "your@email"
# 别动 global git config。
```

想跑一个隔离的全栈实例来验证真 UI 改动、又不碰长期开着的 dev 会话:

```bash
bash scripts/test-stack.sh        # 在 7788/5188 端口 build + 起,数据在 /tmp
bash scripts/test-stack.sh stop   # 拆掉
```

## 致谢

swarmx 站在若干开源项目的肩膀上:

- **[portable-pty](https://docs.rs/portable-pty)** —— 每个 agent 跑在上面的 PTY 抽象。
- **[asciinema-player](https://github.com/asciinema/asciinema-player)** ——
  浏览器内录制回放,WASM 渲染、完整键盘控制。
- **[axum](https://github.com/tokio-rs/axum)** / **[Tauri](https://tauri.app/)**
  / **[xterm.js](https://xtermjs.org/)** —— 服务端、桌面端、终端层。
- **Magentic-One** 编排模型 —— orchestrator 设计背后「团队随任务伸缩」的洞见。

## License

[MIT](LICENSE)。完整文本见该文件。
