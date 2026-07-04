<h1 align="center">swarmx</h1>

<p align="center">
  <strong>把你真实的 <code>claude</code> / <code>codex</code> / <code>opencode</code> / <code>reasonix</code> / <code>zulu</code> CLI 组成一个协作蜂群，再叠上「多模型研究委员会 + 融合竞赛」—— 全在一个浏览器标签页里。</strong>
</p>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-MIT-blue.svg?style=for-the-badge" alt="MIT License"></a>
  <img src="https://img.shields.io/badge/Rust-1.83+-orange.svg?style=for-the-badge&logo=rust" alt="Rust 1.83+">
  <img src="https://img.shields.io/badge/Node-22+-5FA04E.svg?style=for-the-badge&logo=node.js&logoColor=white" alt="Node 22+">
  <img src="https://img.shields.io/badge/Desktop-Tauri-24C8DB.svg?style=for-the-badge&logo=tauri&logoColor=white" alt="Tauri">
  <a href="README.en.md"><img src="https://img.shields.io/badge/Lang-English-blue?style=for-the-badge" alt="English"></a>
</p>

<p align="center">
  <img src="https://raw.githubusercontent.com/curdx/swarmx/main/docs/assets/hero-terminals.png" alt="swarmx 仪表盘 —— 多个真实 CLI agent 并排运行" width="100%">
</p>

**swarmx** 拉起你磁盘上*真实*的订阅模式编码 CLI —— 就是那个 `claude`、`codex`、
`opencode`、`reasonix`、`zulu` 二进制本身 —— 每个跑在自己独立的、PTY 支撑的终端面板里，
并把它们组织成一个能协作的团队。你只用自然语言跟一个 **队长（orchestrator）** 说话，
它按任务规模伸缩团队。

它**不是**又一个 LLM 套壳。你的 OAuth、你的限流、你的套餐限制 —— 一切行为都跟你在
自己终端里敲 `claude` 完全一致，因为跑的就是它本身。swarmx 从不读取、也从不持久化
你的 token。

