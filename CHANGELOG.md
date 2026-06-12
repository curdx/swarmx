# Changelog

All notable changes to flockmux are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); the project aims to
follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Before tagging a release, run `node scripts/bump-version.mjs <x.y.z>` to sync
the version across all four manifests.

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
- **`flockmux-server doctor`** preflight self-check (shim/mcp binaries, CLIs on
  PATH, free port, writable data dir) and an `effective config` startup dump.
- **Periodic retention prune** (every 6h), now also covering the high-frequency
  `agent_usage` / `agent_activities` tables.
- **Daily-rolled file logging** under `~/.flockmux/logs/`.
- **Frontend tests in CI**: vitest unit tests (DAG-edge derivation invariants) +
  Playwright e2e as hard gates; ESLint flat config (react-hooks); cargo-audit +
  npm audit + Dependabot.
- `docs/configuration.md` documenting every `FLOCKMUX_*` variable (harness-check
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
