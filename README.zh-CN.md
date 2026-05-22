# flockmux-core

<p align="center">
  <strong>把真实的 <code>claude</code>、<code>codex</code> CLI 拉起在 PTY 里，串成蜂群，让它们互相发消息协作完成任务的浏览器面板。</strong>
</p>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-MIT-blue.svg" alt="MIT License"></a>
  <img src="https://img.shields.io/badge/Rust-1.83%2B-orange.svg" alt="Rust 1.83+">
  <img src="https://img.shields.io/badge/Node-22%2B-brightgreen.svg" alt="Node 22+">
  <img src="https://img.shields.io/badge/状态-MVP%20完成%20(M1–M5)-success" alt="status">
  <a href="README.md"><img src="https://img.shields.io/badge/Lang-English-blue" alt="English"></a>
</p>

flockmux 直接拉起你磁盘上**订阅版的真实 CLI**——就是你已经登录过的 `claude`
和 `codex` 二进制——把它们装进同一个浏览器标签页里。每个 agent 拿到一个
PTY 后端的终端 pane（基于 xterm.js + WebGL 加速）。在这之上叠一层协调层，
给 agent 提供 3 个独立运行时没有的能力：

1. **共享收件箱**。任何 agent 都能调 `swarm_send_message` 给另一个 agent
   按 id 发消息；接收方在它下一个 turn 边界通过 Stop hook 触发的
   wake-check 自动看见（不轮询、不 PTY 注入）。
2. **共享黑板**。一个带 FTS5 全文搜索、版本历史、`/ws/swarm` 实时推送的
   markdown KV 存储。任何 agent 编辑都广播给所有连着的客户端。
3. **法术（Spells）**。一个文件就声明一种多 agent 编排拓扑：N 个角色各自
   带 system_prompt，一键拉起。内置的 `critic-loop` 跑写手 → 批评家 →
   编辑 3 个 CLI 调用，编辑同时读写手原稿和批评家笔记，再把终版发到
   `system`。

面板还会把每个 agent 的会话存成 asciicast v2 `.cast` 文件，在浏览器内用
官方 `asciinema-player`（WASM 渲染、完整键盘控制）回放。

