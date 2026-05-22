+++
name = "auto-dispatch"
description = "自动派活：planner 看你输入的自然语言，自动选 spell 并启动它"
shared_workspace = false

[[agents]]
role_ref = "planner"
+++

# auto-dispatch

A one-agent spell whose only job is to route. Pick this spell when you
don't know which workflow you want — the planner reads your task,
calls `swarm_list_spells` to see what's available, picks the most
appropriate one, sharpens the task description, and calls
`swarm_run_spell` to launch it.

## What you'll see

1. One planner pane appears (claude). It runs ~10-30 seconds, calls
   `swarm_list_spells` + `swarm_run_spell` once each, then stops.
2. The spawned spell's panes appear next to the planner's. The
   planner pane stays open (idle) so you can see what it decided.
3. From there it's whatever the chosen spell normally does.

## When NOT to use it

- If you already know which spell you want — just pick it from the
  dropdown directly. You save ~20 seconds of planner overhead and
  one extra agent pane.
- If your task is genuinely ambiguous and might need clarification.
  The planner picks something even when nothing fits great. Read its
  justification message before letting the downstream crew run for
  10 minutes on the wrong target.

## Adding new spells

The planner discovers spells dynamically via `swarm_list_spells`, so
you don't need to teach it about each one. Make sure each new spell's
`description` line in its front-matter actually describes what it
does — that's the only signal the planner has.
