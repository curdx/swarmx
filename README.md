# flockmux-core

<p align="center">
  <strong>A browser dashboard that spawns real <code>claude</code> &amp; <code>codex</code> CLIs under PTY, wires them into a swarm, and lets them message each other to finish a task.</strong>
</p>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-MIT-blue.svg" alt="MIT License"></a>
  <img src="https://img.shields.io/badge/Rust-1.83%2B-orange.svg" alt="Rust 1.83+">
  <img src="https://img.shields.io/badge/Node-22%2B-brightgreen.svg" alt="Node 22+">
  <img src="https://img.shields.io/badge/status-MVP%20done%20(M1–M5)-success" alt="status">
  <a href="README.zh-CN.md"><img src="https://img.shields.io/badge/Lang-中文-red" alt="中文"></a>
</p>

flockmux runs **real subscription-mode CLIs** — the same `claude` and `codex`
binaries you have on disk — inside a single browser tab. Each agent gets its
own PTY-backed terminal pane (xterm.js, WebGL-accelerated). A coordination
layer on top gives the agents three new capabilities they don't have
standalone:

1. **A shared inbox.** Any agent can call `swarm_send_message` to address
   another agent by id; the recipient sees the message at its next turn
   boundary via a Stop-hook driven wake-check (no polling, no PTY injection).
2. **A shared blackboard.** A markdown KV store with FTS5 full-text search,
   versioned history, and live `/ws/swarm` notifications when any agent
   edits it.
3. **Spells.** One-file declarative orchestration templates that spawn N
   agents with role-specific system prompts and a topology. The bundled
   `critic-loop` runs writer → critic → editor in three CLI invocations,
   the editor reading both the original draft and the critique notes via
   the swarm bus before publishing the final version to `system`.

The dashboard also records every session as an asciicast v2 `.cast` file and
plays it back in-browser using the official `asciinema-player` (WASM-backed,
full keyboard controls).

