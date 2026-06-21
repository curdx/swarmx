---
name: swarmx-agent-upgrades
description: Use when changing swarmx agent transports, Claude/Codex plugin behavior, ACP/app-server integration, billing guardrails, Goal Mode, or multi-agent orchestration features in this repo.
---

# Swarmx Agent Upgrades

Use this skill before editing provider transports, plugin manifests, orchestration state, or agent-control UI in swarmx.

## Non-Negotiables

- Keep Claude Code on the interactive PTY subscription path by default.
- Do not switch Claude to `claude -p`, Claude Agent SDK, API-key transport, or ACP adapter by default.
- Any Claude print/SDK/API/adapter path must require explicit opt-in and visible billing language.
- Keep ambient `ANTHROPIC_*` env vars blocked from default Claude children unless the user explicitly opts into paid transport.
- Prefer Codex for structured-transport experiments first. Codex app-server is the safer first target because it is a CLI-account structured surface and does not require changing Claude's billing surface.
- Keep PTY fallback available for both providers until the structured session driver is complete and tested.

## Upgrade Order

1. Read current manifests in `cli-plugins/` and the spawn guardrails in `crates/swarmx-server/src/spawn.rs`.
2. If adding structured transport, start with protocol plumbing and tests in `crates/swarmx-server/src/acp.rs`.
3. Wire only a narrow, opt-in route before replacing any normal spawn path.
4. For Goal Mode or orchestration state, keep storage/API/UI aligned:
   - migration in `crates/swarmx-storage/migrations/`
   - model/store methods in `crates/swarmx-storage/src/`
   - axum route in `crates/swarmx-server/src/routes/`
   - web types/helpers/routes in `web/src/`
5. Add focused tests first, then run broader tests.

## Verification

Run these after relevant changes:

```bash
cargo test -p swarmx-server billing_policy_tests -- --nocapture
cargo test -p swarmx-server plugins::tests::shipped_manifests_declare_formats -- --nocapture
cargo test -p swarmx-server acp -- --nocapture
cargo test -p swarmx-storage --tests -- --nocapture
cargo test --workspace -- --nocapture
```

For frontend changes:

```bash
cd web && npm run build
```

Avoid `cargo fmt --all` when the tree already has unrelated formatting drift. Format only touched Rust files unless the user explicitly asks for repository-wide formatting.
