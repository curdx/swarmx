# flockmux-core

订阅版 Claude Code + Codex CLI 多 agent 蜂群面板。

真实的 `claude` / `codex` 二进制跑在 PTY 里 → 通过 WebSocket 桥到浏览器
xterm.js → 上面叠一层 swarm 协调层（agent 间显式 `swarm_send_message`
+ 共享 markdown 黑板 + turn 边界自动唤醒）。

**状态**: MVP 完成（M1–M5 全部交付）。

## 已实现能力

| 能力 | 说明 |
|---|---|
| 单 agent PTY + OAuth | 浏览器里完成 claude / codex 登录与对话 |
| 多 pane + WebGL 池 | 同屏多 agent 自适应排版，WebGL context 满了自动降级 Canvas |
| Swarm L2: 显式发消息 + 黑板 | agent 之间通过 `/api/message` 互发 + 共享 markdown 文件 + FTS5 搜索 |
| Swarm L3: MCP 工具 | `flockmux-mcp` 把 `swarm_send_message` / `swarm_list_messages` / 黑板读写暴露给 LLM |
| Turn 边界自动唤醒 | claude/codex 每个回合结束触发 Stop hook，wake-check 自动把未读消息塞回下一回合 |
| Codex 信任弹窗自动确认 | 服务端监控 PTY 输出，自动按"信任所有 hook" |
| Asciicast 录制 + 浏览器回放 | 每个 agent 一个 `.cast` 文件，点 ▶ 在 sidebar 内直接播放 |
| Spells（蜂群编排模板） | `spells/<name>.md` 声明拓扑 + 角色 prompt，一键启动多 agent 流水线 |

## 快速开始

```bash
# 后端
cargo run -p flockmux-server          # 监听 127.0.0.1:7777
# 前端（另开终端）
cd web && npm install && npm run dev  # vite 起在 5173，反代 /api 和 /ws
```

打开 <http://localhost:5173>，按顶栏按钮：

- **+ Claude Code** / **+ Codex CLI**：spawn 单个 agent，首次会走 OAuth 流程
- **✨ critic-loop**：选一个 spell + 填任务描述 + 点 `run` → 一键启动 3 个 agent 跑写手→批评家→编辑流水线
- **messages / blackboard / recordings**：右侧抽屉，分别看消息、黑板、录像（带 ▶ 浏览器回放）

第一次启动 codex 可能弹一次 hook 信任确认（每个 workspace 一次），服务端会自动按"信任所有 hook"——用户视觉无感。

## 内置 spell

| spell | 拓扑 | 用途 |
|---|---|---|
| `critic-loop` | writer (claude) → critic (codex) → editor (claude) → system | 初稿 → 挑刺 → 整合定稿 |

spell 格式见 `spells/critic-loop.md` 顶部的 TOML front-matter；写一个新的 markdown 文件丢进 `spells/` 重启 server 即可（无需改 Rust 代码）。

## Crate 布局

```
crates/
├── flockmux-protocol   WebSocket 帧 + REST DTO
├── flockmux-shim       小二进制，OSC ready/exit 包真实 CLI
├── flockmux-pty        portable-pty 包装 + 双线程桥 + seq 环形缓冲
├── flockmux-server     axum HTTP/WS 入口，/ws/pty + /api/*
├── flockmux-swarm      每 agent 收件箱 + 黑板 watcher
├── flockmux-mcp        stdio JSON-RPC，对 LLM 暴露 swarm 工具；也带 wake-check 子命令
├── flockmux-storage    SQLite + FTS5
├── flockmux-recorder   asciicast v2 录制
└── flockmux-cli        `flockmux up` 启动器
cli-plugins/            每 CLI 一个 .toml（目前只有 claude + codex）
spells/                 蜂群编排模板（目前只有 critic-loop）
web/                    Vite + React + xterm.js 前端
```

## 设计文档

详细方案 + crate 边界 + 风险分析见 `~/.claude/plans/twinkly-discovering-dahl-agent-a052c31f56171070f.md`。

## 凭据策略

`HOME` 透传给子进程 PTY，让 claude/codex 自己读 `~/.claude/` / `~/.codex/` 下的 OAuth token。**flockmux 绝不读、绝不持久化** token，遵守 Anthropic 2026/02 ToS。

## License

MIT