flockmux never reads or persists your OAuth tokens. It passes `HOME` through
to the spawned CLI and lets claude/codex use the credentials you've already
stored in `~/.claude/` / `~/.codex/`. This is the same model `tmux` uses for
session credentials — see [Security &amp; Credentials](#security--credentials).

---

## Table of Contents

- [Why flockmux](#why-flockmux)
- [Features](#features)
- [Screenshots](#screenshots)
- [Quick Start](#quick-start)
- [Concepts](#concepts)
- [Walkthrough: critic-loop in 60 seconds](#walkthrough-critic-loop-in-60-seconds)
- [Architecture](#architecture)
- [Configuration reference](#configuration-reference)
- [REST &amp; WebSocket API](#rest--websocket-api)
- [Security &amp; Credentials](#security--credentials)
- [Troubleshooting / FAQ](#troubleshooting--faq)
- [Roadmap](#roadmap)
- [Acknowledgments](#acknowledgments)
- [Contributing](#contributing)
- [License](#license)

---

## Why flockmux

Most "agent orchestration" projects either reimplement the LLM client from
scratch (losing the official CLIs' rough edges that subscribers actually
paid for) or wrap the CLIs at the wrong layer (e.g. ACP, which can't reuse
the subscription auth). flockmux is intentionally the simplest possible
layer that adds coordination *without* replacing anything:

- **PTY at the bottom.** Each agent is the unmodified `claude` / `codex`
  binary running under `portable-pty`. OAuth, rate limits, plan limits all
  behave exactly as if you typed `claude` in your terminal.
- **MCP at the top.** Swarm messaging is exposed to the LLM as native tools
  via the stdio JSON-RPC MCP protocol — `flockmux-mcp` is a tiny binary
  every agent's CLI launches as a sub-process and talks to over stdio.
- **A thin shim in the middle.** `flockmux-shim` is ~70 lines of Rust that
  `execvp`s the real CLI and emits two OSC sequences (`ready` / `exit`).
  The CLIs don't know it's there.

That's the whole abstraction. Everything else — the WebSocket bridge, the
recording pipeline, the wake-check, the spell loader — is built on top of
these three pieces and adds zero new requirements on the CLI side.

## Features

| | |
|---|---|
| **Real subscription CLIs** | Spawns the exact `claude` and `codex` binaries you have on `$PATH`. OAuth uses your existing `~/.claude/` / `~/.codex/` credentials — flockmux never reads or persists tokens. |
| **Multi-agent grid** | Spawn arbitrary numbers of agents; each gets its own pane with WebGL-accelerated xterm.js. A cooldown pool keeps the browser under its WebGL context cap and silently falls back to DOM for overflow. |
| **Swarm messaging** | `POST /api/message` or the in-CLI `swarm_send_message` tool delivers messages with `from`, `to`, `kind`, `body`, and an optional `in_reply_to` thread parent. All persisted to SQLite with FTS5. |
| **Shared blackboard** | Markdown files under `~/.flockmux/blackboard/` with FTS5 search, versioned history (each write is a row), and `/ws/swarm` push events on change. |
| **Turn-boundary wake-check** | Both claude and codex Stop hooks invoke `flockmux-mcp wake-check`; if the agent has unread mail, the hook emits a `decision:block` continuation so the agent reads its inbox on the next turn — zero polling, zero PTY injection. |
| **Codex first-run dialog auto-confirm** | codex 0.130+ pops a "Hooks need review" trust dialog the first time it sees a new hook path. flockmux's server watches PTY output and synthesizes the `2 + Enter` keystrokes, so spawn is one click for the user. |
| **Asciicast v2 record + browser replay** | Every session writes a `.cast` file; the recordings drawer plays them inline with the official `asciinema-player` (WASM renderer, fullscreen + scrubbing). |
| **Spells** | TOML front-matter + markdown body declares a multi-agent topology (`[[agents]] role / cli / system_prompt`). `POST /api/spell/run` spawns all of them, substitutes `{task}` and `{<role>_id}` placeholders, and injects each agent's bootstrap prompt. Bundled: `critic-loop`. |
| **Local-first** | Binds `127.0.0.1:7777` only. No authentication (single-user). No network egress beyond what the CLIs themselves make to their providers. |

## Screenshots

> _Screenshots/asciicast GIFs land here. Until then, see
> [docs/walkthrough.md](docs/walkthrough.md) (TODO) or run the
> [walkthrough below](#walkthrough-critic-loop-in-60-seconds) yourself._

## Quick Start

### Prerequisites

| Tool | Version | Purpose |
|---|---|---|
| Rust | 1.83+ | Workspace toolchain (`rust-toolchain.toml` pins it) |
| Node | 22+ | Vite dev server / production build |
| `claude` | Any recent | Logged-in via `claude` once (browser OAuth) |
| `codex` | 0.132+ | Logged-in via `codex login`. **0.132 specifically** ships `--dangerously-bypass-hook-trust`, which is required for the wake-check loop to fire automatically. |

### Build &amp; run

```bash
# clone
git clone https://github.com/curdx/flockmux-core.git
cd flockmux-core

# build everything in one shot
cargo build --workspace
cd web && npm install && cd ..

# terminal 1 — backend
cargo run -p flockmux-server      # listens on 127.0.0.1:7777

# terminal 2 — frontend (dev mode with hot reload)
cd web && npm run dev             # vite on 5173, proxies /api + /ws → 7777

# open the dashboard
open http://localhost:5173
```

For a production-style single-port deployment (axum serves the built bundle
itself), run `cd web && npm run build` and point your browser at
`http://127.0.0.1:7777` after the next `cargo run`.

### First spawn

1. Click **+ Claude Code** in the header. A new pane appears; if it's your
   first time, complete OAuth inside the embedded terminal exactly as you
   would running `claude` from your shell.
2. Click **+ Codex CLI**. First-time codex will pop a `Hooks need review`
   dialog — flockmux's auto-answer kicks in within ~500 ms and you proceed
   straight to the prompt. (See the `auto-answered codex Hooks-need-review
   dialog` log line in the server.)
3. Type any prompt in either pane and confirm the agent talks back.

### Wire the swarm

In the **messages** drawer on the right:

1. Pick an agent id under **to**.
2. Type "what is your favorite color, briefly?" under **body**.
3. Click **send**.
4. Type any prompt (e.g. `say hi`) in that agent's pane.
5. Watch: after the agent finishes the `say hi` turn, its Stop hook fires
   `flockmux-mcp wake-check`, sees `unread=1`, and continues the agent into
   another turn that calls `swarm_list_messages` and replies via
   `swarm_send_message`. The reply appears in the messages drawer with the
   correct `in_reply_to` parent link.

### Run a spell

In the header dropdown, pick **✨ critic-loop**, type a task description
(e.g. `haiku about Rust async cancellation`), hit **run**. Three agents
spawn — writer, critic, editor — and you watch them hand the work off
through the messages drawer. The final piece comes back addressed to
`system` with `kind: "reply"` and an `in_reply_to` linking back to the
critic's notes.

## Concepts

| Concept | One-line definition | Lives in |
|---|---|---|
| **Agent** | One subscription CLI process under PTY + shim + recorder. Identified by `<plugin>-<8hex>` (e.g. `claude-de332d7b`). | `flockmux-server::spawn`, `flockmux-pty` |
| **Plugin** | `cli-plugins/<id>.toml` declaring how to spawn one CLI: binary, default args, ready detector, MCP injection mode, hook installation flags. | `cli-plugins/`, `flockmux-server::plugins` |
| **Workspace** | Per-agent scratch directory at `~/.flockmux/workspaces/<agent_id>/` containing the CLI's `.claude/` or `.codex/` config overrides. Pre-spawn patches make this look like a trusted, pre-configured project to the CLI. | `flockmux-server::pre_spawn` |
| **Swarm message** | A row in `messages` (SQLite) addressed `from_agent → to_agent`, with optional `in_reply_to`. Sent via `POST /api/message` or `swarm_send_message` MCP tool; broadcast on `/ws/swarm`. | `flockmux-swarm`, `flockmux-storage` |
| **Blackboard** | Markdown KV at `<root>/<path>.md` with full history. Read via `swarm_read_blackboard` / `GET /api/blackboard/...`; write via the inverse. notify-debouncer watches the FS for direct edits. | `flockmux-swarm::watcher`, `flockmux-storage` |
| **Wake-check** | `flockmux-mcp wake-check` subcommand. Reads stdin JSON from Stop hook, derives `agent_id` from the `cwd` field, queries `/api/message/unread_count`, emits `{decision:"block", reason:"..."}` when there's mail. Throttle file at `~/.flockmux/wake/<id>.json` caps wakes per window. | `flockmux-mcp::wake_check` |
| **Spell** | `spells/<name>.md` with TOML front-matter declaring `[[agents]]` (role + cli + system_prompt). Run via `POST /api/spell/run {name, task}`; placeholders `{task}` and `{<role>_id}` are substituted before each prompt is PTY-injected. | `spells/`, `flockmux-server::spells` |
| **Shim** | `flockmux-shim` — ~70-line binary that `execvp`s the real CLI and emits OSC `ready` / `exit` sequences so flockmux can detect lifecycle without polling. | `flockmux-shim` |
| **MCP** | `flockmux-mcp` — stdio JSON-RPC server exposing `swarm_send_message`, `swarm_list_messages`, blackboard tools. Auto-installed in each agent's CLI config so the LLM can call them as native tools. | `flockmux-mcp` |

## Walkthrough: critic-loop in 60 seconds

```bash
# 1. Start the stack
cargo run -p flockmux-server &
cd web && npm run dev &

# 2. Fire the spell directly via REST (the UI does the same call)
curl -sX POST http://127.0.0.1:7777/api/spell/run \
  -H 'content-type: application/json' \
  -d '{
        "name": "critic-loop",
        "task": "haiku about Rust async cancellation"
      }' | jq .

# Response:
# {
#   "spell": "critic-loop",
#   "agents": [
#     { "role": "writer", "cli": "claude", "agent_id": "claude-890b3c93" },
#     { "role": "critic", "cli": "codex",  "agent_id": "codex-5796ef7c" },
#     { "role": "editor", "cli": "claude", "agent_id": "claude-c46442a7" }
#   ]
# }

# 3. Watch the message bus
curl -sN http://127.0.0.1:7777/api/message | jq '.[-3:]'
# Three messages will appear:
#   #7  writer → critic   (draft haiku)
#   #8  critic → editor   (draft + critique notes)            in_reply_to=#7
#   #9  editor → system   (final revised haiku, kind="reply") in_reply_to=#8
```

The whole loop takes ~3.5 minutes wall-clock on a warm cache. Each
hand-off is the agent's Stop hook firing `wake-check`, seeing the unread
message from upstream, and continuing into a `swarm_list_messages` →
`swarm_send_message` pair. No polling, no PTY injection beyond the
initial system-prompt bootstrap.

## Architecture

```
┌─────────────────────────────────────────────────────────────────────┐
│ Browser (vite-served Vite + React 18, xterm.js + WebGL pool)        │
│                                                                     │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────────────┐    │
│  │ Pane #1  │  │ Pane #2  │  │ Pane #N  │  │ swarm drawer +   │    │
│  │ xterm.js │  │ xterm.js │  │ xterm.js │  │ recordings +     │    │
│  └────┬─────┘  └────┬─────┘  └────┬─────┘  │ spells launcher  │    │
│       │             │             │        └────────┬─────────┘    │
└───────┼─────────────┼─────────────┼─────────────────┼──────────────┘
        │ /ws/pty/    │             │                 │ /ws/swarm
        │ <agent_id>  │             │                 │ + /api/*
        ▼             ▼             ▼                 ▼
┌─────────────────────────────────────────────────────────────────────┐
│ flockmux-server (axum, 127.0.0.1:7777, loopback only)               │
│                                                                     │
│   /api/agent    /api/message    /api/blackboard    /api/recording   │
│   /api/spells   /api/spell/run  /api/plugins                        │
│                                                                     │
│   ┌─ AppState ────────────────────────────────────────────────┐    │
│   │ PluginRegistry · SpellRegistry · Registry (live PTY slots)│    │
│   │ Store (SQLite)  · Swarm · BlackboardWatcher               │    │
│   └────────────────────────────────────────────────────────────┘    │
└──────────────┬──────────────────────────────────────────────────────┘
               │ stdin / stdout (PTY)
               ▼
┌─────────────────────────────────────────────────────────────────────┐
│ flockmux-shim (per agent, tiny Rust wrapper)                        │
│   - execvp("claude" | "codex" ...)                                  │
│   - emits OSC ready / exit sequences                                │
└──────────────┬──────────────────────────────────────────────────────┘
               │
               ▼
┌─────────────────────────────────────────────────────────────────────┐
│ Real CLI (claude / codex 0.132+)                                    │
│                                                                     │
│   spawns ─►  flockmux-mcp (stdio)  ◄─►  /api/message etc            │
│              wake-check (Stop hook)                                 │
└─────────────────────────────────────────────────────────────────────┘
```

### Crate layout

| Crate | Lines | Purpose |
|---|---|---|
| `flockmux-protocol` | ~250 | WebSocket frame schema, REST DTOs. Shared by server + clients. |
| `flockmux-shim` | ~70 | The OSC-emitting wrapper that `execvp`s the real CLI. |
| `flockmux-pty` | ~300 | `portable-pty` wrapper + 2-thread bridge + monotonic seq ring buffer. |
| `flockmux-server` | ~2500 | axum HTTP/WS gateway. Routes, lifecycle, pre-spawn patches, dialog auto-answer, spell executor. |
| `flockmux-swarm` | ~600 | Per-agent inbox, blackboard CRUD, notify-debouncer watcher. |
| `flockmux-mcp` | ~700 | Stdio JSON-RPC MCP server. Also hosts the `wake-check` subcommand invoked by Stop hooks. |
| `flockmux-storage` | ~800 | SQLite + FTS5. Migrations, agents/messages/recordings/blackboard tables. |
| `flockmux-recorder` | ~250 | asciicast v2 writer, finalize-on-EOF. |
| `flockmux-cli` | ~50 | Thin entry point (`flockmux up` launches server + opens dashboard). |
| `cli-plugins/` | — | Per-CLI `.toml`: `claude.toml`, `codex.toml`. |
| `spells/` | — | Per-spell `.md`: `critic-loop.md`. |
| `web/` | — | Vite + React + xterm.js + asciinema-player frontend. |

### Data flow at a glance

```
1.  User clicks "+ Codex CLI" in the browser.
2.  POST /api/agent { cli: "codex" }
3.  Server: PluginRegistry.get("codex") → CliPlugin
            spawn::spawn_agent() forks flockmux-shim → execs codex
            pre_spawn::run_codex_patches writes
              <workspace>/.codex/config.toml  (mcp_servers.flockmux-swarm)
              <workspace>/.codex/hooks.json   (Stop hook → wake-check)
            DialogAutoAnswer arms a 30s watcher for "Hooks need review"
            Recording opens .cast file under recordings_root
4.  PTY pump scans bytes for OSC_READY → broadcasts ShimReady
            Recorder appends each chunk asciicast-v2 framed
            Registry stores AgentSlot (bridge, input_tx, lifecycle_tx)
5.  Browser opens /ws/pty/codex-XXXX → bidirectional binary stream
6.  Browser also opens /ws/swarm → receives agent_state + message events
7.  Codex starts; flockmux-mcp launches as a sub-process for tool use
8.  Each turn end: codex Stop hook → flockmux-mcp wake-check → REST
    /api/message/unread_count → if >0, emit {decision:block, reason:...}
    → codex continues into another turn that reads & responds.
```

## Configuration reference

### `cli-plugins/<id>.toml`

```toml
id                       = "codex"          # used as `<id>-<8hex>` agent prefix
display_name             = "Codex CLI"
binary                   = "codex"          # resolved via $PATH
default_args             = ["--dangerously-bypass-approvals-and-sandbox"]
ready_detect             = "shim_osc"       # or "prompt_pattern" | "none"
mcp_inject               = "codex_global_toml"
home_env                 = "HOME"

# Each `auto_*` flag toggles one pre-spawn patch. Setting them all to
# false means flockmux just spawns the CLI naked; you'd then have to
# trust the workspace, install MCP, etc. by hand.
auto_inject_mcp          = true
auto_trust_workspace     = true   # write `[projects.<ws>] trust_level = "trusted"`
auto_dismiss_update      = true   # set dismissed_version = latest (codex only)
auto_inject_stop_hook    = true   # write workspace .codex/hooks.json Stop hook
auto_answer_hooks_dialog = true   # watch PTY for "Hooks need review" + send "2\r"
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
You are the WRITER. Task: {task}
Hand off to critic={critic_id}, editor={editor_id} via swarm_send_message.
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

# Free-form markdown body (documentation, ignored by the parser).
```

Substitution rules at run time:
- `{task}` → the task string from `POST /api/spell/run`.
- `{<role>_id}` → the actual `agent_id` allocated for that role (e.g.
  `{writer_id}` becomes `claude-890b3c93`).
- Unknown `{…}` placeholders are left literal (deliberately — silent drops
  hide spell-author bugs).

## REST &amp; WebSocket API

### REST (loopback only)

| Method | Path | Purpose |
|---|---|---|
| `GET` | `/api/plugins` | List loaded CLI plugins. |
| `POST` | `/api/agent` | Spawn one agent. Body: `{ cli, role?, workspace? }`. |
| `GET` | `/api/agent` | List live + historical agents. |
| `DELETE` | `/api/agent/:id` | Kill an agent. |
| `GET` | `/api/message` | List messages, with optional `from` / `to` / `since` filters. |
| `POST` | `/api/message` | Send a swarm message. |
| `POST` | `/api/message/read` | Mark messages as read. |
| `GET` | `/api/message/unread_count` | Query unread count for an agent (used by wake-check). |
| `GET` | `/api/blackboard` | List blackboard files. |
| `GET` | `/api/blackboard/*path` | Read a blackboard file. |
| `PUT` | `/api/blackboard/*path` | Write a blackboard file. |
| `GET` | `/api/blackboard-history/*path` | Versioned history of a blackboard path. |
| `GET` | `/api/recording` | List recordings. |
| `GET` | `/api/recording/:id` | Stream the raw `.cast` file. |
| `GET` | `/api/spells` | List loaded spell manifests. |
| `POST` | `/api/spell/run` | Run a spell. Body: `{ name, task }`. |

### WebSocket

| Path | Purpose |
|---|---|
| `/ws/pty/:agent_id` | Bidirectional PTY bridge. Binary frames are `[4B BE seq][bytes…]`; text frames are control JSON (`resize`, `ack`, `hello`, `shim_ready`, `shim_exit`). |
| `/ws/swarm` | Server → client event stream: `agent_state`, `message`, `message_read`, `blackboard`, `shim_event`, `mcp_health`. |

## Security &amp; Credentials

flockmux follows the **PTY-only credentials model**, the same model used by
`tmux`, `screen`, `ttyd`, and the official `claude` &amp; `codex` CLIs themselves:

- flockmux **never reads** files under `~/.claude/` or `~/.codex/`.
- flockmux **never persists** OAuth tokens, refresh tokens, API keys.
- flockmux **does** pass `HOME` to the spawned CLI process so it can read
  *its own* config exactly as if you'd run it from your shell. The PATH
  env is also forwarded so the CLI can find its own subcommands.

What flockmux does write (with the user's explicit consent every time, by
running it):

- A workspace dir at `~/.flockmux/workspaces/<agent_id>/` containing the
  CLI's per-project config overrides (MCP server entry, Stop hook config,
  workspace trust marker). Nothing in here contains credentials.
- Recordings at `~/.flockmux/recordings/*.cast` (terminal output bytes, no
  keystrokes, no env, no credentials).
- A SQLite DB at `~/.flockmux/flockmux.db` (agent metadata, messages,
  blackboard mirror, recording metadata).
- A small wake-check throttle file at `~/.flockmux/wake/<agent_id>.json`
  (epoch ms + counter).

The server binds **only** to `127.0.0.1:7777`. There is no authentication
because there's no remote access — this is the same posture as `cargo run`
or `vite dev`. For multi-machine / remote-access plans, see the
[Roadmap](#roadmap).

## Troubleshooting / FAQ

<details>
<summary><b>"My codex agent ignores swarm messages."</b></summary>

Check the codex version: `codex --version` must report **0.132 or higher**.
codex 0.132 ships `--dangerously-bypass-hook-trust`; earlier versions
silently refuse to fire flockmux's Stop hook. The fix is `brew upgrade
codex` or `npm install -g @openai/codex@latest`, then restart the server
(flockmux probes the flag once per process).

You can confirm the probe ran by grepping the server log for `binary flag
probe result … flag="--dangerously-bypass-hook-trust" supported=true`.
</details>

<details>
<summary><b>"codex pops a 'Hooks need review' dialog every time."</b></summary>

That's the normal codex 0.130+ trust gate. flockmux's
`auto_answer_hooks_dialog` flag (on by default in `cli-plugins/codex.toml`)
arms a server-side watcher that synthesizes `2 + Enter` within ~500 ms.
If you don't see the dialog auto-dismiss, check the server log for
`auto-answered codex Hooks-need-review dialog`. If that log is missing,
the watcher's 30 s window expired before the dialog appeared — usually
because codex took longer than expected to start. Increase the `WINDOW`
constant in `spawn::DialogAutoAnswer` and rebuild.
</details>

<details>
<summary><b>"claude says 'I don't have a swarm_send_message tool available'."</b></summary>

This happens when the agent's first turn fires before the MCP sub-process
has finished its handshake. flockmux's spell executor already waits
2.5 s after `ShimReady` to mitigate this; if you're invoking
`POST /api/agent` and immediately injecting a prompt yourself, do the
same.
</details>

<details>
<summary><b>"The recording drawer is empty even though I have agents running."</b></summary>

The recording is finalized only when the agent's PTY EOFs (i.e. the CLI
exits). Active recordings show as `● live` in the drawer once they have
any bytes flushed. If a recording row is missing entirely, check
`tail -f ~/.flockmux/recordings/*.cast` to see if the file is growing.
</details>

<details>
<summary><b>"Browser shows 'WS closed (code 1005)' on a pane I just had open."</b></summary>

That pane's PTY exited (the underlying CLI crashed or exited cleanly).
The XtermPane component will display the exit code in its status bar.
This is informational, not an error of flockmux itself.
</details>

## Roadmap

### Done (M1 – M5)

- ✅ **M1** Single-agent PTY + OAuth + WebSocket bridge + WebGL pool
- ✅ **M2** Multi-CLI (claude + codex) + GridView + WebGL cooldown
- ✅ **M3** Swarm L2: per-agent inbox, blackboard, asciicast recording
- ✅ **M4** Swarm L3: `flockmux-mcp` exposing `swarm_send_message` /
            `swarm_list_messages` / blackboard tools
- ✅ **M5a** Observability: `read_at`, `in_reply_to`, blackboard history
- ✅ **M5b** Turn-boundary wake-check (claude + codex 0.132)
- ✅ **M5c** Spells (`critic-loop`) + in-browser asciicast playback

### Backlog (post-MVP, all from plan §13)

| Priority | Item | Effort |
|---|---|---|
| P1 | `cli-plugins/gemini.toml` (Google Gemini CLI) | One toml file + manual auth verification |
| P1 | `cli-plugins/qwen.toml` (Alibaba Qwen CLI) | Same as gemini; `ready_detect = "prompt_pattern"` |
| P1 | `spells/tree-executor.md` (recursive task decomposition) | One md file |
| P1 | `spells/map-reduce.md` (parallel workers + reducer) | One md file |
| P2 | `cli-plugins/opencode.toml`, `cli-plugins/aider.toml` | Per-CLI auth research |
| P2 | `spells/werewolf.md`, `spells/red-team.md` | One md per spell |
| P2 | Push-style wake (PTY-inject on message arrival for idle agents) | ~80 lines |
| P3 | Session-token auth + CORS for remote access | Borrow hermes-agent's `_SESSION_TOKEN` design |
| P3 | Tauri desktop packaging | Borrow golutra's `src-tauri/` |
| P3 | Agent sandboxing (Docker / SSH isolation) | Borrow openclaw's `agents/sandbox/` |

## Acknowledgments

flockmux stands on the shoulders of several open-source projects:

- **[hermes-agent](https://github.com/NousResearch/hermes-agent)** — PTY
  bridge + multi-channel gateway architecture. The wake-check JSON wire
  protocol is directly inspired by Hermes's shell hooks.
- **[OpenClaw](https://github.com/openclaw/openclaw)** — Spell front-matter
  conventions, MCP dynamic loading, agent sandboxing patterns.
- **[swarm-ide](https://github.com/swarm-ide)** — "create + send" two-
  primitive philosophy, per-agent runner model, topology-as-spell concept.
- **[golutra](https://github.com/golutra)** — Tauri-side PTY plumbing,
  WebGL cooldown pool design, OSC shim pattern, CLI plugin manifest.
- **[asciinema-player](https://github.com/asciinema/asciinema-player)** —
  In-browser recording playback. WASM-rendered, full keyboard controls.
- **[portable-pty](https://docs.rs/portable-pty)** — The PTY abstraction
  every agent runs on.

## Contributing

flockmux is currently a personal project. PRs and issues are welcome but
expect slow response times.

When proposing a new CLI plugin (Gemini, Qwen, OpenCode, ...), include a
recorded OAuth verification (asciicast or video) showing the plugin works
end-to-end on a fresh checkout. The MVP shipped only claude + codex
because those are the only two we've personally verified at length.

For larger structural changes, please read the design plan first (private
to the maintainer's `~/.claude/plans/`; ask for a copy if you need
context).

Commit identity in this repo is set per-repo via local git config:

```bash
git config user.name  "your-name"
git config user.email "your@email"
# DO NOT modify global git config.
```

## License

[MIT](LICENSE). See the file for the full text.
