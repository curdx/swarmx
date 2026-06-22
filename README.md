# swarmx

<p align="center">
  <strong>A browser dashboard that spawns real <code>claude</code> / <code>codex</code> / <code>opencode</code> / <code>reasonix</code> CLIs under PTY, wires them into a swarm, and lets them message each other to finish a task.</strong>
</p>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-MIT-blue.svg" alt="MIT License"></a>
  <img src="https://img.shields.io/badge/Rust-1.83%2B-orange.svg" alt="Rust 1.83+">
  <img src="https://img.shields.io/badge/Node-22%2B-brightgreen.svg" alt="Node 22+">
  <img src="https://img.shields.io/badge/desktop-Tauri-9cf.svg" alt="Tauri">
  <a href="README.zh-CN.md"><img src="https://img.shields.io/badge/Lang-中文-red" alt="中文"></a>
</p>

swarmx runs **real subscription-mode CLIs** — the same `claude`, `codex`,
`opencode`, and `reasonix` binaries you already have on disk — inside a single
browser tab (or a Tauri desktop app). Each agent gets its own PTY-backed
terminal pane (xterm.js, WebGL-accelerated). A thin coordination layer on top
gives the agents capabilities they don't have standalone:

1. **A shared inbox.** Any agent can call `swarm_send_message` to address
   another agent by id; the recipient sees the message at its next turn
   boundary via a Stop-hook driven wake-check.
2. **A shared blackboard.** A markdown KV store with FTS5 full-text search,
   versioned history, and live `/ws/swarm` notifications when any agent
   edits it.
3. **A single point of contact — the orchestrator.** Each workspace boots one
   long-lived orchestrator agent. You talk to it in plain language; it decides
   per task whether to answer directly, do the work itself, or spawn workers
   via `swarm_spawn_worker`. The team scales to the task (the Magentic-One
   model) instead of a pre-declared topology.
4. **Push-style wakeup.** When a blackboard key a role `depends_on` is written,
   the server pushes a mailbox note **and** injects `\x15…\r` into the
   subscriber's PTY — so an agent that already stopped idle is revived the
   moment its upstream lands. No polling. No deadlocks.

The dashboard records every session as an asciicast v2 `.cast` file and plays
it back in-browser using the official `asciinema-player` (WASM-backed, full
keyboard controls).

