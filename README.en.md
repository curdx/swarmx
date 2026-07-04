<h1 align="center">swarmx</h1>

<p align="center">
  <strong>Run your real <code>claude</code> / <code>codex</code> / <code>opencode</code> / <code>reasonix</code> / <code>zulu</code> CLIs as a collaborating swarm — plus a multi-model research committee &amp; code fusion — in one browser tab.</strong>
</p>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-MIT-blue.svg?style=for-the-badge" alt="MIT License"></a>
  <img src="https://img.shields.io/badge/Rust-1.83+-orange.svg?style=for-the-badge&logo=rust" alt="Rust 1.83+">
  <img src="https://img.shields.io/badge/Node-22+-5FA04E.svg?style=for-the-badge&logo=node.js&logoColor=white" alt="Node 22+">
  <img src="https://img.shields.io/badge/Desktop-Tauri-24C8DB.svg?style=for-the-badge&logo=tauri&logoColor=white" alt="Tauri">
  <a href="README.md"><img src="https://img.shields.io/badge/Lang-中文-red?style=for-the-badge" alt="中文"></a>
</p>

<p align="center">
  <img src="https://raw.githubusercontent.com/curdx/swarmx/main/docs/assets/hero-terminals.png" alt="swarmx dashboard — multiple real CLI agents running side by side" width="100%">
</p>

**swarmx** spawns the *real* subscription-mode coding CLIs you already have on
disk — the same `claude`, `codex`, `opencode`, `reasonix`, and `zulu` binaries —
each in its own PTY-backed terminal pane, and wires them into a team you steer by
talking to one **orchestrator** in plain language; it scales the team to the job.

It is **not** another LLM wrapper. Your OAuth, your rate limits, your plan
limits — everything behaves exactly as if you typed `claude` in your own
terminal, because that's literally what's running. swarmx never reads or
persists your tokens.

