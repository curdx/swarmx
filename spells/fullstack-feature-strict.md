+++
name = "fullstack-feature-strict"
description = "全栈 + 严格代码评审 + 自动修复：FE/BE 并行 → critic 评审 → 有 block 就让 fixer 改 → critic 再评 → 最多 3 轮 → test 验证。比 fullstack-feature-reviewed 多一个 fixer 闸；critic verdict 真的能挡住下游。"
shared_workspace = true
# M6d-3: critic↔fixer is an intentional dependency loop (critic
# depends_on=["fixer.done"], fixer depends_on=["review.completed"],
# both produce the other's wake signal). The loop is bounded by
# fixer's prompt-level `round < 3` check, NOT by graph topology.
# This flag tells run_spell's cycle detector to permit it.
allow_cycles = true

[[agents]]
role_ref = "frontend"

[[agents]]
role_ref = "backend"

[[agents]]
role_ref = "critic"

[[agents]]
role_ref = "fixer"

[[agents]]
role_ref = "test"
# Spell-level override (M6a): in strict mode, test depends on
# review.completed reaching verdict ∈ {pass, warn} OR the fixer
# escalating after max rounds. The test prompt branches on the
# review body — if all verdicts are pass/warn, runs e2e normally;
# if fixer.escalated is present, runs e2e with a note in the
# report that critic flagged remaining blockers the fixer couldn't
# address.
depends_on = ["review.completed"]
+++

# fullstack-feature-strict

The quality-gated variant of `fullstack-feature`. Critic isn't
advisory anymore — when it flags `block`-level issues, a `fixer`
agent reads the findings, makes minimum-viable code changes,
commits, and asks critic for a re-review. Loop until critic
clears or the round counter hits 3.

```
FE writes frontend.done ──┐
                          ├─► critic round 1 ─► review.completed
BE writes backend.done  ──┘                            │
                                            ┌──────────┴──────────┐
                                            │                     │
                                          block               pass/warn
                                            │                     │
                                            ▼                     │
                                         fixer reads               │
                                         <role>.review             │
                                         fixes code                │
                                         writes fixer.done         │
                                            │                     │
                                            ▼                     │
                                         critic round N+1          │
                                            │                     │
                                            └─ loop until pass     │
                                               or round >= 3       │
                                            ▼                     │
                                         (escalated or             │
                                          eventually pass)         │
                                            └────────────►────────►test
```

## When to use

- **High-stakes features** where you want the agents to iterate to
  passing code before test runs.
- **Tricky integrations** (auth, payments, file uploads) where
  critic's first-pass review is likely to catch real blockers and
  you trust the fixer to address them without scope creep.

For prototypes / demos, stick with the un-strict variants:
- `fullstack-feature` — no critic at all, fastest turnaround.
- `fullstack-feature-reviewed` — advisory review only, no fixer
  loop, test runs regardless of critic verdict.

## Cost vs benefit

A strict run costs:
- 1 extra agent (fixer) spawned upfront (idle if all critic
  verdicts are pass/warn — no wasted tokens, just an idle PTY)
- Each fix round adds 1-2 minutes (fixer reads + edits + commits,
  critic re-reads + re-reviews)
- Max 3 rounds = up to 6 extra minutes vs un-strict reviewed

In return you get code that critic has approved at least once
before test runs against it — fewer "test passes but the code is
embarrassing" outcomes.

## What if fixer can't address an issue

Fixer escalates: `fixer.escalated` lands on the blackboard with
the remaining issues. Test runs anyway but its final report
mentions the escalation. Operator decides what to do (re-run
with a sharper task, accept the warnings, hand-edit, etc.).

## Composition with HITL gate

This spell does NOT include the M6c-7 architect gate. If you want
BOTH human-approved design AND strict code review, use
`fullstack-feature-gated` first to settle the design, kill it
after FE/BE/test commit, then re-run with `fullstack-feature-strict`
on the same workspace — but in practice you almost never need
both at once. Pick the layer that matters for the task.