swarmx **never reads or persists your OAuth tokens.** It passes `HOME` through
to the spawned CLI and lets each tool use the credentials you've already stored
in `~/.claude/`, `~/.codex/`, etc. — the same model `tmux` uses for session
credentials. See [Security & Credentials](#security--credentials).

---

## Table of Contents

- [Why swarmx](#why-swarmx)
- [Features](#features)
- [Quick Start](#quick-start)
- [Concepts](#concepts)
- [Architecture](#architecture)
- [Configuration reference](#configuration-reference)
- [REST & WebSocket API](#rest--websocket-api)
- [Security & Credentials](#security--credentials)
- [Packaging the desktop app](#packaging-the-desktop-app)
- [Troubleshooting / FAQ](#troubleshooting--faq)
- [Contributing](#contributing)
- [Acknowledgments](#acknowledgments)
- [License](#license)

---

## Why swarmx

Most "agent orchestration" projects either reimplement the LLM client from
scratch (losing the official CLIs' subscription auth that users actually paid
for) or wrap the CLIs at the wrong layer (e.g. ACP, which can't reuse the
subscription session). swarmx is intentionally the simplest possible layer that
adds coordination *without* replacing anything:

- **PTY at the bottom.** Each agent is the unmodified `claude` / `codex` /
  `opencode` / `reasonix` binary running under `portable-pty`. OAuth, rate
  limits, and plan limits all behave exactly as if you typed the command in
  your own terminal.
- **A thin shim in the middle.** `swarmx-shim` is ~70 lines of Rust that
  `execvp`s the real CLI and emits two OSC sequences (`ready` / `exit`). The
  CLIs don't know it's there.
- **MCP at the top.** Swarm messaging is exposed to the LLM as native tools via
  the stdio JSON-RPC MCP protocol — `swarmx-mcp` is a tiny binary every agent's
  CLI launches as a sub-process and talks to over stdio.

That's the whole abstraction. Everything else — the WebSocket bridge, the
recording pipeline, the wake-check, the spell loader — is built on top of these
three pieces and adds zero new requirements on the CLI side.

Engine quirks are absorbed in the server's per-CLI adapters
(`crates/swarmx-server/src/cli/{claude,codex,opencode,reasonix}.rs`):
opencode, when acting as a captain, drives a full-screen TUI via its official
`/tui` HTTP control interface; reasonix runs over `reasonix serve` HTTP/SSE
rather than a PTY. From the dashboard's perspective they all look like agents.

## Features

| | |
|---|---|
| **Real subscription CLIs** | Spawns the exact `claude` / `codex` / `opencode` / `reasonix` binaries on your `$PATH`. OAuth uses your existing `~/.claude/`, `~/.codex/`, … credentials — swarmx never reads or persists tokens. |
| **Multi-agent grid** | Spawn arbitrary numbers of agents; each gets its own pane with WebGL-accelerated xterm.js. A cooldown pool keeps the browser under its WebGL context cap and silently falls back to DOM for overflow. |
| **Orchestrator dispatch** | Each workspace boots one persistent orchestrator. It maintains a `task.ledger.md` + `progress.ledger.md` on the blackboard and dispatches workers on demand via `swarm_spawn_worker` (Magentic-One model — scale the team per task, no pre-declared topology). |
| **Swarm messaging** | `POST /api/message` or the in-CLI `swarm_send_message` tool delivers messages with `from`, `to`, `kind`, `body`, and an optional `in_reply_to` thread parent. All persisted to SQLite with FTS5. |
| **Shared blackboard** | Markdown files under `~/.swarmx/blackboard/` with FTS5 search, versioned history (each write is a row), and `/ws/swarm` push events on change. |
| **Turn-boundary wake-check** | Stop hooks invoke `swarmx-mcp wake-check`; if the agent has unread mail, the hook emits a `decision:block` continuation so the agent reads its inbox on the next turn — zero polling. opencode (no blocking Stop hook) gets an equivalent wake via a plugin. |
| **Push-style wake on blackboard write** | The `WakeCoordinator` subscribes to `SwarmEvent::BlackboardChanged`. When a key is written, every agent whose role declares `depends_on=["<key>"]` is woken in the same tick: a `kind="wake"` mailbox note lands **and** `\x15<msg>\r` is injected into the subscriber's PTY — restarting agents that already stopped idle. |
| **Directions (git worktrees)** | A workspace can fork an isolated *direction* into its own git worktree so parallel work doesn't collide; ledger keys are namespaced by `workspace_id` + direction slug. |
| **Session recording** | Every PTY session is recorded as asciicast v2 (`~/.swarmx/recordings/*.cast`) and replayed in-browser via `asciinema-player`. |
| **Desktop app** | Ships as a Tauri app bundling the server, shim, and MCP binaries as sidecars. Download → install → open → use, zero terminal commands. |

## Quick Start

### Prerequisites

| Tool | Version | Purpose |
|---|---|---|
| Rust | 1.83+ | Workspace toolchain (`rust-toolchain.toml` pins it) |
| Node | 22+ | Vite dev server / production build |
| `claude` | Any recent | Logged in via `claude` once (browser OAuth) |
| `codex` | 0.132+ | Logged in via `codex login`. **0.132 specifically** ships `--dangerously-bypass-hook-trust`, required for the wake-check loop to fire automatically. |
| `opencode` / `reasonix` | optional | Only needed if you want to spawn those engines. |

### Build & run (dev)

```bash
# clone
git clone https://github.com/curdx/swarmx.git
cd swarmx

# build everything in one shot (the server needs the shim binary present)
cargo build --workspace
cd web && npm install && cd ..

# terminal 1 — backend (run from the repo root so it finds spells/roles/cli-plugins)
cargo run -p swarmx-server      # listens on 127.0.0.1:7777

# terminal 2 — frontend (dev mode with hot reload)
cd web && npm run dev           # vite on 5173, proxies /api + /ws → 7777

# open the dashboard
open http://localhost:5173
```

For a production-style single-port deployment (axum serves the built bundle
itself), run `cd web && npm run build` and point your browser at
`http://127.0.0.1:7777` after the next `cargo run`.

### First spawn

1. Click **+ Claude Code** in the header. A new pane appears; if it's your
   first time, complete OAuth inside the embedded terminal exactly as you would
   running `claude` from your shell.
2. Click **+ Codex CLI**. First-time codex pops a `Hooks need review` dialog —
   swarmx's auto-answer kicks in within ~500 ms and you proceed straight to the
   prompt. (See the `auto-answered codex Hooks-need-review dialog` server log
   line.)
3. Type any prompt in either pane and confirm the agent talks back.

### Talk to your workspace's orchestrator

Create a workspace pointed at a real project directory. swarmx runs the
built-in `spells/init.md`, which spawns one **orchestrator** agent (claude) in
that directory. It scans your project (~30s), writes `task.ledger.md` +
`progress.ledger.md` to the blackboard, and greets you.

From then on you just talk to it in natural language. The orchestrator decides
per task whether to answer directly, do the work itself, or dispatch one or
more workers via `swarm_spawn_worker` — scaling the team to the task (the
Magentic-One model) instead of pre-allocating a fixed topology. Workers come
and go in the swarm drawer; the orchestrator stays.

> There is no "pick a spell from a dropdown" step. The earlier pre-declared
> multi-agent spells (`critic-loop` / `fullstack-feature*` / `auto-dispatch`)
> were removed in favour of this runtime-scaled dispatch. `spells/init.md` is
> the one spell that still ships; see `roles/orchestrator.md` for its prompt.
> The multi-agent machinery (`role_ref` / `allow_cycles` / `shared_workspace`)
> stays fully implemented and unit-tested for future use.

## Concepts

| Concept | One-line definition | Lives in |
|---|---|---|
| **Agent** | One subscription CLI process under PTY + shim + recorder. Identified by `<plugin>-<8hex>` (e.g. `claude-de332d7b`). | `swarmx-server::spawn`, `swarmx-pty` |
| **Plugin** | `cli-plugins/<id>.toml` declaring how to spawn one CLI: binary, default args, ready detector, MCP injection mode, hook flags. Bundled: `claude`, `codex`, `opencode`, `reasonix`. | `cli-plugins/`, `swarmx-server::plugins` |
| **Workspace** | A project the swarm operates on. Holds the orchestrator, ledgers, and any spawned workers. Per-agent CLI config overrides live under `~/.swarmx/`. | `swarmx-server::routes::workspaces` |
| **Direction** | An optional isolated branch of work inside a workspace, backed by its own git worktree so parallel directions don't clobber each other. | `swarmx-server::worktree` |
| **Orchestrator** | The single persistent agent per workspace. Runs the Magentic-One dual-ledger loop: scan → greet → dispatch / do / chat for the workspace's lifetime. | `roles/orchestrator.md`, `spells/init.md` |
| **Swarm message** | A row in `messages` (SQLite) addressed `from_agent → to_agent`, with optional `in_reply_to`. Sent via `POST /api/message` or the `swarm_send_message` MCP tool; broadcast on `/ws/swarm`. | `swarmx-swarm`, `swarmx-storage` |
| **Blackboard** | Markdown KV at `<root>/<path>.md` with full history. Read via `swarm_read_blackboard` / `GET /api/blackboard`; write via the inverse. notify-debouncer watches the FS for direct edits. | `swarmx-swarm::watcher`, `swarmx-storage` |
| **Wake-check** | `swarmx-mcp wake-check` subcommand. Reads stdin JSON from the Stop hook, resolves `agent_id`, queries unread count, emits `{decision:"block", reason:"…"}` when there's mail. Single-shot per Stop event — does NOT restart already-stopped agents (that's the WakeCoordinator's job). | `swarmx-mcp::wake_check` |
| **WakeCoordinator** | Roles declare blackboard keys via `depends_on`. On `BlackboardChanged{key}`, writes a `kind="wake"` mailbox note to every subscriber (excluding the writer) **and** injects `\x15<msg>\r` into their PTY. Cycle detection runs before any spawn. | `swarmx-server::wake` |
| **Spell** | `spells/<name>.md` with TOML front-matter declaring `[[agents]]`. Each block either inlines `role/cli/system_prompt` or sets `role_ref="<id>"` to inherit a `roles/<id>.md` template. `shared_workspace = true` flips spawn to one shared cwd. Only `init.md` ships today. | `spells/`, `swarmx-server::spells` |
| **Role** | `roles/<id>.md` — reusable SOP template referenced by spells. Carries `default_cli`, `artifact_paths`, `handoff_signal`, `depends_on`, and a `system_prompt_template` with `{task}` / `{<role>_id}` placeholders. Bundled: orchestrator, frontend, backend, reviewer, test-runner, docs-writer, researcher, fixer. | `roles/`, `swarmx-server::roles` |
| **Shim** | `swarmx-shim` — ~70-line binary that `execvp`s the real CLI and emits OSC `ready` / `exit` sequences so swarmx can detect lifecycle without polling. | `swarmx-shim` |
| **MCP** | `swarmx-mcp` — stdio JSON-RPC server exposing the `swarm_*` tools. Auto-installed in each agent's CLI config so the LLM can call them natively. Claude gets a per-agent `--mcp-config` file so shared-workspace agents don't clobber each other's identity. | `swarmx-mcp` |

## Architecture

```
┌─────────────────────────────────────────────────────────────────────┐
│ Browser / Tauri webview (Vite + React 18, xterm.js + WebGL pool)    │
│                                                                     │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────────────┐    │
│  │ Pane #1  │  │ Pane #2  │  │ Pane #N  │  │ swarm drawer +   │    │
│  │ xterm.js │  │ xterm.js │  │ xterm.js │  │ recordings +     │    │
│  └────┬─────┘  └────┬─────┘  └────┬─────┘  │ DAG / blackboard │    │
│       │             │             │        └────────┬─────────┘    │
└───────┼─────────────┼─────────────┼─────────────────┼──────────────┘
        │ /ws/pty/    │             │                 │ /ws/swarm
        │ <agent_id>  │             │                 │ + /api/*
        ▼             ▼             ▼                 ▼
┌─────────────────────────────────────────────────────────────────────┐
│ swarmx-server (axum, 127.0.0.1:7777, loopback only)                 │
│                                                                     │
│   /api/agent  /api/message  /api/blackboard  /api/recording         │
│   /api/spells /api/spell/run /api/plugins   /api/roles  /api/worker │
│                                                                     │
│   ┌─ AppState ────────────────────────────────────────────────┐    │
│   │ PluginRegistry · SpellRegistry · RoleRegistry · Registry  │    │
│   │ Store (SQLite) · Swarm · BlackboardWatcher · WakeCoord    │    │
│   └────────────────────────────────────────────────────────────┘   │
│   per-CLI adapters: cli/{claude,codex,opencode,reasonix}.rs         │
└──────────────┬──────────────────────────────────────────────────────┘
               │ stdin / stdout (PTY)
               ▼
┌─────────────────────────────────────────────────────────────────────┐
│ swarmx-shim (per agent, tiny Rust wrapper)                          │
│   - execvp("claude" | "codex" | "opencode" | "reasonix")            │
│   - emits OSC ready / exit sequences                                │
└──────────────┬──────────────────────────────────────────────────────┘
               │
               ▼
┌─────────────────────────────────────────────────────────────────────┐
│ Real CLI                                                            │
│   spawns ─►  swarmx-mcp (stdio)  ◄─►  /api/message etc              │
│             wake-check (Stop hook)                                  │
└─────────────────────────────────────────────────────────────────────┘
```

### Crate layout

| Crate | Purpose |
|---|---|
| `swarmx-protocol` | WebSocket frame schema, REST DTOs. Shared by server + clients. |
| `swarmx-shim` | The OSC-emitting wrapper that `execvp`s the real CLI (one per agent). harness-check verifies it lands in Tauri's `externalBin`. |
| `swarmx-pty` | `portable-pty` wrapper + 2-thread bridge + monotonic seq ring buffer. |
| `swarmx-server` | axum HTTP/WS gateway. Routes, lifecycle, pre-spawn patches, spell executor, role registry, `WakeCoordinator`, reaper, billing, engine-probe. Per-CLI adapters in `src/cli/{claude,codex,opencode,reasonix}.rs`. |
| `swarmx-swarm` | Per-agent inbox, blackboard CRUD, notify-debouncer FS watcher. |
| `swarmx-mcp` | Stdio JSON-RPC MCP server. Also hosts the `wake-check` subcommand invoked by Stop hooks. |
| `swarmx-storage` | SQLite + FTS5. Migrations, agents/messages/recordings/blackboard tables. |
| `swarmx-recorder` | asciicast v2 writer, finalize-on-EOF. |
| `swarmx-cli` | Thin `swarmx up` entry point (currently a stub — run `cargo run -p swarmx-server`). |

### Runtime resources (shipped, not crates)

These are compiled **into** the server binary via `include_str!`, so the
packaged app works with `CWD=/` and no env vars:

- `spells/init.md` — the one spell that ships (spawns the orchestrator).
- `roles/*.md` — 8 role SOP templates (including orchestrator).
- `cli-plugins/*.toml` — 4 engine manifests (claude / codex / opencode / reasonix).

The only resource that must be bundled as a file (it's JS executed by
opencode/node, can't be embedded) is `cli-plugins/opencode/swarmx-wake.js`,
shipped via Tauri `bundle.resources`.

## Configuration reference

Every `SWARMX_*` environment variable is documented in
[`docs/configuration.md`](docs/configuration.md) (a CI harness check guards its
completeness). Highlights:

| Variable | Purpose |
|---|---|
| `SWARMX_PORT` | Server port (default `7777`). |
| `SWARMX_DB_PATH` | SQLite path (default `~/.swarmx/swarmx.db`). |
| `SWARMX_MAX_LIVE_AGENTS` | Cap on concurrent live agents. |
| `SWARMX_RETENTION_DAYS` | Recording / activity retention window. |
| `SWARMX_SHIM_PATH` / `SWARMX_MCP_PATH` | Override bundled binary locations (set by the Tauri sidecar). |
| `SWARMX_{SPELLS,ROLES,CLI_PLUGINS}_DIR` | Optional on-disk overlays atop the compiled-in builtins. |

### `cli-plugins/<id>.toml`

```toml
id                       = "codex"          # used as `<id>-<8hex>` agent prefix
display_name             = "Codex CLI"
binary                   = "codex"          # resolved via $PATH
default_args             = ["--dangerously-bypass-approvals-and-sandbox"]
ready_detect             = "shim_osc"       # or "prompt_pattern" | "none"
mcp_inject               = "codex_global_toml"
home_env                 = "HOME"

# Each `auto_*` flag toggles one pre-spawn patch. All false = swarmx spawns
# the CLI naked; you'd then trust the workspace, install MCP, etc. by hand.
auto_inject_mcp          = true
auto_trust_workspace     = true   # write `[projects.<ws>] trust_level = "trusted"`
auto_dismiss_update      = true   # set dismissed_version = latest (codex only)
auto_inject_stop_hook    = true   # write workspace .codex/hooks.json Stop hook
auto_answer_hooks_dialog = true   # watch PTY for "Hooks need review" + send "2\r"
```

## REST & WebSocket API

### REST (loopback only)

| Method | Path | Purpose |
|---|---|---|
| `GET` | `/api/plugins` | List loaded CLI plugins. |
| `GET` | `/api/roles` | List loaded roles. |
| `GET` | `/api/spells` | List loaded spell manifests. |
| `POST` | `/api/spell/run` | Run a spell. Body: `{ name, task, workspace_dir? }`. |
| `POST` | `/api/worker` | Spawn a worker agent (used by `swarm_spawn_worker`). |
| `GET` `DELETE` | `/api/agent` · `/api/agent/:id` | List / kill agents. |
| `POST` | `/api/agent/:id/{interrupt,resume,wake}` | Lifecycle controls. |
| `POST` | `/api/message` *(via swarm tools)* | Send a swarm message. |
| `POST` | `/api/message/read` | Mark messages as read. |
| `GET` `PUT` | `/api/blackboard` | List / read / write blackboard files. |
| `GET` | `/api/recording` · `/api/recording/:id` | List / stream `.cast` files. |
| `GET` | `/api/usage` | Token / billing usage per agent. |
| `GET` | `/api/files/list` · `/api/files/read` | Sandboxed file browser (credential paths denylisted). |

### WebSocket

| Path | Purpose |
|---|---|
| `/ws/pty/:agent_id` | Bidirectional PTY bridge. Binary frames are `[4B BE seq][bytes…]`; text frames are control JSON (`resize`, `ack`, `hello`, `shim_ready`, `shim_exit`). |
| `/ws/swarm` | Server → client event stream: `agent_state`, `message`, `message_read`, `blackboard`, `shim_event`, `mcp_health`. |

## Security & Credentials

swarmx follows the **PTY-only credentials model**, the same model used by
`tmux`, `screen`, `ttyd`, and the official CLIs themselves:

- swarmx **never reads** files under `~/.claude/`, `~/.codex/`, etc.
- swarmx **never persists** OAuth tokens, refresh tokens, or API keys.
- swarmx **does** pass `HOME` (and `PATH`) to the spawned CLI so it can read
  *its own* config exactly as if you'd run it from your shell.

What swarmx does write (no credentials in any of it):

- Per-agent CLI config overrides under `~/.swarmx/` (MCP server entry, Stop
  hook config, workspace trust marker).
- Recordings at `~/.swarmx/recordings/*.cast` (terminal output bytes only).
- A SQLite DB at `~/.swarmx/swarmx.db` (agent metadata, messages, blackboard
  mirror, recording metadata).
- A small wake-check throttle file at `~/.swarmx/wake/<agent_id>.json`.

The server binds **only** to `127.0.0.1:7777`. There is no authentication
because there is no remote access — the same posture as `cargo run` or
`vite dev`. DNS-rebind defense: no-Origin requests also require a loopback
`Host`. The file browser hard-denies credential paths (`~/.ssh`, `~/.aws`,
`*.pem`/`*.key`, `~/.claude.json`, …) on every request.

## Packaging the desktop app

swarmx ships as a Tauri app that bundles the three server-side binaries
(`swarmx-server`, `swarmx-shim`, `swarmx-mcp`) as sidecars, so a user can
download → install → open → use with zero terminal commands.

```bash
cd web
npm run sidecar:release   # compile release backend + copy as Tauri sidecar
npm run tauri:build       # produce the real installer (.app / .dmg / …)
npm run tauri:dev         # Tauri dev (debug mode does NOT auto-start the backend)
```

> **Packaging invariant:** anything read at runtime must either be compiled in
> via `include_str!` / `sqlx::migrate!`, or shipped through Tauri
> `bundle.resources` with its absolute path injected via a `SWARMX_*` env var
> when the sidecar starts. The packaged app runs with `CWD=/` and no
> `SWARMX_*` env vars unless explicitly set, so relative-path lookups that work
> from the repo root will fail in the installed app. See `CLAUDE.md` for the
> full release checklist.

## Troubleshooting / FAQ

<details>
<summary><b>"My codex agent ignores swarm messages."</b></summary>

Check the codex version: `codex --version` must report **0.132 or higher**.
codex 0.132 ships `--dangerously-bypass-hook-trust`; earlier versions silently
refuse to fire swarmx's Stop hook. Fix with `brew upgrade codex` or
`npm install -g @openai/codex@latest`, then restart the server (swarmx probes
the flag once per process). Confirm via the server log:
`binary flag probe result … flag="--dangerously-bypass-hook-trust" supported=true`.
</details>

<details>
<summary><b>"codex pops a 'Hooks need review' dialog every time."</b></summary>

That's the normal codex 0.130+ trust gate. swarmx's `auto_answer_hooks_dialog`
flag (on by default in `cli-plugins/codex.toml`) arms a server-side watcher
that synthesizes `2 + Enter` within ~500 ms. If it doesn't auto-dismiss, check
the server log for `auto-answered codex Hooks-need-review dialog`; a missing
line usually means codex took longer than the watcher's window to start.
</details>

<details>
<summary><b>"claude says 'I don't have a swarm_send_message tool available'."</b></summary>

This happens when the agent's first turn fires before the MCP sub-process has
finished its handshake. swarmx already waits after `ShimReady` to mitigate
this; if you inject a prompt yourself immediately after `POST /api/agent`, add
the same delay.
</details>

<details>
<summary><b>"The recording drawer is empty even though I have agents running."</b></summary>

A recording is finalized only when the agent's PTY EOFs (the CLI exits). Active
recordings show as `● live` once they have any bytes flushed. If a row is
missing entirely, check `tail -f ~/.swarmx/recordings/*.cast` to see if the file
is growing.
</details>

## Contributing

PRs and issues are welcome. CI hard gates are: `node scripts/harness-check.mjs`
(cross-file invariant checks), `cargo build/test --workspace --locked`, the
`web` `npm run build` (tsc type-check), and an isolated-backend
`directions-smoke.mjs`. Swarm/resume/stress smoke tests that need real logged-in
CLIs are manual (`scripts/golden-cli-test.sh`).

When proposing a new CLI plugin, include a recorded OAuth verification
(asciicast or video) showing it works end-to-end on a fresh checkout.

Commit identity is set per-repo via local git config; commit messages are
written in **English**:

```bash
git config user.name  "your-name"
git config user.email "your@email"
# DO NOT modify global git config.
```

For an isolated full-stack instance to verify real UI changes without touching
a long-running dev session:

```bash
bash scripts/test-stack.sh        # build + start on ports 7788/5188, data in /tmp
bash scripts/test-stack.sh stop   # tear down
```

## Acknowledgments

swarmx stands on the shoulders of several open-source projects:

- **[portable-pty](https://docs.rs/portable-pty)** — the PTY abstraction every
  agent runs on.
- **[asciinema-player](https://github.com/asciinema/asciinema-player)** —
  in-browser recording playback, WASM-rendered with full keyboard controls.
- **[axum](https://github.com/tokio-rs/axum)** / **[Tauri](https://tauri.app/)**
  / **[xterm.js](https://xtermjs.org/)** — the server, desktop, and terminal
  layers.
- The **Magentic-One** orchestration model — the "scale the team per task"
  insight behind the orchestrator design.

## License

[MIT](LICENSE). See the file for the full text.
