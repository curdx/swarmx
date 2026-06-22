<h1 align="center">swarmx</h1>

<p align="center">
  <strong>把你真实的 <code>claude</code>、<code>codex</code>、<code>opencode</code>、<code>reasonix</code> CLI 组成一个协作蜂群 —— 全在一个浏览器标签页里。</strong>
</p>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-MIT-blue.svg?style=for-the-badge" alt="MIT License"></a>
  <img src="https://img.shields.io/badge/Rust-1.83+-orange.svg?style=for-the-badge&logo=rust" alt="Rust 1.83+">
  <img src="https://img.shields.io/badge/Node-22+-5FA04E.svg?style=for-the-badge&logo=node.js&logoColor=white" alt="Node 22+">
  <img src="https://img.shields.io/badge/Desktop-Tauri-24C8DB.svg?style=for-the-badge&logo=tauri&logoColor=white" alt="Tauri">
  <a href="README.md"><img src="https://img.shields.io/badge/Lang-English-blue?style=for-the-badge" alt="English"></a>
</p>

<p align="center">
  <img src="https://raw.githubusercontent.com/curdx/swarmx/main/docs/assets/hero-terminals.png" alt="swarmx 仪表盘 —— 多个真实 CLI agent 并排运行" width="100%">
</p>

**swarmx** 拉起你磁盘上*真实*的订阅模式编码 CLI —— 就是那个 `claude`、`codex`、
`opencode`、`reasonix` 二进制本身 —— 每个跑在自己独立的、PTY 支撑的终端面板里,
并把它们组成一个蜂群:互相发消息、共享一块黑板、一起把任务拆着干。你只用自然语言
跟一个 **orchestrator** 说话,它按任务规模伸缩团队。

它**不是**又一个 LLM 套壳。你的 OAuth、你的限流、你的套餐限制 —— 一切行为都跟你在
自己终端里敲 `claude` 完全一致,因为跑的就是它本身。swarmx 从不读取、也从不持久化
你的 token。