[快速开始](#快速开始) · [三种玩法](#三种玩法) · [工作原理](#工作原理) · [配置](docs/configuration.md) · [English](README.en.md)

---

## 三种玩法

同一个仪表盘，同一批真实 CLI，三种协作范式按需切换：

### 🐝 蜂群协作（默认）

跟一个常驻的**队长**说人话。它自己判断：要么直接干，要么用 `swarm_spawn_worker`
即时派出 worker（Magentic-One 模型，不预声明拓扑）。成员之间靠**共享收件箱**
（`swarm_send_message`，在对方下一个回合边界投递，零轮询）和**共享黑板**
（带全文检索、版本历史、写入即推送）协作；黑板某个 key 被写时，所有等它的 agent
在同一 tick 被唤醒 —— 连已经停下的也当场复活。

<p align="center">
  <img src="https://raw.githubusercontent.com/curdx/swarmx/main/docs/assets/screenshot-orchestrator-chat.png" alt="用自然语言跟队长对话" width="49%">
  <img src="https://raw.githubusercontent.com/curdx/swarmx/main/docs/assets/screenshot-dag.png" alt="蜂群的实时依赖协作图" width="49%">
</p>

### 🧠 研究委员会（多模型会诊）

高价值决策不该只听一个模型。**研究委员会**让 N 个模型并行答同一个问题 →
一个 judge 做**结构化对比**（共识 / 矛盾 / 独特洞察 / 盲区，是「比较」不是「投票」）→
外层模型据此**综合出定稿**。技术选型、竞品分析、方案评审、高风险决策的反方检查 ——
一次会诊，胜过反复追问单个模型。

<p align="center">
  <img src="https://raw.githubusercontent.com/curdx/swarmx/main/docs/assets/screenshot-consult.png" alt="研究委员会 —— 多模型并行答题、结构化对比、综合定稿" width="80%">
</p>

### ⚔️ 融合竞赛（多模型写代码，一键全自动）

把**同一个需求**丢给 N 个模型，各自在隔离的 git worktree 里独立实现，
一个可选的**客观检查命令**（如 `pytest`、`cargo test`）当硬门禁，judge 再
**综合各家最优**合并回主线。新手只需填一句需求、点一下「一键开赛」——
选模型、并行实现、跑门禁、综合、合并，全程零手动。裁决由**服务端看门狗**兜底：
无论模型是否规规矩矩收尾，结果都不会静默卡死。

> 🔑 **研究委员会 / 融合竞赛的多模型来源 = [Comate Zulu](https://www.npmjs.com/package/@comate/zulu)**：
> 一把 Comate license 即可驱动十余个模型（DeepSeek / GLM / Kimi / MiniMax…）。
> 在「设置 → 插件」页**一键安装 zulu** 并填入 license 即可，见[下方](#快速开始)。

## 为什么做这个

大多数「agent 编排」工具，要么**重写一个 LLM 客户端**（丢掉你付费买到的订阅鉴权），
要么**在错误的层去包 CLI**（ACP、HTTP shim，没法复用你的会话）。swarmx 是能加协调、
但什么都不替换的最薄一层：

- 🖥️ **真实 CLI，原样不动。** 每个 agent 都是 `portable-pty` 下的真二进制。同样的
  OAuth、同样的限流、同样的行为。
- 📬 **共享收件箱。** agent 之间用 id 互相寻址，投递发生在对方下一个回合边界 —— 零轮询。
- 📋 **共享黑板。** 带全文检索、版本历史、写入即推送的 markdown KV 存储。
- 🧠 **一个队长，按任务伸缩。** 你只跟一个常驻 agent 对话；它自己判断该答、该干、还是该派人。
- ⏰ **推式唤醒。** 写一个黑板 key，等它的每个 agent 同 tick 复活 —— 连停下发呆的也算。
- 🧠 **多模型会诊 & 竞赛。** 研究委员会 + 融合竞赛，把「换个模型问一遍」升级成结构化的综合。
- 🎬 **全程录像。** 每个会话都是一段可在浏览器里重放的 asciicast。

## 快速开始

> **前置：** Rust 1.83+、Node 22+，以及至少一个已登录的 CLI（`claude`；
> `codex` 需 **0.132+** 才有自动唤醒回合）。`opencode` / `reasonix` 可选。

```bash
git clone https://github.com/curdx/swarmx.git
cd swarmx

# 全量构建（server 需要 shim 二进制在场）
cargo build --workspace
cd web && npm install && cd ..

# 终端 1 —— 后端（必须从仓库根目录起）
cargo run -p swarmx-server          # → 127.0.0.1:7777

# 终端 2 —— 前端
cd web && npm run dev               # → http://localhost:5173
```

打开 **http://localhost:5173**，把工作空间指到一个真实项目目录，开始跟它的队长对话。就这样。

**想用多模型会诊 / 融合竞赛？** 打开「设置 → 插件」，在 Comate Zulu 那一栏点
**「一键安装」**（后端跑 `npm install -g @comate/zulu` 并实时回传日志），装完在同一页
填入你的 **Comate license** 即可 —— 一把 license 十余个模型。

想要桌面 App？swarmx 打成 Tauri 包（server / shim / MCP 三个二进制作为 sidecar 内嵌）——
**下载 → 安装 → 打开 → 能用，全程零命令行**。见 [桌面端](#桌面端)。

## 工作原理

三层，仅此而已：

```
  MCP   ─►  swarm_send_message / swarm_write_blackboard / swarm_spawn_worker …
            （LLM 直接调用的原生工具；swarmx-mcp 讲 stdio JSON-RPC）
  shim  ─►  swarmx-shim execvp 真 CLI，发 OSC ready/exit（约 70 行）
  PTY   ─►  未经修改的 claude / codex / opencode / reasonix / zulu 二进制
```

浏览器（或 Tauri webview）为每个 agent 开一条 WebSocket 跑实时终端，外加一条
`/ws/swarm` 事件流。Rust 服务端（axum，仅 loopback）负责拉起进程、收件箱、黑板、录像，
以及把黑板写入变成 agent 唤醒的 `WakeCoordinator`。各引擎的怪癖（opencode 的 TUI、
reasonix / zulu 的 HTTP/SSE）都被各自的 per-CLI 适配器吸收，仪表盘看到的是统一的 agent。

## 文档

- 📦 **[配置参考](docs/configuration.md)** —— 每个 `SWARMX_*` 变量、插件/角色/spell 格式、REST + WebSocket API。
- 🤝 **[交接协议](docs/handoff-protocol.md)** —— 显式生产者/消费者契约的黑板 key 约定。
- 🧭 **[CLAUDE.md](CLAUDE.md)** —— 仓库工作约定 + 打包不变量 + 发版清单。
- 📝 **[CHANGELOG.md](CHANGELOG.md)** —— 每个版本的显著变更。

### 随包发的运行时资源

通过 `include_str!` 编译**进** server 二进制，所以打包版 `CWD=/`、无环境变量也能跑：

- `spells/init.md` —— 唯一随包发的 spell（建空间时拉起队长）。
- `roles/*.md` —— 8 个角色模板：队长、前端、后端、评审、测试、文档、研究、修复。
- `cli-plugins/*.toml` —— 引擎清单（claude / codex / opencode / reasonix / zulu）。

<h3 id="桌面端">桌面端</h3>

```bash
cd web
npm run sidecar:release   # 编译 release 后端 + 拷成 Tauri sidecar
npm run tauri:build       # 出真实安装包（.app / .dmg / …）
```

## 安全与凭据

swarmx 用**纯 PTY 凭据模型** —— 跟 `tmux`、`ttyd` 以及 CLI 本身一样：

- ❌ 从不读 `~/.claude/`、`~/.codex/` 等。
- ❌ 从不持久化 OAuth token、refresh token、API key。
- ✅ 把 `HOME` / `PATH` 透传给子 CLI，让它读*它自己*的配置，就像你的 shell 一样。
- 🔑 Comate license 只存本机 `~/.swarmx/comate.json`，用于驱动 zulu 的多模型。

服务端**只**绑 `127.0.0.1:7777` —— 无远程访问、无鉴权，跟 `cargo run` 同样的姿态。
文件浏览器有 DNS-rebind 防护和凭据路径黑名单（`~/.ssh`、`*.pem`、`~/.claude.json`…）。

## 贡献

欢迎 PR 和 issue。CI 硬门禁：`node scripts/harness-check.mjs`（跨文件不变量）、
`cargo build/test --workspace --locked`、`web` 的 `npm run build`（tsc），以及隔离后端的
`directions-smoke.mjs`。需要真实登录 CLI 的 swarm 烟测是手动的（`scripts/golden-cli-test.sh`）。

起一个隔离的全栈来验证 UI 改动，不碰你的开发会话：

```bash
bash scripts/test-stack.sh        # build + 起在 7788/5188，数据在 /tmp
bash scripts/test-stack.sh stop   # 拆掉
```

## Star History

<a href="https://www.star-history.com/#curdx/swarmx&Date">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/svg?repos=curdx/swarmx&type=Date&theme=dark" />
    <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/svg?repos=curdx/swarmx&type=Date" />
    <img alt="Star History Chart" src="https://api.star-history.com/svg?repos=curdx/swarmx&type=Date" />
  </picture>
</a>

## 致谢

基于 [portable-pty](https://docs.rs/portable-pty)、
[asciinema-player](https://github.com/asciinema/asciinema-player)、
[axum](https://github.com/tokio-rs/axum)、[Tauri](https://tauri.app/)、
[xterm.js](https://xtermjs.org/) 构建。队长设计遵循 **Magentic-One** 的
「按任务伸缩团队」模型。

## 许可

[MIT](LICENSE)。