flockmux **绝不读、绝不持久化** 你的 OAuth token。它只把 `HOME` 透传给子
进程，让 claude / codex 自己去读 `~/.claude/` / `~/.codex/` 里你已经登录
好的凭据。这是 `tmux` 复用 shell session 凭据的同款模型——详见
[安全与凭据策略](#安全与凭据策略)。

---

## 目录

- [为什么做这个](#为什么做这个)
- [功能特性](#功能特性)
- [截图](#截图)
- [快速开始](#快速开始)
- [核心概念](#核心概念)
- [演练: 60 秒跑 critic-loop](#演练-60-秒跑-critic-loop)
- [架构](#架构)
- [配置参考](#配置参考)
- [REST 与 WebSocket API](#rest-与-websocket-api)
- [安全与凭据策略](#安全与凭据策略)
- [常见问题](#常见问题)
- [路线图](#路线图)
- [致谢](#致谢)
- [贡献](#贡献)
- [License](#license)

---

## 为什么做这个

主流的"agent 编排"项目要么从零写一个 LLM 客户端（丢掉订阅版 CLI 那些
"难用但你已经付了钱"的功能），要么在错误的层包了一下（比如 ACP，绕不开
订阅版认证）。flockmux 是有意做成尽可能最薄的协调层——**不替换任何东西**：

- **底层是 PTY**。每个 agent 都是没改过的 `claude` / `codex`，跑在
  `portable-pty` 里。OAuth、限流、套餐限制的行为和你直接在 shell 里敲
  `claude` 一模一样。
- **上层是 MCP**。蜂群消息通过 stdio JSON-RPC MCP 协议作为原生 tool 暴露
  给 LLM——`flockmux-mcp` 是每个 agent 的 CLI 自己拉起的小子进程，stdio
  通信。
- **中间一个薄薄的 shim**。`flockmux-shim` 是 ~70 行 Rust，`execvp` 真实
  CLI 同时发两个 OSC 序列（`ready` / `exit`）告诉 flockmux"我起来了 /
  我退出了"。CLI 本身完全感知不到。

整个抽象就这 3 层。其他一切——WebSocket 桥、录像、wake-check、法术加载
器——都建在这 3 层之上，对 CLI 侧零额外要求。

## 功能特性

| | |
|---|---|
| **跑真实订阅版 CLI** | 拉起的就是你 `$PATH` 里那个 `claude` 和 `codex`。OAuth 复用你 `~/.claude/` / `~/.codex/` 已有凭据——flockmux 绝不读、绝不持久化 token。 |
| **多 agent 网格** | 任意数量 agent，每个一个 pane，WebGL 加速。浏览器 WebGL context 数有限制（Chrome 16 / Safari 8），冷却池满了自动降级 DOM 渲染。 |
| **蜂群消息** | `POST /api/message` 或 CLI 内 `swarm_send_message` tool 发消息，字段含 `from` / `to` / `kind` / `body` / 可选 `in_reply_to`。全部入 SQLite + FTS5。 |
| **共享黑板** | `~/.flockmux/blackboard/` 下的 markdown 文件，每次写入是一条历史记录，FTS5 搜索、文件系统 watcher 直接编辑也能同步，`/ws/swarm` 实时推送变更。 |
| **Turn 边界 wake-check** | claude 和 codex 的 Stop hook 都调 `flockmux-mcp wake-check`，未读 > 0 时 hook 返回 `decision:block` 让 agent 续 turn 读消息——0 轮询、0 PTY 注入。 |
| **codex 首次启动信任弹窗自动确认** | codex 0.130+ 第一次看到新 hook 路径会弹"Hooks need review"，flockmux 服务端监控 PTY 输出自动按 `2 + Enter`，用户视觉无感。 |
| **asciicast 录制 + 浏览器回放** | 每个 session 一个 `.cast` 文件，录像抽屉里点 ▶ 直接用官方 `asciinema-player`（WASM 渲染、全屏、跳进度）在 sidebar 内回放。 |
| **法术（Spells）** | TOML front-matter + markdown body 声明多 agent 拓扑（`[[agents]] role / cli / system_prompt`）。`POST /api/spell/run` 拉起所有 agent，替换 `{task}` 和 `{<role>_id}` 占位符，注入每个 agent 的引导 prompt。内置 `critic-loop`。 |
| **本地优先** | 只绑 `127.0.0.1:7777`，无鉴权（单用户）。除了 CLI 自己往各家 LLM provider 的请求，flockmux 不发任何对外网络请求。 |

## 截图

> _截图 / asciicast GIF 待补。在此之前请按 [快速开始](#快速开始)
> 自己跑一遍，或参考 [60 秒演练](#演练-60-秒跑-critic-loop)。_

## 快速开始

### 前置依赖

| 工具 | 版本 | 用途 |
|---|---|---|
| Rust | 1.83+ | 工作区工具链（`rust-toolchain.toml` 锁定） |
| Node | 22+ | Vite 开发服务器 / 生产构建 |
| `claude` | 任意近期版本 | 至少跑过一次 `claude` 完成浏览器 OAuth |
| `codex` | **0.132+** | 必须 0.132 及以上：0.132 才支持 `--dangerously-bypass-hook-trust`，没有这个 flag wake-check 跑不起来 |

### 编译 & 启动

```bash
# 克隆仓库
git clone https://github.com/curdx/flockmux-core.git
cd flockmux-core

# 一次构建所有 crate
cargo build --workspace
cd web && npm install && cd ..

# 终端 1：后端
cargo run -p flockmux-server      # 监听 127.0.0.1:7777

# 终端 2：前端（开发模式，带热更新）
cd web && npm run dev             # vite 起在 5173，反代 /api 和 /ws 到 7777

# 打开面板
open http://localhost:5173
```

如果要生产单端口部署（让 axum 自己 serve 静态资源），先跑
`cd web && npm run build`，下次 `cargo run` 之后直接打开
`http://127.0.0.1:7777` 就行。

### 第一次 spawn

1. 顶部点 **+ Claude Code**，新出现一个 pane。如果是首次，在嵌入的终端
   里完成 OAuth 流程，跟你在 shell 里跑 `claude` 完全一样。
2. 点 **+ Codex CLI**。首次启动 codex 会弹 `Hooks need review` 对话框，
   flockmux 的自动应答会在 ~500 毫秒内按掉，你直接进入 prompt。
   （日志里能看到 `auto-answered codex Hooks-need-review dialog`。）
3. 在任何一个 pane 里打字，确认 agent 能正常对话。

### 接通蜂群

右侧 **messages** 抽屉里：

1. **to** 字段填一个 agent id。
2. **body** 字段输入"what is your favorite color, briefly?"。
3. 点 **send**。
4. 在那个 agent 的 pane 里随便打个 prompt（比如 `say hi`）。
5. 看：agent 跑完 `say hi` 这个 turn 后，Stop hook 触发
   `flockmux-mcp wake-check`，看到 `unread=1`，让 agent 续一个 turn 调
   `swarm_list_messages` 拿到消息再调 `swarm_send_message` 回复。回复
   会出现在 messages 抽屉里，带正确的 `in_reply_to` 父链接。

### 跑一个法术

顶部下拉里选 **✨ critic-loop**，输入任务描述（比如
`haiku about Rust async cancellation`），点 **run**。3 个 agent 出现——
写手、批评家、编辑——你在 messages 抽屉里能看到它们一来一往。终版会以
`kind: "reply"` 发回 `system`，`in_reply_to` 指向批评家的笔记。

## 核心概念

| 概念 | 一句话定义 | 代码位置 |
|---|---|---|
| **Agent** | 一个跑在 PTY + shim + recorder 下的订阅版 CLI 进程。ID 形如 `<plugin>-<8 位 hex>`（例如 `claude-de332d7b`）。 | `flockmux-server::spawn`, `flockmux-pty` |
| **Plugin** | `cli-plugins/<id>.toml`，声明怎么 spawn 这个 CLI：二进制、默认参数、就绪检测、MCP 注入方式、hook 安装开关。 | `cli-plugins/`, `flockmux-server::plugins` |
| **Workspace** | 每个 agent 一个目录在 `~/.flockmux/workspaces/<agent_id>/`，里面装这个 agent 自己的 `.claude/` 或 `.codex/` 配置 override。pre-spawn 阶段写好这些文件让 CLI 一启动就认为是受信任的、已配置好的项目。 | `flockmux-server::pre_spawn` |
| **Swarm message** | `messages` 表里的一行（SQLite），地址形如 `from_agent → to_agent`，可选 `in_reply_to`。通过 `POST /api/message` 或 `swarm_send_message` MCP tool 发送，`/ws/swarm` 广播。 | `flockmux-swarm`, `flockmux-storage` |
| **Blackboard** | `<root>/<path>.md` 路径下的 markdown KV，全程版本化。读：`swarm_read_blackboard` / `GET /api/blackboard/...`；写：反方向。notify-debouncer 监听文件系统变更，外部直接编辑也能同步。 | `flockmux-swarm::watcher`, `flockmux-storage` |
| **Wake-check** | `flockmux-mcp wake-check` 子命令。从 Stop hook 的 stdin JSON 拿 `cwd` 字段反推 `agent_id`，调 `/api/message/unread_count`，未读 > 0 就输出 `{decision:"block", reason:"..."}`。`~/.flockmux/wake/<id>.json` 文件做滑动窗口节流。 | `flockmux-mcp::wake_check` |
| **Spell** | `spells/<name>.md`，TOML front-matter 声明 `[[agents]]`（role + cli + system_prompt）。`POST /api/spell/run {name, task}` 拉起，`{task}` 和 `{<role>_id}` 占位符在 PTY 注入前被替换。 | `spells/`, `flockmux-server::spells` |
| **Shim** | `flockmux-shim`，~70 行二进制。`execvp` 真实 CLI 同时发 OSC `ready` / `exit` 序列，让 flockmux 0 轮询就能检测生命周期。 | `flockmux-shim` |
| **MCP** | `flockmux-mcp`，stdio JSON-RPC 服务，给 LLM 暴露 `swarm_send_message` / `swarm_list_messages` / 黑板工具。pre-spawn 自动写到每个 agent 的 CLI 配置里，LLM 把它们当原生 tool 用。 | `flockmux-mcp` |

## 演练: 60 秒跑 critic-loop

```bash
# 1. 拉起后端 + 前端
cargo run -p flockmux-server &
cd web && npm run dev &

# 2. 直接 REST 调用触发法术（UI 内部也是同样的调用）
curl -sX POST http://127.0.0.1:7777/api/spell/run \
  -H 'content-type: application/json' \
  -d '{
        "name": "critic-loop",
        "task": "haiku about Rust async cancellation"
      }' | jq .

# 响应：
# {
#   "spell": "critic-loop",
#   "agents": [
#     { "role": "writer", "cli": "claude", "agent_id": "claude-890b3c93" },
#     { "role": "critic", "cli": "codex",  "agent_id": "codex-5796ef7c" },
#     { "role": "editor", "cli": "claude", "agent_id": "claude-c46442a7" }
#   ]
# }

# 3. 看消息总线
curl -sN http://127.0.0.1:7777/api/message | jq '.[-3:]'
# 会出现 3 条消息：
#   #7  writer → critic   （初稿俳句）
#   #8  critic → editor   （初稿 + 批评家笔记）           in_reply_to=#7
#   #9  editor → system   （终版改稿，kind="reply"）       in_reply_to=#8
```

完整闭环在热缓存下 ~3.5 分钟跑完。每一次手把都是 agent 的 Stop hook
fire 出 `wake-check`，看到上游来的未读，续 turn 调
`swarm_list_messages` → `swarm_send_message`。没有轮询，除了首次的
system-prompt 引导之外也没有 PTY 注入。

## 架构

```
┌─────────────────────────────────────────────────────────────────────┐
│ 浏览器 (Vite + React 18, xterm.js + WebGL 池)                       │
│                                                                     │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────────────┐    │
│  │ Pane #1  │  │ Pane #2  │  │ Pane #N  │  │ swarm 抽屉 +     │    │
│  │ xterm.js │  │ xterm.js │  │ xterm.js │  │ recordings +     │    │
│  └────┬─────┘  └────┬─────┘  └────┬─────┘  │ spells launcher  │    │
│       │             │             │        └────────┬─────────┘    │
└───────┼─────────────┼─────────────┼─────────────────┼──────────────┘
        │ /ws/pty/    │             │                 │ /ws/swarm
        │ <agent_id>  │             │                 │ + /api/*
        ▼             ▼             ▼                 ▼
┌─────────────────────────────────────────────────────────────────────┐
│ flockmux-server (axum, 127.0.0.1:7777，仅 loopback)                 │
│                                                                     │
│   /api/agent    /api/message    /api/blackboard    /api/recording   │
│   /api/spells   /api/spell/run  /api/plugins                        │
│                                                                     │
│   ┌─ AppState ────────────────────────────────────────────────┐    │
│   │ PluginRegistry · SpellRegistry · Registry (live PTY 槽位) │    │
│   │ Store (SQLite) · Swarm · BlackboardWatcher                │    │
│   └────────────────────────────────────────────────────────────┘    │
└──────────────┬──────────────────────────────────────────────────────┘
               │ stdin / stdout (PTY)
               ▼
┌─────────────────────────────────────────────────────────────────────┐
│ flockmux-shim (每个 agent 一个，~70 行 Rust 包装)                   │
│   - execvp("claude" | "codex" ...)                                  │
│   - 发 OSC ready / exit 序列                                        │
└──────────────┬──────────────────────────────────────────────────────┘
               │
               ▼
┌─────────────────────────────────────────────────────────────────────┐
│ 真实 CLI (claude / codex 0.132+)                                    │
│                                                                     │
│   spawns ─►  flockmux-mcp (stdio)  ◄─►  /api/message 等             │
│              wake-check (Stop hook)                                 │
└─────────────────────────────────────────────────────────────────────┘
```

### Crate 布局

| Crate | 行数 | 职责 |
|---|---|---|
| `flockmux-protocol` | ~250 | WebSocket 帧结构、REST DTO。server 和客户端共享。 |
| `flockmux-shim` | ~70 | 发 OSC 的薄包装，`execvp` 真实 CLI。 |
| `flockmux-pty` | ~300 | `portable-pty` 包装 + 双线程桥 + 单调递增 seq 环形缓冲。 |
| `flockmux-server` | ~2500 | axum HTTP/WS gateway。路由、生命周期、pre-spawn 补丁、dialog 自动应答、spell 执行器。 |
| `flockmux-swarm` | ~600 | 每 agent 收件箱、黑板 CRUD、notify-debouncer watcher。 |
| `flockmux-mcp` | ~700 | stdio JSON-RPC MCP server。也是 Stop hook 调用的 `wake-check` 子命令的宿主。 |
| `flockmux-storage` | ~800 | SQLite + FTS5。迁移、agents / messages / recordings / blackboard 表。 |
| `flockmux-recorder` | ~250 | asciicast v2 writer，EOF 时自动 finalize。 |
| `flockmux-cli` | ~50 | 薄入口（`flockmux up` 拉起 server + 打开面板）。 |
| `cli-plugins/` | — | 每 CLI 一个 `.toml`：`claude.toml`、`codex.toml`。 |
| `spells/` | — | 每个法术一个 `.md`：`critic-loop.md`。 |
| `web/` | — | Vite + React + xterm.js + asciinema-player 前端。 |

### 一次 spawn 的数据流

```
1.  浏览器点 "+ Codex CLI"。
2.  POST /api/agent { cli: "codex" }
3.  Server: PluginRegistry.get("codex") → CliPlugin
            spawn::spawn_agent() fork flockmux-shim → exec codex
            pre_spawn::run_codex_patches 写：
              <workspace>/.codex/config.toml  (mcp_servers.flockmux-swarm)
              <workspace>/.codex/hooks.json   (Stop hook → wake-check)
            DialogAutoAnswer 起 30 秒窗口监听 "Hooks need review"
            Recorder 在 recordings_root 下打开 .cast 文件
4.  PTY pump 扫字节流找 OSC_READY → 广播 ShimReady
            Recorder 把每个 chunk 写入 asciicast v2
            Registry 存 AgentSlot（bridge、input_tx、lifecycle_tx）
5.  浏览器开 /ws/pty/codex-XXXX → 双向二进制流
6.  浏览器也开 /ws/swarm → 收 agent_state、message 等事件
7.  codex 启动；flockmux-mcp 作为子进程被它拉起用于 tool 调用
8.  每个 turn 结束：codex Stop hook → flockmux-mcp wake-check → REST
    /api/message/unread_count → 若 >0，输出 {decision:block, reason:...}
    → codex 续一个 turn 读消息并响应。
```

## 配置参考

### `cli-plugins/<id>.toml`

```toml
id                       = "codex"          # `<id>-<8 位 hex>` agent 前缀来源
display_name             = "Codex CLI"
binary                   = "codex"          # 通过 $PATH 解析
default_args             = ["--dangerously-bypass-approvals-and-sandbox"]
ready_detect             = "shim_osc"       # 或 "prompt_pattern" | "none"
mcp_inject               = "codex_global_toml"
home_env                 = "HOME"

# 每个 `auto_*` 开关切换一项 pre-spawn 补丁。全置 false 表示 flockmux
# 只是裸 spawn CLI，要自己手动信任 workspace、装 MCP 等。
auto_inject_mcp          = true
auto_trust_workspace     = true   # 写 `[projects.<ws>] trust_level = "trusted"`
auto_dismiss_update      = true   # codex 专属：跳过"有新版本"弹窗
auto_inject_stop_hook    = true   # 在 workspace 写 .codex/hooks.json
auto_answer_hooks_dialog = true   # 监听 PTY 看到 "Hooks need review" 自动发 "2\r"
```

### `spells/<name>.md`

```markdown
+++
name        = "critic-loop"
description = "writer → critic → editor"

[[agents]]
role          = "writer"
cli           = "claude"
system_prompt = """
你是 WRITER。任务：{task}
通过 swarm_send_message 把初稿交给 critic={critic_id}，editor={editor_id}。
"""

[[agents]]
role          = "critic"
cli           = "codex"
system_prompt = """..."""

[[agents]]
role          = "editor"
cli           = "claude"
system_prompt = """..."""
+++

# 自由发挥的 markdown 正文（文档说明，解析器不读）。
```

运行时替换规则：
- `{task}` → `POST /api/spell/run` 传的任务字符串。
- `{<role>_id}` → 那个 role 实际分到的 `agent_id`（比如 `{writer_id}`
  变成 `claude-890b3c93`）。
- 未知的 `{…}` 占位符保留字面量（故意的——静默丢弃会把法术作者的 bug
  藏起来）。

## REST 与 WebSocket API

### REST（仅 loopback）

| Method | Path | 用途 |
|---|---|---|
| `GET` | `/api/plugins` | 列已加载的 CLI 插件。 |
| `POST` | `/api/agent` | spawn 一个 agent。Body：`{ cli, role?, workspace? }`。 |
| `GET` | `/api/agent` | 列活的 + 历史 agent。 |
| `DELETE` | `/api/agent/:id` | kill 一个 agent。 |
| `GET` | `/api/message` | 列消息，可选 `from` / `to` / `since` 过滤。 |
| `POST` | `/api/message` | 发蜂群消息。 |
| `POST` | `/api/message/read` | 标记消息已读。 |
| `GET` | `/api/message/unread_count` | 查未读数（wake-check 在用）。 |
| `GET` | `/api/blackboard` | 列黑板文件。 |
| `GET` | `/api/blackboard/*path` | 读黑板文件。 |
| `PUT` | `/api/blackboard/*path` | 写黑板文件。 |
| `GET` | `/api/blackboard-history/*path` | 黑板路径的版本历史。 |
| `GET` | `/api/recording` | 列录像。 |
| `GET` | `/api/recording/:id` | 流式返回 `.cast` 原文。 |
| `GET` | `/api/spells` | 列已加载的法术。 |
| `POST` | `/api/spell/run` | 跑一个法术。Body：`{ name, task }`。 |

### WebSocket

| Path | 用途 |
|---|---|
| `/ws/pty/:agent_id` | 双向 PTY 桥。二进制帧格式 `[4B BE seq][bytes…]`；文本帧是控制 JSON（`resize`、`ack`、`hello`、`shim_ready`、`shim_exit`）。 |
| `/ws/swarm` | server → 客户端事件流：`agent_state`、`message`、`message_read`、`blackboard`、`shim_event`、`mcp_health`。 |

## 安全与凭据策略

flockmux 采用 **PTY 透传凭据模型**，和 `tmux`、`screen`、`ttyd`、官方
`claude` & `codex` CLI 自己用的是同一种模型：

- flockmux **绝不读**`~/.claude/` 或 `~/.codex/` 下的任何文件。
- flockmux **绝不持久化**任何 OAuth token、refresh token、API key。
- flockmux **只**把 `HOME` 传给子 CLI，让它去读*它自己*的配置——和你直
  接在 shell 里跑它一模一样。PATH 也透传，方便 CLI 找到自己的子命令。

flockmux **会**写的东西（都是用户每次执行就明确同意的）：

- `~/.flockmux/workspaces/<agent_id>/` 每个 agent 一个工作目录，里面是
  CLI 的每项目配置 override（MCP 服务条目、Stop hook 配置、workspace
  trust 标记）。**这里面不含任何凭据**。
- `~/.flockmux/recordings/*.cast` 录像（只录终端输出字节，不录键盘输入、
  不录环境变量、不录凭据）。
- `~/.flockmux/flockmux.db` SQLite 数据库（agent 元数据、消息、黑板
  镜像、录像元数据）。
- `~/.flockmux/wake/<agent_id>.json` 小小的 wake-check 节流文件
  （epoch ms + 计数器）。

server **只**绑 `127.0.0.1:7777`。没有鉴权，因为没有远程访问——和
`cargo run` 或 `vite dev` 同等安全级别。多机 / 远程访问的方案在
[路线图](#路线图)里。

## 常见问题

<details>
<summary><b>"我的 codex agent 不响应蜂群消息"</b></summary>

先确认 codex 版本：`codex --version` 必须 **0.132 或更高**。codex 0.132
才有 `--dangerously-bypass-hook-trust`，更早的版本会静默拒绝跑
flockmux 的 Stop hook。修复办法：`brew upgrade codex` 或
`npm install -g @openai/codex@latest`，然后重启 server（flockmux 每个
进程只探测一次 flag）。

确认 probe 跑了：`grep 'binary flag probe result' /tmp/.../server.log`
应该能看到 `flag="--dangerously-bypass-hook-trust" supported=true`。
</details>

<details>
<summary><b>"codex 每次都弹 'Hooks need review' 对话框"</b></summary>

这是 codex 0.130+ 的正常信任门。flockmux 在 `cli-plugins/codex.toml`
里默认开了 `auto_answer_hooks_dialog`，会起一个服务端 watcher 在
~500 毫秒内合成 `2 + Enter` 把它按掉。如果你看不到 dialog 被自动关掉，
查日志里有没有 `auto-answered codex Hooks-need-review dialog`。如果
没有，说明 watcher 30 秒窗口期内 dialog 没出现——一般是 codex 启动太慢。
把 `spawn::DialogAutoAnswer` 的 `WINDOW` 常量调大，重新编译。
</details>

<details>
<summary><b>"claude 说 'I don't have a swarm_send_message tool available'"</b></summary>

这是 agent 在 MCP 子进程握手完成之前就开始第一个 turn 了。flockmux 的
spell executor 已经在 `ShimReady` 之后等 2.5 秒缓解这个；如果你是手动
`POST /api/agent` 然后立刻自己注入 prompt，也得自己加这个等待。
</details>

<details>
<summary><b>"录像抽屉空空的，但 agent 明明在跑"</b></summary>

录像只在 agent 的 PTY EOF（即 CLI 退出）时才 finalize。正在录的会以
`● live` 标记出现在抽屉里，只要它有任何字节落盘。如果整条记录都消失了
检查 `tail -f ~/.flockmux/recordings/*.cast` 看文件有没有增长。
</details>

<details>
<summary><b>"浏览器上显示 'WS closed (code 1005)'，那个 pane 刚才还在"</b></summary>

说明那个 pane 的 PTY 退出了（底层 CLI 崩了或正常退出）。XtermPane 组件
会在状态栏显示退出码。这只是个提示，不是 flockmux 自己的错误。
</details>

## 路线图

### 已完成（M1 – M5）

- ✅ **M1** 单 agent PTY + OAuth + WebSocket 桥 + WebGL 池
- ✅ **M2** 多 CLI（claude + codex）+ GridView + WebGL 冷却
- ✅ **M3** 蜂群 L2：每 agent 收件箱、黑板、asciicast 录制
- ✅ **M4** 蜂群 L3：`flockmux-mcp` 暴露 `swarm_send_message` /
            `swarm_list_messages` / 黑板工具
- ✅ **M5a** 可观测性：`read_at`、`in_reply_to`、黑板历史
- ✅ **M5b** Turn 边界 wake-check（claude + codex 0.132）
- ✅ **M5c** 法术（`critic-loop`）+ 浏览器内 asciicast 回放

### Backlog（MVP 之外，全部来自 plan §13）

| 优先级 | 内容 | 工作量 |
|---|---|---|
| P1 | `cli-plugins/gemini.toml`（Google Gemini CLI） | 一个 toml 文件 + 人工验证一遍 OAuth |
| P1 | `cli-plugins/qwen.toml`（阿里千问 CLI） | 同 gemini；`ready_detect = "prompt_pattern"` |
| P1 | `spells/tree-executor.md`（递归任务分解） | 一个 md 文件 |
| P1 | `spells/map-reduce.md`（并行 worker + reducer） | 一个 md 文件 |
| P2 | `cli-plugins/opencode.toml`、`cli-plugins/aider.toml` | 每个 CLI 的 OAuth 适配调研 |
| P2 | `spells/werewolf.md`、`spells/red-team.md` | 每个法术一个 md |
| P2 | 推模式 wake（消息到达时给 idle agent 注入 PTY） | ~80 行 |
| P3 | session token 鉴权 + CORS 远程访问 | 借鉴 hermes-agent 的 `_SESSION_TOKEN` 设计 |
| P3 | Tauri 桌面打包 | 借鉴 golutra 的 `src-tauri/` |
| P3 | Agent 沙箱（Docker / SSH 隔离） | 借鉴 openclaw 的 `agents/sandbox/` |

## 致谢

flockmux 站在几个开源项目的肩膀上：

- **[hermes-agent](https://github.com/NousResearch/hermes-agent)** — PTY
  桥 + 多渠道 gateway 架构。wake-check 的 JSON 线协议直接受 Hermes shell
  hooks 启发。
- **[OpenClaw](https://github.com/openclaw/openclaw)** — 法术 front-matter
  约定、MCP 动态加载、agent 沙箱设计。
- **[swarm-ide](https://github.com/swarm-ide)** — "create + send" 两原语
  哲学、每 agent runner 模型、拓扑即法术的概念。
- **[golutra](https://github.com/golutra)** — Tauri 侧 PTY 管线、WebGL
  冷却池设计、OSC shim 模式、CLI 插件 manifest。
- **[asciinema-player](https://github.com/asciinema/asciinema-player)** —
  浏览器内录像回放。WASM 渲染、完整键盘控制。
- **[portable-pty](https://docs.rs/portable-pty)** — 每个 agent 跑在它上
  面的 PTY 抽象。

## 贡献

flockmux 目前是个人项目。欢迎 PR 和 issue，但响应时间可能很慢。

提新 CLI 插件（Gemini / Qwen / OpenCode / ...）时，请附上一段录像
（asciicast 或视频）证明这个插件在新克隆上能 OAuth + 跑通端到端。MVP
只发了 claude + codex 是因为这两个是亲自验证过的。

更大的结构性改动请先读设计方案文档（在维护者私人 `~/.claude/plans/` 下，
需要可以问要一份）。

本仓库的 commit 身份用 per-repo git config 设置：

```bash
git config user.name  "你的名字"
git config user.email "你的@邮箱"
# 别动 global git config。
```

## License

[MIT](LICENSE)。许可证全文见 LICENSE 文件。