[快速开始](#快速开始) · [工作原理](#工作原理) · [架构](docs/configuration.md) · [配置](docs/configuration.md) · [贡献](#贡献) · [English](README.md)

---

## 为什么做这个

大多数「agent 编排」工具,要么**重写一个 LLM 客户端**(丢掉你付费买到的订阅鉴权),
要么**在错误的层去包 CLI**(ACP、HTTP shim,没法复用你的会话)。swarmx 是「能加协调、
又不替换任何东西」的最薄一层:

- 🖥️ **真 CLI,未经修改。** 每个 agent 都是 `portable-pty` 下那个真二进制。同样的 OAuth、限流、行为。
- 📬 **共享收件箱。** agent 用 `swarm_send_message` 按 id 互相寻址;在收件方下个回合边界投递 —— 零轮询。
- 📋 **共享黑板。** 带全文检索、版本化历史的 markdown KV 存储,每次写入实时推送。
- 🧠 **一个 orchestrator,随任务伸缩。** 你只跟一个常驻 agent 对话;它直接回答、自己动手、或按需派 worker(Magentic-One 模型 —— 无预声明拓扑)。
- ⏰ **推送式唤醒。** 写一个黑板 key,所有等它的 agent 在同一 tick 被唤醒 —— 哪怕已经空闲停下。
- 🎬 **全程录制。** 每个会话都是一段可在浏览器里回放的 asciicast。

## 快速开始

> **前置:** Rust 1.83+、Node 22+,以及至少一个已登录的 CLI(`claude`;`codex` 需
> **0.132+** 才能自动唤醒)。`opencode` / `reasonix` 可选。

```bash
git clone https://github.com/curdx/swarmx.git
cd swarmx

# 全量构建(服务端启动需要 shim 二进制在场)
cargo build --workspace
cd web && npm install && cd ..

# 终端 1 —— 后端(从仓库根目录跑)
cargo run -p swarmx-server          # → 127.0.0.1:7777

# 终端 2 —— 前端
cd web && npm run dev               # → http://localhost:5173
```

打开 **http://localhost:5173**,把一个 workspace 指向真实项目目录,开始跟它的
orchestrator 对话。就这样。

想要桌面应用?swarmx 以 Tauri 包发布(server、shim、MCP 三个二进制作为 sidecar 打进包)
—— 下载 → 安装 → 打开 → 直接用,全程零命令。见 [打包](#桌面应用)。

## 工作原理

三层,仅此而已:

```
  MCP   ─►  swarm_send_message / swarm_write_blackboard / swarm_spawn_worker …
            (LLM 当原生工具调;swarmx-mcp 走 stdio JSON-RPC)
  shim  ─►  swarmx-shim execvp 真 CLI,发 OSC ready/exit(约 70 行)
  PTY   ─►  未经修改的 claude / codex / opencode / reasonix 二进制
```

浏览器(或 Tauri webview)为每个 agent 开一条 WebSocket 接实时终端,外加一条
`/ws/swarm` 事件流。Rust 服务端(axum,仅 loopback)负责 spawn、swarm 收件箱、黑板、
录制,以及一个把黑板写入转成 agent 唤醒的 `WakeCoordinator`。各引擎的差异
(opencode 的 TUI、reasonix 的 HTTP/SSE)都吸收在 per-CLI 适配器里,所以仪表盘看到的
是统一的 agent。

<p align="center">
  <img src="https://raw.githubusercontent.com/curdx/swarmx/main/docs/assets/screenshot-orchestrator-chat.png" alt="用自然语言跟 orchestrator 对话" width="49%">
  <img src="https://raw.githubusercontent.com/curdx/swarmx/main/docs/assets/screenshot-dag.png" alt="蜂群的实时依赖 DAG" width="49%">
</p>
<p align="center">
  <img src="https://raw.githubusercontent.com/curdx/swarmx/main/docs/assets/screenshot-ledger.png" alt="orchestrator 在黑板上的任务 + 进度 ledger" width="49%">
  <img src="https://raw.githubusercontent.com/curdx/swarmx/main/docs/assets/screenshot-task-done.png" alt="完成的任务" width="49%">
</p>

## 文档

- 📦 **[配置参考](docs/configuration.md)** —— 每个 `SWARMX_*` 变量、plugin/role/spell 格式、REST + WebSocket API。
- 🤝 **[Handoff 协议](docs/handoff-protocol.md)** —— 显式 producer/consumer 契约的黑板 key 约定。
- 🧭 **[CLAUDE.md](CLAUDE.md)** —— 仓库工作约定 + 打包不变量(什么必须 `include_str!`、什么必须随包发)+ 发版清单。
- 📝 **[CHANGELOG.md](CHANGELOG.md)** —— 每个版本的显著变更。

### 随包发的运行时资源

通过 `include_str!` 编译**进**服务端二进制,所以打包后的应用在 `CWD=/`、无环境变量时也能跑:

- `spells/init.md` —— 唯一发的 spell(spawn orchestrator)。
- `roles/*.md` —— 8 个 role 模板:orchestrator、frontend、backend、reviewer、test-runner、docs-writer、researcher、fixer。
- `cli-plugins/*.toml` —— 4 个引擎清单(claude / codex / opencode / reasonix)。

> 早先那批预声明的多 agent spell(`critic-loop` / `fullstack-feature*` /
> `auto-dispatch`)已被 orchestrator 用 `swarm_spawn_worker` 的运行时派活取代。
> 相关机制(`role_ref` / `shared_workspace` / 环路检测)仍完整实现并带单测,留作未来用。

<h3 id="桌面应用">桌面应用</h3>

```bash
cd web
npm run sidecar:release   # 编译 release 后端 + 拷成 Tauri sidecar
npm run tauri:build       # 出真实安装包(.app / .dmg / …)
```

## 安全与凭据

swarmx 用 **PTY-only 凭据模型** —— 和 `tmux`、`ttyd` 以及 CLI 本身用的是同一套:

- ❌ 从不读取 `~/.claude/`、`~/.codex/` 等。
- ❌ 从不持久化 OAuth token、refresh token、API key。
- ✅ 把 `HOME` / `PATH` 透传给子进程,让它读*自己*的配置,就跟你的 shell 一样。

服务端**只**绑 `127.0.0.1:7777` —— 没有远程访问、没有鉴权,跟 `cargo run` 同样的姿态。
DNS-rebind 防御 + 凭据路径 denylist(`~/.ssh`、`*.pem`、`~/.claude.json`…)守住文件浏览器。

## 贡献

欢迎 PR 和 issue。CI 硬门禁:`node scripts/harness-check.mjs`(跨文件不变量)、
`cargo build/test --workspace --locked`、`web` 的 `npm run build`(tsc),以及隔离后端的
`directions-smoke.mjs`。需要真实登录 CLI 的 swarm 烟测是手动的(`scripts/golden-cli-test.sh`)。

跑一个隔离的全栈来验证 UI 改动、又不碰你的 dev 会话:

```bash
bash scripts/test-stack.sh        # 在 7788/5188 build + 起,数据在 /tmp
bash scripts/test-stack.sh stop   # 拆掉
```

提议新 CLI 插件?请附一段录好的 OAuth 验证,证明它在全新 checkout 上端到端能跑通。
commit message 用**英文**写;commit 身份按 per-repo 设置(绝不动 global git config)。

## 致谢

构建于 [portable-pty](https://docs.rs/portable-pty)、
[asciinema-player](https://github.com/asciinema/asciinema-player)、
[axum](https://github.com/tokio-rs/axum)、[Tauri](https://tauri.app/) 和
[xterm.js](https://xtermjs.org/) 之上。orchestrator 设计遵循 **Magentic-One** 的
「团队随任务伸缩」模型。

## License

[MIT](LICENSE)。
