+++
name = "fullstack-feature"
description = "全栈特性开发：frontend + backend 并行 → test 验证。三个 agent 共享同一 workspace。"
shared_workspace = true

[[agents]]
role_ref = "frontend"

[[agents]]
role_ref = "backend"

[[agents]]
role_ref = "test"
+++

# fullstack-feature

A three-agent spell that delivers a small full-stack feature end-to-end:

- **frontend** (claude by default) writes the UI
- **backend** (codex by default) writes the API + the api.spec contract
- **test** (claude by default) writes and runs e2e tests after both ship

All three agents share the **same workspace directory** — they are
peers in a monorepo (`apps/frontend/`, `apps/backend/`, `tests/`),
not isolated worktrees.

## Topology

```
       (spawn time)
            │
   ┌────────┼────────┐
   ▼        ▼        ▼
frontend backend  test
   │        │        │
   │        │        │ (idles, reads blackboard
   │        │        │  on each wake-check)
   │        ▼        │
   │   writes        │
   │   api.spec ─────┘ (FE reads, starts coding)
   │   on blackboard
   │
   ├─ writes apps/frontend, commits, blackboard.frontend.done
   │  + message to test "FE ready"
   ▼
backend
   ├─ writes apps/backend, commits, blackboard.backend.done
   │  + message to test "BE ready"
   ▼
test (wake-check fires on second message)
   ├─ reads frontend.done + backend.done from blackboard
   ├─ writes tests/e2e/, runs Playwright
   └─ writes test.passed / test.failed + message to system
```

The fork-join is implemented by the **prompts**, not the spell
executor. Test is spawned at t=0 alongside FE/BE but its role prompt
makes it idle until both done signals are present on the blackboard.
wake-check turns each incoming swarm message into a fresh turn for
test, which re-reads the blackboard and decides whether to keep
idling or proceed.

## Why this works without a `depends_on` mechanism

- The spell executor spawns all three agents at once (current
  `run_spell` behaviour — no changes needed).
- The roles' system_prompt_template strings encode the synchronisation:
  - backend writes `api.spec` first; frontend's prompt idles until
    that key is non-empty
  - test's prompt idles until BOTH `frontend.done` and `backend.done`
    are present
- wake-check (Stop hook → unread message → block decision) ensures
  no agent burns tokens while idling — they wake only when there's
  a new message in their inbox

## Requirements

- `shared_workspace = true` requires the M6a runner change that
  routes all three agents to the same cwd instead of giving each one
  a `workspaces_root/<agent_id>/` directory
- `role_ref` requires the RoleRegistry change that loads `roles/*.md`
  on startup and resolves `role_ref` to the matching role manifest's
  `default_cli` + `system_prompt_template`

If either of those changes is missing, this spell will fail to load
with a clear error (no silent fallback).

## Usage

```bash
# Pre-create the workspace dir (M6a UX — the SpellLauncher will pass
# this as the workspace_dir field in the run-spell request):
mkdir -p /tmp/my-feature
cd /tmp/my-feature && git init

# In the browser UI:
# 1. Open SpellLauncher
# 2. Pick "fullstack-feature"
# 3. workspace_dir: /tmp/my-feature
# 4. task: "Build a todo app — add/remove/toggle, React + FastAPI + SQLite"
# 5. Launch
```

Three PTYs appear simultaneously; backend hits the blackboard first
with `api.spec`, frontend wakes once it sees it and starts coding,
test idles in the background. Total turn-around for a tiny demo:
~5-10 minutes (mostly model latency).
