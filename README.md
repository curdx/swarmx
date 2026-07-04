<h1 align="center">swarmx</h1>

<p align="center">
  在一个浏览器标签页里，把你本机真实的 <code>claude</code> / <code>codex</code> / <code>opencode</code> / <code>reasonix</code> / <code>zulu</code> 命令行，组成一支会协作的 AI 团队。
</p>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-MIT-blue.svg?style=for-the-badge" alt="MIT License"></a>
  <img src="https://img.shields.io/badge/Rust-1.83+-orange.svg?style=for-the-badge&logo=rust" alt="Rust 1.83+">
  <img src="https://img.shields.io/badge/Node-22+-5FA04E.svg?style=for-the-badge&logo=node.js&logoColor=white" alt="Node 22+">
  <img src="https://img.shields.io/badge/Desktop-Tauri-24C8DB.svg?style=for-the-badge&logo=tauri&logoColor=white" alt="Tauri">
  <a href="README.en.md"><img src="https://img.shields.io/badge/Lang-English-blue?style=for-the-badge" alt="English"></a>
</p>

<p align="center">
  <img src="https://raw.githubusercontent.com/curdx/swarmx/main/docs/assets/hero-terminals.png" alt="swarmx 仪表盘：多个真实 CLI agent 并排运行" width="100%">
</p>

swarmx 是一个跑在本地的浏览器仪表盘。它在 PTY 下拉起你已经装好、登录好的编码
CLI——每个 agent 就是那个二进制本身——再给它们接上一套协作层：共享收件箱、共享黑板、
一个统一入口的队长。你用自然语言跟队长说要做什么，它自己拆解、派人、把结果汇总回来。

跑的是 CLI 本身，不是套壳。所以 OAuth、限流、套餐额度这些，都跟你在终端里敲
`claude` 时一模一样。swarmx 不读、也不存你的任何 token。

## 三件事

**蜂群协作。** 跟队长说需求，它自己决定是直接做，还是拆开派几个 worker（Magentic-One
那套：不预先画流程图，按任务临时派）。成员之间靠收件箱互相寻址——消息在对方下一个
回合边界投递，不轮询；靠黑板共享状态——黑板某个 key 一被写，所有在等它的 agent 当场
被唤醒，包括已经停下发呆的那些。

<p align="center">
  <img src="https://raw.githubusercontent.com/curdx/swarmx/main/docs/assets/screenshot-orchestrator-chat.png" alt="用自然语言跟队长对话" width="49%">
  <img src="https://raw.githubusercontent.com/curdx/swarmx/main/docs/assets/screenshot-dag.png" alt="蜂群的实时依赖协作图" width="49%">
</p>

**研究委员会。** 重要决策别只问一个模型。让若干模型并行回答同一个问题，一个 judge
把它们的答案拆成共识、分歧、独特点、盲区来对比（是对比，不是投票），再由外层模型
综合出一份定稿。技术选型、方案评审、给一个高风险决定找反方意见——都比反复追问单个
模型强。

<p align="center">
  <img src="https://raw.githubusercontent.com/curdx/swarmx/main/docs/assets/screenshot-consult.png" alt="研究委员会：多模型并行答题、结构化对比、综合定稿" width="80%">
</p>

**融合竞赛。** 同一个需求，丢给几个模型各写一版，每版在自己隔离的 git worktree 里独立
实现；可以挂一条客观检查命令（比如 `pytest`、`cargo test`）当硬门禁，judge 再综合各家
最优、合并回主线。新手只要填一句需求、勾上「全自动」、点一下，后面选模型、并行实现、
跑检查、综合、合并全自动，人不用管。裁决由服务端兜底，不会因为某个模型没收好尾就卡死。

研究委员会和融合竞赛的多模型，来自 [Comate Zulu](https://www.npmjs.com/package/@comate/zulu)——
一把 license 就能调十几个模型。在「设置 → 插件」页把 zulu 一键装上、填进 license 即可。

## 快速开始

前置：Rust 1.83+、Node 22+，以及至少一个登录好的 CLI（`claude`；想要自动唤醒回合的话
`codex` 需 0.132+）。opencode / reasonix 可选。

```bash
git clone https://github.com/curdx/swarmx.git
cd swarmx

# 全量构建（server 需要 shim 二进制在场，先跑这个）
cargo build --workspace
cd web && npm install && cd ..

# 终端 1：后端（从仓库根目录起）
cargo run -p swarmx-server          # → 127.0.0.1:7777

# 终端 2：前端
cd web && npm run dev               # → http://localhost:5173
```

打开 http://localhost:5173，把工作空间指向一个真实项目目录，直接跟队长说话就行。

想用研究委员会或融合竞赛，去「设置 → 插件」把 Comate Zulu 一键装上、填进 license——
一把 license，十几个模型。

打成桌面包（Tauri）后，server / shim / mcp 三个二进制作为 sidecar 内嵌，下载装好打开
就能用，全程不碰命令行。

## 原理

三层，没别的：

```
  MCP   ─►  swarm_send_message / swarm_write_blackboard / swarm_spawn_worker …
            （LLM 直接调用的原生工具；swarmx-mcp 讲 stdio JSON-RPC）
  shim  ─►  swarmx-shim execvp 真 CLI，发 OSC ready/exit（约 70 行）
  PTY   ─►  未经修改的 claude / codex / opencode / reasonix / zulu 二进制
```

浏览器给每个 agent 开一条 WebSocket 跑实时终端，另有一条 `/ws/swarm` 事件流。Rust
服务端（axum，只绑 loopback）管进程、收件箱、黑板、录像，以及把黑板写入变成唤醒的
调度器。各引擎的差异——opencode 的 TUI、reasonix / zulu 的 HTTP/SSE——都收在各自的
适配器里，仪表盘看到的是统一的 agent。

## 文档

- [配置参考](docs/configuration.md)：每个 `SWARMX_*` 变量、插件 / 角色 / spell 格式、REST + WebSocket API。
- [交接协议](docs/handoff-protocol.md)：显式生产者 / 消费者契约的黑板 key 约定。
- [CLAUDE.md](CLAUDE.md)：仓库工作约定、打包不变量、发版清单。
- [CHANGELOG.md](CHANGELOG.md)：每个版本的显著变更。

运行时资源（`spells/init.md`、`roles/*.md`、`cli-plugins/*.toml`）用 `include_str!` 编译进
server 二进制，所以打包版在 `CWD=/`、无环境变量的情况下也能跑。

## 安全

跟 `tmux`、`ttyd` 以及 CLI 本身一样的纯 PTY 凭据模型：不读 `~/.claude/`、`~/.codex/`
这些目录，不存 OAuth token / API key，只把 `HOME` / `PATH` 透传给子 CLI，让它读自己的
配置。Comate license 只存本机 `~/.swarmx/comate.json`。

服务端只绑 `127.0.0.1:7777`，无远程访问、无鉴权，跟 `cargo run` 一个姿态。文件浏览器有
DNS-rebind 防护和凭据路径黑名单（`~/.ssh`、`*.pem`、`~/.claude.json` 等）。

## 贡献

CI 硬门禁：`node scripts/harness-check.mjs`、`cargo build/test --workspace --locked`、
`web` 的 `npm run build`，以及隔离后端的 `directions-smoke.mjs`。需要真实登录 CLI 的 swarm
烟测是手动的（`scripts/golden-cli-test.sh`）。

想验 UI 改动又不想碰你的开发会话，起一个隔离全栈：

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

## 许可

[MIT](LICENSE)。
