<h1 align="center">swarmx</h1>

<p align="center">
  <strong>Run your real <code>claude</code>, <code>codex</code>, <code>opencode</code> &amp; <code>reasonix</code> CLIs as a collaborating swarm — in one browser tab.</strong>
</p>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-MIT-blue.svg?style=for-the-badge" alt="MIT License"></a>
  <img src="https://img.shields.io/badge/Rust-1.83+-orange.svg?style=for-the-badge&logo=rust" alt="Rust 1.83+">
  <img src="https://img.shields.io/badge/Node-22+-5FA04E.svg?style=for-the-badge&logo=node.js&logoColor=white" alt="Node 22+">
  <img src="https://img.shields.io/badge/Desktop-Tauri-24C8DB.svg?style=for-the-badge&logo=tauri&logoColor=white" alt="Tauri">
  <a href="README.zh-CN.md"><img src="https://img.shields.io/badge/Lang-中文-red?style=for-the-badge" alt="中文"></a>
</p>

<p align="center">
  <img src="https://raw.githubusercontent.com/curdx/swarmx/main/docs/assets/hero-terminals.png" alt="swarmx dashboard — multiple real CLI agents running side by side" width="100%">
</p>

**swarmx** spawns the *real* subscription-mode coding CLIs you already have on
disk — the same `claude`, `codex`, `opencode`, and `reasonix` binaries — each in
its own PTY-backed terminal pane, and wires them into a swarm that can message
each other, share a blackboard, and divide up a task. You talk to one
**orchestrator** in plain language; it scales the team to the job.

It is **not** another LLM wrapper. Your OAuth, your rate limits, your plan
limits — everything behaves exactly as if you typed `claude` in your own
terminal, because that's literally what's running. swarmx never reads or
persists your tokens.

