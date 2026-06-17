# Configuration (`FLOCKMUX_*` environment variables)

flockmux-server reads all configuration from `FLOCKMUX_*` environment variables.
**None are required for a normal local run** — every one has a default. Set them
before launching the server (or the Tauri app, which spawns the server).

This file is the single reference for every variable the code reads; it is
guarded by `scripts/harness-check.mjs` (rule 6 / 规则7) so it can't silently
drift out of sync with the code.

## Platform support

flockmux targets **macOS and Linux** (Unix). The PTY teardown that reclaims a
spawned CLI and its descendants relies on Unix process-group signals (`killpg`
+ SIGTERM/SIGKILL), and data paths default off `$HOME`. The release pipeline
also builds Windows artifacts, but Windows is **unverified / experimental** — on
non-Unix only the direct shim child is killed (a grandchild CLI can be left
running). Use macOS or Linux for a supported setup.

## Network

| Variable | Default | Purpose |
|---|---|---|
| `FLOCKMUX_PORT` | `7777` | TCP port the server binds (loopback only). Also derives `FLOCKMUX_SERVER_URL` and the URL baked into spawned agents' wake-hook + MCP config. |
| `FLOCKMUX_SERVER_URL` | `http://127.0.0.1:<PORT>` | Base REST URL agents / MCP use to reach the server. Derived from `FLOCKMUX_PORT`; set explicitly only for unusual setups. |

## Data locations

| Variable | Default | Purpose |
|---|---|---|
| `FLOCKMUX_DB_PATH` | `~/.flockmux/flockmux.db` | SQLite database file. |
| `FLOCKMUX_WORKSPACES_DIR` | `~/.flockmux/workspaces` | Per-agent scratch workspaces. |
| `FLOCKMUX_BLACKBOARD_DIR` | `~/.flockmux/blackboard` | Blackboard markdown KV store. |
| `FLOCKMUX_RECORDINGS_DIR` | `~/.flockmux/recordings` | asciicast session recordings. |

## Resource / binary paths (override bundled defaults — mostly for dev)

| Variable | Default | Purpose |
|---|---|---|
| `FLOCKMUX_CLI_PLUGINS_DIR` | bundled `cli-plugins/` | Built-in CLI plugin manifests. |
| `FLOCKMUX_USER_CLI_PLUGINS_DIR` | `~/.flockmux/cli-plugins` | User-added CLI plugins (extend / override built-ins). |
| `FLOCKMUX_ROLES_DIR` | bundled `roles/` | Role SOP templates. |
| `FLOCKMUX_SPELLS_DIR` | bundled `spells/` | Spell definitions. |
| `FLOCKMUX_WEB_DIR` | resolved next to the binary | Built web bundle the server serves. |
| `FLOCKMUX_SHIM_PATH` | sibling of the server binary | Path to the `flockmux-shim` binary. |
| `FLOCKMUX_MCP_PATH` | sibling of the server binary | Path to the `flockmux-mcp` binary. |
| `FLOCKMUX_OPENCODE_PLUGIN` | bundled `cli-plugins/opencode/flockmux-wake.js` | Path to the opencode wake plugin JS merged into each opencode worker's per-agent config (the Tauri sidecar sets this to the packaged resource). If unresolved, opencode workers still get swarm tools but no auto-wake. |

## Limits & retention

| Variable | Default | Purpose |
|---|---|---|
| `FLOCKMUX_RETENTION_DAYS` | `30` | At boot, prune rows older than N days. `0` (or negative) = keep everything, never prune. |
| `FLOCKMUX_MAX_LIVE_AGENTS` | built-in cap | Max concurrently live agents (back-pressure on spawn). |
| `FLOCKMUX_MAX_SPAWN_DEPTH` | built-in cap | Max depth of agent-spawns-agent chains (runaway-spawn guard). |

## Behaviour switches

| Variable | Default | Purpose |
|---|---|---|
| `FLOCKMUX_AUTO_RESPAWN_ORCHESTRATORS` | unset (off) | `1` = on boot, re-spawn orchestrators for alive workspaces the orphan sweep killed. Can burn an LLM turn / revive a provider you're avoiding — opt-in. |
| `FLOCKMUX_ALLOW_PAID_TRANSPORT` | unset (off) | Opt-in to a CLI's paid SDK/API transport when its plugin declares one (billing guard). |
| `FLOCKMUX_ALLOW_CLAUDE_PRINT` | unset (off) | Opt-in to claude non-PTY print/SDK mode (a separate billing surface). |

## Agent git identity

| Variable | Default | Purpose |
|---|---|---|
| `FLOCKMUX_GIT_USER_NAME` | global git default | git author name agents commit under. |
| `FLOCKMUX_GIT_USER_EMAIL` | global git default | git author email agents commit under. |

## Diagnostics

| Variable | Default | Purpose |
|---|---|---|
| `FLOCKMUX_MCP_LOG` | unset | Enable verbose `flockmux-mcp` logging. |

## Internal / not user-settable

- `FLOCKMUX_AGENT_ID` — injected by the server into each spawned worker so its
  MCP subprocess knows its own identity. **Do not set this yourself.**

## Test-only (used by the test suite / CI; ignore in normal operation)

- `FLOCKMUX_LEAK_CANARY` — env-isolation canary asserted by a PTY test.
- `FLOCKMUX_TEST_TRUTHY` — truthy-parsing fixture.
- `FLOCKMUX_GOLDEN_PORT` — port for the golden-CLI smoke scripts.
