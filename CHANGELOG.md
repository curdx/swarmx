# Changelog

All notable changes to swarmx are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); the project aims to
follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Before tagging a release, run `node scripts/bump-version.mjs <x.y.z>` to sync
the version across all four manifests.

## [0.2.0] — 2026-07-20

New engine: **Kimi Code** (`kimi`) joins the swarm as a first-class PTY
engine, on par with claude/codex — spawn, wake, multi-agent delegation,
activity, and usage all verified live and in a real browser (Playwright).

### Added
- **Kimi Code engine** (`cli-plugins/kimi.toml` + `cli/kimi.rs`): OAuth
  subscription billing (`billing_surface = interactive-subscription`),
  `--yolo` auto-approve, ambient `KIMI_MODEL_*` API-key override blocked by
  default, `swarmx-mcp wake-check --hook-format kimi` Stop-hook protocol
  (stderr + exit 2) patched idempotently into the user-level
  `~/.kimi-code/config.toml`.
- **Keystroke bootstrap hardening for large prompts**: new manifest fields
  `bracketed_paste` (explicit `ESC[200~…201~` framing) and
  `bootstrap_ready_needle` / `bootstrap_ready_settle_ms` (gate the paste on
  the TUI's own settled banner). Fixes a live-verified race where kimi's TUI
  silently ate the ~26KB bootstrap before its input pipeline was ready.
- **kimi transcript tailer**: `transcript.rs` now tails
  `~/.kimi-code/sessions/wd_*_<sha256(cwd)[:12]>/<session>/agents/main/
  wire.jsonl`, feeding the Activity view (`tool.call`/`tool.result`) and the
  Usage page (`usage.record`, `kimi-code/*` models) — previously both were
  empty for kimi.

### Changed
- **kimi MCP injection relies on env inheritance**: `.kimi-code/mcp.json`
  carries no per-agent values (verified: kimi's stdio MCP children inherit
  the parent's env), so same-workspace agents can no longer overwrite each
  other's MCP identity (the reasonix/zulu class of bug).
- `swarm_spawn_worker`'s `cli` enum, fusion `valid_clis`, and the autopilot
  fusion panel now include `kimi` (and `zulu`); the first-response watchdog
  gives transcript-less engines (incl. kimi) a 150s window.
- `roles/orchestrator.md`'s engine-selection guidance covers kimi.

## [Unreleased]

Production-readiness hardening from the 2026-06 maturity audit
(`docs/maturity-audit-2026-06.md`).

### Added
- **Database safety net**: pre-migration `VACUUM INTO` snapshots, corrupt-DB
  detection (`PRAGMA quick_check`) with quarantine + rebuild on open, and a
  migration upper-bound guard that refuses to run an older binary against a
  newer schema.
- **Liveness reaper**: a periodic sweep that retires any agent whose child
  process actually died without emitting a `ShimExit`, killing the "forever
  green dot" fake state.
- **`swarmx-server doctor`** preflight self-check (shim/mcp binaries, CLIs on
  PATH, free port, writable data dir) and an `effective config` startup dump.
- **Periodic retention prune** (every 6h), now also covering the high-frequency
  `agent_usage` / `agent_activities` tables.
- **Daily-rolled file logging** under `~/.swarmx/logs/`.
- **Frontend tests in CI**: vitest unit tests (DAG-edge derivation invariants) +
  Playwright e2e as hard gates; ESLint flat config (react-hooks); cargo-audit +
  npm audit + Dependabot.
- `docs/configuration.md` documenting every `SWARMX_*` variable (harness-check
  guards completeness); a top-level `LICENSE` (MIT).

### Changed
- **Tauri**: closing the main window now quits the app and terminates the
  bundled server sidecar (no more orphaned server holding port 7777).
- **Versioning**: a single source of truth via `scripts/bump-version.mjs`;
  releases are now gated on CI (`cargo test` + harness) passing.
- Migration registry refactored to a `MIGRATIONS` array (harness-check updated).

### Security
- **File browser**: hard denylist for credential paths (`~/.ssh`, `~/.aws`,
  `*.pem`/`*.key`, `~/.claude.json`, …) on every request, even with `all=1`.
- **DNS-rebind defense**: no-Origin requests now also require a loopback `Host`.
- **Panic isolation**: a handler panic returns 500 for that request instead of
  dropping the whole connection; the server now drains on SIGINT/SIGTERM.
