# flockmux-core

Multi-agent swarm dashboard for **subscription-mode** Claude Code & Codex CLI.

Real `claude` / `codex` binaries spawned under PTY → xterm.js in browser → swarm
coordination layer (explicit `send` between agents + shared markdown blackboard).

**Status**: M1 (single-agent PTY + OAuth flow). See `.claude/plans/` for the
design plan.

## Quick start (M1)

```
cargo run -p flockmux-server   # bind 127.0.0.1:7777
cd web && npm run dev          # vite dev on 5173, proxies /api + /ws
```

Open <http://localhost:5173>, pick `claude` or `codex`, complete OAuth in the
embedded terminal.

## Layout

- `crates/flockmux-protocol`  — WS frame schema, REST DTOs
- `crates/flockmux-shim`      — tiny binary that wraps real CLI with OSC ready/exit
- `crates/flockmux-pty`       — portable-pty wrapper + 2-thread bridge + seq ring
- `crates/flockmux-server`    — axum HTTP/WS entry, /ws/pty + /api/*
- `crates/flockmux-swarm`     — per-agent inbox runner, blackboard watcher (M3+)
- `crates/flockmux-mcp`       — stdio swarm-mcp server (M4)
- `crates/flockmux-storage`   — SQLite + FTS5 (M3+)
- `crates/flockmux-recorder`  — asciicast v2 record/replay (M3+)
- `crates/flockmux-cli`       — `flockmux up` launcher
- `cli-plugins/`              — per-CLI toml (claude/codex; gemini/qwen/opencode in backlog)
- `spells/`                   — orchestration templates (critic-loop in MVP)
- `web/`                      — Vite + React + xterm.js dashboard

## License

MIT