[Quick Start](#quick-start) · [Three modes](#three-modes) · [How it works](#how-it-works) · [Configuration](docs/configuration.md) · [中文](README.md)

---

## Three modes

One dashboard, the same real CLIs, three collaboration paradigms on demand:

### 🐝 Swarm collaboration (default)

Talk to a persistent **orchestrator** in plain language. It decides: do the work
itself, or spawn workers on demand via `swarm_spawn_worker` (the Magentic-One
model — no pre-declared topology). Members coordinate through a **shared inbox**
(`swarm_send_message`, delivered at the recipient's next turn boundary, zero
polling) and a **shared blackboard** (full-text search, versioned history, live
push on every write); write a blackboard key and every agent waiting on it revives
in the same tick — even one that already stopped idle.

<p align="center">
  <img src="https://raw.githubusercontent.com/curdx/swarmx/main/docs/assets/screenshot-orchestrator-chat.png" alt="Talking to the orchestrator in plain language" width="49%">
  <img src="https://raw.githubusercontent.com/curdx/swarmx/main/docs/assets/screenshot-dag.png" alt="Live dependency DAG of the swarm" width="49%">
</p>

### 🧠 Research committee (multi-model consult)

High-stakes decisions shouldn't rest on one model. The **research committee**
has N models answer the same question in parallel → a judge produces a
**structured comparison** (consensus / contradictions / unique insights / blind
spots — a *comparison*, not a vote) → an outer model **synthesizes** the final
answer. Tech selection, competitive analysis, design review, red-teaming a risky
call — one consult beats re-prompting a single model over and over.

<p align="center">
  <img src="https://raw.githubusercontent.com/curdx/swarmx/main/docs/assets/screenshot-consult.png" alt="Research committee — parallel answers, structured comparison, synthesized verdict" width="80%">
</p>

### ⚔️ Fusion (multi-model code competition, one-click autopilot)

Hand the **same need** to N models; each implements it independently in an
isolated git worktree, an optional **objective check** (e.g. `pytest`, `cargo
test`) is the hard gate, and a judge **synthesizes the best of all** back into
the main line. A novice types one need and clicks **一键开赛** (start) — pick
models, implement in parallel, run the gate, synthesize, merge — zero manual
steps. A **server-side watchdog** backstops the verdict: whether or not a model
finishes cleanly, the batch never silently stalls.

> 🔑 **The multi-model source for the committee & fusion = [Comate Zulu](https://www.npmjs.com/package/@comate/zulu)**:
> one Comate license drives a dozen-plus models (DeepSeek / GLM / Kimi / MiniMax…).
> In **Settings → Plugins**, **one-click install zulu** and paste your license —
> see [Quick Start](#quick-start).

## Why swarmx

Most "agent orchestration" tools either **reimplement the LLM client** (and lose
the subscription auth you paid for) or **wrap the CLI at the wrong layer** (ACP,
HTTP shims) that can't reuse your session. swarmx is the thinnest possible layer
that adds coordination without replacing anything:

- 🖥️ **Real CLIs, unmodified.** Each agent is the actual binary under `portable-pty`.
- 📬 **Shared inbox.** Agents address each other by id; delivery at the next turn boundary — zero polling.
- 📋 **Shared blackboard.** A markdown KV store with full-text search, versioned history, live push.
- 🧠 **One orchestrator, scaled to the task.** You chat with one persistent agent.
- ⏰ **Push-style wakeup.** Write a blackboard key and every waiter revives the same tick.
- 🧠 **Multi-model consult & fusion.** The committee + fusion turn "ask another model" into a structured synthesis.
- 🎬 **Everything recorded.** Each session is an in-browser asciicast.

## Quick Start

> **Prereqs:** Rust 1.83+, Node 22+, and at least one logged-in CLI (`claude`;
> `codex` must be **0.132+** for the auto-wake loop). `opencode` / `reasonix` optional.

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
and start talking to its orchestrator.

**Want the multi-model consult / fusion?** Open **Settings → Plugins**, hit
**One-click install** on the Comate Zulu row (the backend runs
`npm install -g @comate/zulu` and streams the log), then paste your **Comate
license** on the same page — one license, a dozen-plus models.

Prefer a desktop app? swarmx ships as a Tauri bundle (server, shim, MCP baked in
as sidecars) — download → install → open → use, zero terminal commands.

## How it works

Three layers, nothing more:

```
  MCP   ─►  swarm_send_message / swarm_write_blackboard / swarm_spawn_worker …
            (native tools the LLM calls; swarmx-mcp speaks stdio JSON-RPC)
  shim  ─►  swarmx-shim execvp's the real CLI, emits OSC ready/exit (~70 lines)
  PTY   ─►  the unmodified claude / codex / opencode / reasonix / zulu binary
```

A browser (or Tauri webview) opens a WebSocket per agent for the live terminal,
plus one `/ws/swarm` event stream. The Rust server (axum, loopback-only) owns
spawning, the inbox, the blackboard, recordings, and a `WakeCoordinator` that
turns blackboard writes into wakeups. Each engine's quirks (opencode's TUI,
reasonix/zulu's HTTP/SSE) are absorbed in per-CLI adapters.

## Docs

- 📦 **[Configuration reference](docs/configuration.md)** — every `SWARMX_*` variable, plugin/role/spell format, REST + WS API.
- 🤝 **[Handoff protocol](docs/handoff-protocol.md)** — blackboard-key conventions for producer/consumer contracts.
- 🧭 **[CLAUDE.md](CLAUDE.md)** — repo conventions + the packaging invariant + release checklist.
- 📝 **[CHANGELOG.md](CHANGELOG.md)** — notable changes per release.

## Security & Credentials

swarmx uses the **PTY-only credentials model** — the same as `tmux`, `ttyd`, and the CLIs themselves:

- ❌ Never reads `~/.claude/`, `~/.codex/`, etc.
- ❌ Never persists OAuth tokens, refresh tokens, or API keys.
- ✅ Passes `HOME` / `PATH` to the spawned CLI so it reads *its own* config.
- 🔑 The Comate license lives only in `~/.swarmx/comate.json`, used to drive zulu's models.

The server binds **only** to `127.0.0.1:7777` — no remote access, no auth. A
DNS-rebind defense and a credential-path denylist (`~/.ssh`, `*.pem`,
`~/.claude.json`, …) guard the file browser.

## Contributing

PRs and issues welcome. CI hard gates: `node scripts/harness-check.mjs`,
`cargo build/test --workspace --locked`, `web`'s `npm run build` (tsc), and an
isolated-backend `directions-smoke.mjs`. Swarm smoke tests needing real
logged-in CLIs are manual (`scripts/golden-cli-test.sh`).

```bash
bash scripts/test-stack.sh        # build + start on 7788/5188, data in /tmp
bash scripts/test-stack.sh stop   # tear down
```

## Star History

<a href="https://www.star-history.com/#curdx/swarmx&Date">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/svg?repos=curdx/swarmx&type=Date&theme=dark" />
    <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/svg?repos=curdx/swarmx&type=Date" />
    <img alt="Star History Chart" src="https://api.star-history.com/svg?repos=curdx/swarmx&type=Date" />
  </picture>
</a>

## Acknowledgments

Built on [portable-pty](https://docs.rs/portable-pty),
[asciinema-player](https://github.com/asciinema/asciinema-player),
[axum](https://github.com/tokio-rs/axum), [Tauri](https://tauri.app/), and
[xterm.js](https://xtermjs.org/). The orchestrator design follows the
**Magentic-One** "scale the team per task" model.

## License

[MIT](LICENSE).
