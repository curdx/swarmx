+++
name = "fullstack-feature-gated"
description = "全栈 + 人工审批：architect 出设计文档 → 用户在 blackboard 写 design.approved 批准 → FE/BE 才动 → test 跑。比 fullstack-feature 多一个 architect 步骤 + 一个人在回路检查点。"
shared_workspace = true

[[agents]]
role_ref = "architect"

[[agents]]
role_ref = "frontend"
# Spell-level override (M6a): FE waits BOTH for the api.spec (BE
# writes it as usual) AND for the human approval. Either landing
# alone wakes FE, its prompt idles until both keys exist.
depends_on = ["api.spec", "design.approved"]
# CRITICAL: `depends_on` only controls SUBSEQUENT wakes after the
# agent stops, NOT the initial bootstrap-injected turn. Without
# this gate prefix, FE would happily start coding the moment its
# bootstrap prompt lands (which is immediately after spawn), seeing
# api.spec exists (BE will write it) and ignoring design.approved.
# The prefix is the FIRST thing in FE's prompt, so FE's first turn
# checks the gate and idles. WakeCoordinator picks it up later.
system_prompt_prefix = """
[HITL GATE — fullstack-feature-gated]

Before doing ANYTHING in your normal SOP below, call
`swarm_read_blackboard` for key `design.approved`.
  - If it does NOT exist OR its body is empty: send one short
    `kind="note"` swarm message to "system" saying
    "frontend idling, waiting for design.approved" and STOP this
    turn immediately. Do not read code, do not call other tools.
    The runtime will wake you the moment the operator writes
    design.approved (you're subscribed via depends_on).
  - If it DOES exist: skip this gate paragraph and proceed with
    your normal workflow below.

Re-check on every wake — the gate clears once design.approved
exists; subsequent wakes (e.g. on api.spec landing) should fall
through this paragraph and proceed normally.
"""

[[agents]]
role_ref = "backend"
# BE in this spell does NOT start writing api.spec until the human
# has approved the architect's design. Same gate-prefix trick as FE.
depends_on = ["design.approved"]
system_prompt_prefix = """
[HITL GATE — fullstack-feature-gated]

Before doing ANYTHING in your normal SOP below (specifically
before writing `api.spec`), call `swarm_read_blackboard` for key
`design.approved`.
  - If it does NOT exist OR its body is empty: send one short
    `kind="note"` swarm message to "system" saying
    "backend idling, waiting for design.approved" and STOP this
    turn immediately. The runtime will wake you the moment the
    operator writes design.approved.
  - If it DOES exist: skip this gate paragraph and proceed with
    your normal workflow below (which starts with writing api.spec).

You'll be re-woken on each blackboard event in your depends_on;
re-check this gate on every wake — it clears once design.approved
exists.
"""

[[agents]]
role_ref = "test"
# Unchanged from the original fullstack-feature: test waits on both
# done signals, no relationship with the human-approval gate.
+++

# fullstack-feature-gated

Same shared-workspace monorepo topology as `fullstack-feature`, with
**one extra agent up front** (the `architect`) and a **human approval
gate** between the architect's design and FE / BE starting to code.

## What you'll see

1. Four PTYs spawn: architect (claude), frontend (claude), backend
   (codex), test (claude). All share the same workspace.
2. **architect runs immediately**. It writes `design.md` to the
   blackboard and sends a `kind="reply"` swarm message to `system`
   saying "Design ready for review."
3. **FE / BE / test sit idle** — their `depends_on` includes
   `design.approved` which doesn't exist yet, so the
   `WakeCoordinator` keeps them parked.
4. **You** open the swarm drawer → `blackboard` tab → click
   `design.md`. Read the architect's plan. Two outcomes:
   - **Approve**: in the same panel, type `design.approved` into
     the new-path input, enter any non-empty body (e.g. `"ok"`),
     click write. The WakeCoordinator wakes FE + BE in the same
     tick. They proceed exactly like in `fullstack-feature`.
   - **Reject (M6d-2)**: type `design.rejected` into the same
     new-path input + write `{"reason": "..."}` as the body. The
     architect is subscribed to that key; it'll wake within a
     second, read your reason, rewrite `design.md` addressing
     it, send another "ready for review" message. Loop as many
     times as needed; approve when you're happy.
5. FE writes UI; BE writes api.spec + implementation; test runs
   Playwright. Same flow as `fullstack-feature` from this point on.

End state on the blackboard:
- `design.md` (architect)
- `design.approved` (you, manually)
- `api.spec`, `frontend.done`, `backend.done`, `test.passed` (same
  as the un-gated spell)

## When to use this over plain `fullstack-feature`

- Non-throwaway features where you want a chance to course-correct
  on tech-stack / data-model / scope before code lands.
- Tasks vague enough that you suspect the agents will misinterpret —
  the architect's `Open questions for the operator` section flags
  exactly that.
- When you're prepared to spend ~30 seconds reading a design
  doc to save 5-10 minutes of wasted FE/BE work.

Stick with `fullstack-feature` for demos / quick prototyping where
fast turnaround matters more than direction-checking.

## Known limitations (M6e+ work)

- If architect crashes, FE/BE silently keep waiting on
  `design.approved` (architect's `*.error` fan-out goes to nobody
  because nothing subscribes to `design.md`). Operator must notice
  and kill the spell.
- No approval timeout — architect (and FE / BE / test) sit forever
  if the operator forgets.
- No dedicated "Approve" / "Reject" buttons in the UI yet — the
  operator writes the key by hand via the blackboard editor's
  new-path input. M6d backlog item.