[Quick Start](#quick-start) · [How it works](#how-it-works) · [Architecture](docs/configuration.md) · [Configuration](docs/configuration.md) · [Contributing](#contributing) · [中文](README.zh-CN.md)

---

## Why swarmx

Most "agent orchestration" tools either **reimplement the LLM client** (and lose
the subscription auth you paid for) or **wrap the CLI at the wrong layer** (ACP,
HTTP shims) that can't reuse your session. swarmx is the thinnest possible layer
that adds coordination without replacing anything:

- 🖥️ **Real CLIs, unmodified.** Each agent is the actual binary under
  `portable-pty`. Same OAuth, same rate limits, same behavior.
- 📬 **Shared inbox.** Agents address each other by id with `swarm_send_message`;
  delivery happens at the recipient's next turn boundary — zero polling.
- 📋 **Shared blackboard.** A markdown KV store with full-text search, versioned
  history, and live push on every write.
- 🧠 **One orchestrator, scaled to the task.** You chat with a single persistent
  agent; it answers, does the work itself, or spawns workers on demand
  (the Magentic-One model — no pre-declared topology).
- ⏰ **Push-style wakeup.** Write a blackboard key and every agent waiting on it
  is revived in the same tick — even one that already stopped idle.
- 🎬 **Everything recorded.** Each session is an asciicast you can replay
  in-browser.

## Quick Start

> **Prereqs:** Rust 1.83+, Node 22+, and at least one logged-in CLI (`claude`;
> `codex` must be **0.132+** for the auto-wake loop). `opencode` / `reasonix`
> are optional.

```bash
git clone https://github.com/curdx/swarmx.git
cd swarmx

# build everything (the server needs the shim binary present)
cargo build --workspace
cd web && npm install && cd ..

# terminal 1 — backend (run from repo root)
cargo run -p swarmx-server          # → 127.0.0.1:7777

# terminal 2 — frontend
cd web && npm run dev               # → http://localhost:5173
```

Open **http://localhost:5173**, point a workspace at a real project directory,
and start talking to its orchestrator. That's it.

Prefer a desktop app? swarmx ships as a Tauri bundle (server, shim, and MCP
binaries baked in as sidecars) — download → install → open → use, zero terminal
commands. See [Packaging](#desktop-app).

## How it works

Three layers, nothing more:

```
  MCP   ─►  swarm_send_message / swarm_write_blackboard / swarm_spawn_worker …
            (native tools the LLM calls; swarmx-mcp speaks stdio JSON-RPC)
  shim  ─►  swarmx-shim execvp's the real CLI, emits OSC ready/exit (~70 lines)
  PTY   ─►  the unmodified claude / codex / opencode / reasonix binary
```

A browser (or Tauri webview) opens a WebSocket per agent for the live terminal,
plus one `/ws/swarm` event stream. The Rust server (axum, loopback-only) owns
spawning, the swarm inbox, the blackboard, recordings, and a `WakeCoordinator`
that turns blackboard writes into agent wakeups. Each engine's quirks
(opencode's TUI, reasonix's HTTP/SSE) are absorbed in per-CLI adapters so the
dashboard sees uniform agents.

<p align="center">
  <img src="https://raw.githubusercontent.com/curdx/swarmx/main/docs/assets/screenshot-orchestrator-chat.png" alt="Talking to the orchestrator in plain language" width="49%">
  <img src="https://raw.githubusercontent.com/curdx/swarmx/main/docs/assets/screenshot-dag.png" alt="Live dependency DAG of the swarm" width="49%">
</p>
<p align="center">
  <img src="https://raw.githubusercontent.com/curdx/swarmx/main/docs/assets/screenshot-ledger.png" alt="Orchestrator task + progress ledgers on the blackboard" width="49%">
  <img src="https://raw.githubusercontent.com/curdx/swarmx/main/docs/assets/screenshot-task-done.png" alt="A finished task" width="49%">
</p>

## Docs

- 📦 **[Configuration reference](docs/configuration.md)** — every `SWARMX_*`
  variable, plugin/role/spell format, REST + WebSocket API.
- 🤝 **[Handoff protocol](docs/handoff-protocol.md)** — blackboard-key
  conventions for explicit producer/consumer contracts.
- 🧭 **[CLAUDE.md](CLAUDE.md)** — repo working conventions + the packaging
  invariant (what must be `include_str!`'d vs. bundled) + release checklist.
- 📝 **[CHANGELOG.md](CHANGELOG.md)** — notable changes per release.

### Bundled runtime resources

Compiled **into** the server binary via `include_str!`, so the packaged app runs
with `CWD=/` and no env vars:

- `spells/init.md` — the one spell that ships (spawns the orchestrator).
- `roles/*.md` — 8 role templates: orchestrator, frontend, backend, reviewer,
  test-runner, docs-writer, researcher, fixer.
- `cli-plugins/*.toml` — 4 engine manifests (claude / codex / opencode / reasonix).

> Earlier pre-declared multi-agent spells (`critic-loop` / `fullstack-feature*` /
> `auto-dispatch`) were removed in favour of the orchestrator's runtime dispatch
> via `swarm_spawn_worker`. The machinery (`role_ref` / `shared_workspace` /
> cycle detection) stays implemented and unit-tested for future use.

<h3 id="desktop-app">Desktop app</h3>

```bash
cd web
npm run sidecar:release   # compile release backend + copy as Tauri sidecar
npm run tauri:build       # produce the real installer (.app / .dmg / …)
```

## Security & Credentials

swarmx uses the **PTY-only credentials model** — the same as `tmux`, `ttyd`, and
the CLIs themselves:

- ❌ Never reads `~/.claude/`, `~/.codex/`, etc.
- ❌ Never persists OAuth tokens, refresh tokens, or API keys.
- ✅ Passes `HOME` / `PATH` to the spawned CLI so it reads *its own* config, just
  like your shell.

The server binds **only** to `127.0.0.1:7777` — no remote access, no auth, the
same posture as `cargo run`. DNS-rebind defense and a credential-path denylist
(`~/.ssh`, `*.pem`, `~/.claude.json`, …) guard the file browser.

## Contributing

PRs and issues welcome. CI hard gates: `node scripts/harness-check.mjs`
(cross-file invariants), `cargo build/test --workspace --locked`, `web`'s
`npm run build` (tsc), and an isolated-backend `directions-smoke.mjs`. Swarm
smoke tests needing real logged-in CLIs are manual (`scripts/golden-cli-test.sh`).

Spin up an isolated full-stack to verify UI changes without touching your dev
session:

```bash
bash scripts/test-stack.sh        # build + start on 7788/5188, data in /tmp
bash scripts/test-stack.sh stop   # tear down
```

Proposing a new CLI plugin? Include a recorded OAuth verification showing it
works end-to-end on a fresh checkout. Commit messages are written in **English**;
set commit identity per-repo (never touch global git config).

## Acknowledgments

Built on [portable-pty](https://docs.rs/portable-pty),
[asciinema-player](https://github.com/asciinema/asciinema-player),
[axum](https://github.com/tokio-rs/axum), [Tauri](https://tauri.app/), and
[xterm.js](https://xtermjs.org/). The orchestrator design follows the
**Magentic-One** "scale the team per task" model.

## License

[MIT](LICENSE).
