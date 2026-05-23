+++
name = "fullstack-feature-reviewed"
description = "全栈 + 代码评审：FE/BE 并行 → critic 评审 → test 验证。比 fullstack-feature 多一层 critic 检查代码本身（不仅是 e2e 接口契约）。"
shared_workspace = true

[[agents]]
role_ref = "frontend"

[[agents]]
role_ref = "backend"

[[agents]]
role_ref = "critic"

[[agents]]
role_ref = "test"
# Spell-level override (M6a `depends_on` Option): test no longer waits
# on the raw `*.done` keys directly — it waits on `review.completed`,
# which the critic writes only after reviewing both sides. This is
# what gates test execution behind a code review.
#
# If critic dies, M6c step 5 fallback writes `critic.error` AND
# directly wakes subscribers of `review.completed` — so test still
# gets reactivated and routes through its upstream-failed branch.
depends_on = ["review.completed"]
+++

# fullstack-feature-reviewed

Same topology as `fullstack-feature` (one shared monorepo with
`apps/frontend/`, `apps/backend/`, `tests/`) with **one extra agent**:
a `critic` that sits between the producers and the tester.

## What you'll see

1. Four PTYs spawn: frontend (claude), backend (codex), critic
   (claude), test (claude). All share the same workspace.
2. Frontend + backend work in parallel just like the original spell.
3. As each side writes its `*.done` key, the WakeCoordinator wakes
   the critic. Critic reads the contract + just-finished side's code
   and writes `<role>.review` to the blackboard. After **both**
   reviews land, critic writes `review.completed` and stops.
4. Test wakes the moment `review.completed` arrives, runs Playwright
   against the live stack, writes `test.passed` / `test.failed`.

End state on the blackboard:
- `api.spec` (BE wrote it)
- `frontend.done` / `backend.done` (producer summaries)
- `frontend.review` / `backend.review` (critic verdicts)
- `review.completed` (critic's summary)
- `test.passed` or `test.failed` (Playwright result)

## When to use this over plain `fullstack-feature`

Pick this when **code quality matters more than turnaround time**:

- Production-ish features you'll ship somewhere.
- Tasks with security implications (auth, file uploads, raw SQL).
- When you suspect the test suite alone wouldn't catch what's wrong
  (e.g. you wrote a vague task and want the critic to flag where it
  was misinterpreted).

Stick with plain `fullstack-feature` for:

- Throwaway demos / proofs of concept.
- When the user has clear acceptance criteria the test agent will
  exercise directly.
- When the ~30-90s of extra wall-clock for the review step matters.

## What this v1 critic does NOT do

- It does **not** block the test agent on `verdict: "block"`. Issues
  are advisory; you'll see them in `frontend.review` /
  `backend.review` and a summary line in the test report.
- It does **not** trigger a fixer loop. If the critic spots real
  problems, the operator decides whether to re-run the spell with a
  sharper task description.
- It does **not** run the code itself — no shell calls beyond Read.
  It reasons about the source statically.
