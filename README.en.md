<h1 align="center">swarmx</h1>

<p align="center">
  Turn the real <code>claude</code> / <code>codex</code> / <code>opencode</code> / <code>reasonix</code> / <code>zulu</code> CLIs on your machine into a collaborating AI team — in one browser tab.
</p>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-MIT-blue.svg?style=for-the-badge" alt="MIT License"></a>
  <img src="https://img.shields.io/badge/Rust-1.83+-orange.svg?style=for-the-badge&logo=rust" alt="Rust 1.83+">
  <img src="https://img.shields.io/badge/Node-22+-5FA04E.svg?style=for-the-badge&logo=node.js&logoColor=white" alt="Node 22+">
  <img src="https://img.shields.io/badge/Desktop-Tauri-24C8DB.svg?style=for-the-badge&logo=tauri&logoColor=white" alt="Tauri">
  <a href="README.md"><img src="https://img.shields.io/badge/Lang-中文-red?style=for-the-badge" alt="中文"></a>
</p>

<p align="center">
  <img src="https://raw.githubusercontent.com/curdx/swarmx/main/docs/assets/hero-terminals.png" alt="swarmx dashboard: multiple real CLI agents running side by side" width="100%">
</p>

swarmx is a local browser dashboard. It spawns the coding CLIs you already have
installed and logged in — each agent *is* that binary — and gives them a
coordination layer: a shared inbox, a shared blackboard, and one orchestrator you
talk to. You describe what you want in plain language; it breaks the work down,
delegates, and reports back.

It runs the CLIs themselves, not a wrapper. So OAuth, rate limits, plan quotas —
all of it behaves exactly like typing `claude` in your own terminal. swarmx never
reads or stores your tokens.

## Three things it does

**Swarm collaboration.** Tell the orchestrator what you need; it decides whether
to do it itself or split it across a few workers (the Magentic-One approach — no
pre-declared topology, spawned per task). Members address each other through the
inbox (messages land at the recipient's next turn boundary — no polling) and share
state on the blackboard; write a blackboard key and every agent waiting on it
wakes up in the same tick, including ones that had gone idle.

<p align="center">
  <img src="https://raw.githubusercontent.com/curdx/swarmx/main/docs/assets/screenshot-orchestrator-chat.png" alt="Talking to the orchestrator in plain language" width="49%">
  <img src="https://raw.githubusercontent.com/curdx/swarmx/main/docs/assets/screenshot-dag.png" alt="Live dependency DAG of the swarm" width="49%">
</p>

**Research committee.** Don't rest an important call on one model. Have several
models answer the same question in parallel; a judge breaks their answers into
consensus, contradictions, unique points, and blind spots (a comparison, not a
vote); an outer model then synthesizes a final answer. Tech selection, design
review, red-teaming a risky decision — all better than re-prompting one model.

<p align="center">
  <img src="https://raw.githubusercontent.com/curdx/swarmx/main/docs/assets/screenshot-consult.png" alt="Research committee: parallel answers, structured comparison, synthesized verdict" width="80%">
</p>

**Fusion.** Hand the same need to a few models; each writes its own version in an
isolated git worktree. Attach an objective check (e.g. `pytest`, `cargo test`) as
a hard gate, and a judge synthesizes the best of them back into the main line.
Type one need, tick "autopilot", click once — model selection, parallel
implementation, the check, synthesis, and merge all run on their own. A
server-side watchdog backstops the verdict, so a model that doesn't finish cleanly
can't leave it stuck.

The multi-model source for the committee and fusion is
[Comate Zulu](https://www.npmjs.com/package/@comate/zulu) — one license reaches a
dozen-plus models. Install zulu from **Settings → Plugins** in one click and paste
your license.

## Quick Start

Prereqs: Rust 1.83+, Node 22+, and at least one logged-in CLI (`claude`; `codex`
must be 0.132+ for the auto-wake loop). opencode / reasonix are optional.

```bash
git clone https://github.com/curdx/swarmx.git
cd swarmx

# build everything (the server needs the shim binary present)
cargo build --workspace
cd web && npm install && cd ..

# terminal 1: backend (run from repo root)
cargo run -p swarmx-server          # → 127.0.0.1:7777

# terminal 2: frontend
cd web && npm run dev               # → http://localhost:5173
```

Open http://localhost:5173, point a workspace at a real project directory, and
start talking to its orchestrator.

For the committee or fusion, install Comate Zulu from Settings → Plugins and paste
your license — one license, a dozen-plus models.

Packaged as a Tauri app, the server / shim / mcp binaries ride along as sidecars:
download, install, open, use — no terminal.

## How it works

Three layers, nothing else:

```
  MCP   ─►  swarm_send_message / swarm_write_blackboard / swarm_spawn_worker …
            (native tools the LLM calls; swarmx-mcp speaks stdio JSON-RPC)
  shim  ─►  swarmx-shim execvp's the real CLI, emits OSC ready/exit (~70 lines)
  PTY   ─►  the unmodified claude / codex / opencode / reasonix / zulu binary
```

The browser opens a WebSocket per agent for the live terminal, plus one
`/ws/swarm` event stream. The Rust server (axum, loopback-only) owns process
spawning, the inbox, the blackboard, recordings, and the scheduler that turns
blackboard writes into wakeups. Each engine's quirks — opencode's TUI, reasonix /
zulu's HTTP/SSE — are absorbed in per-CLI adapters.

## Docs

- [Configuration reference](docs/configuration.md): every `SWARMX_*` variable, plugin / role / spell format, REST + WS API.
- [Handoff protocol](docs/handoff-protocol.md): blackboard-key conventions for producer / consumer contracts.
- [CLAUDE.md](CLAUDE.md): repo conventions, the packaging invariant, release checklist.
- [CHANGELOG.md](CHANGELOG.md): notable changes per release.

Runtime resources (`spells/init.md`, `roles/*.md`, `cli-plugins/*.toml`) are
compiled into the server binary via `include_str!`, so the packaged app runs with
`CWD=/` and no env vars.

## Security

The same PTY-only credentials model as `tmux`, `ttyd`, and the CLIs themselves: it
doesn't read `~/.claude/` or `~/.codex/`, doesn't store OAuth tokens or API keys,
and just passes `HOME` / `PATH` to the child CLI so it reads its own config. The
Comate license lives only in `~/.swarmx/comate.json`.

The server binds only to `127.0.0.1:7777` — no remote access, no auth, the same
posture as `cargo run`. The file browser has DNS-rebind defense and a
credential-path denylist (`~/.ssh`, `*.pem`, `~/.claude.json`, …).

## Contributing

CI hard gates: `node scripts/harness-check.mjs`, `cargo build/test --workspace
--locked`, `web`'s `npm run build`, and an isolated-backend `directions-smoke.mjs`.
Swarm smoke tests needing real logged-in CLIs are manual (`scripts/golden-cli-test.sh`).

To verify UI changes without touching your dev session, spin up an isolated stack:

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

## License

[MIT](LICENSE).
