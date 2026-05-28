+++
name = "init"
description = "工作空间初始化:拉一个 orchestrator,持续在线接客 + 派 worker。"
# orchestrator 必须 cwd = 用户的真实项目目录,否则它扫不到东西也派不下去。
shared_workspace = true

[[agents]]
role_ref = "orchestrator"
+++

# init

One-agent spell run at workspace creation. The agent is the
**orchestrator** (see `roles/orchestrator.md`) — flockmux's single
point of contact per workspace. It runs Phase A (scan + greet) on
first wake, then sits in Phase B (dual-ledger loop) for the lifetime
of the workspace.

## What you'll see

1. One orchestrator pane appears (claude). It scans ~30s, writes
   `task.ledger.md` + `progress.ledger.md` to the blackboard, and
   greets the user.
2. From then on, the user talks to one entity. The orchestrator
   decides: chat back, do the work itself, or `swarm_spawn_worker`
   to dispatch.
3. Workers come and go. The orchestrator stays.

## Why one agent (not the old scout + planner + business team)

The old model pre-allocated roles via static spells (fullstack-feature
always = FE + BE + Test). hello world also paid that 3-agent tax.
Magentic-One's insight: scaling is a runtime decision. The orchestrator
looks at the task and picks 0 / 1 / N workers.

## Restart resilience

If the server restarts, the orchestrator's PTY dies. On next launch:
- The Phase A scan is skipped because `task.ledger.md` already exists
- Orchestrator reads ledgers, sees current state, picks up where it
  was.

No special recovery logic required — the ledger IS the recovery.
