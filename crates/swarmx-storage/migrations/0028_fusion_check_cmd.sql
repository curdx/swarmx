-- 0028_fusion_check_cmd: objective quality gate for a fusion competition.
--
-- A judge that only reads `git diff` is fooled by code that LOOKS correct but
-- fails at runtime (a `>` that should be `>=`, a deadlock, a perf regression).
-- Reading a diff cannot catch those — running the code can. This column lets a
-- batch carry an optional shell command (e.g. `pytest -q`, `cargo test`,
-- `npm test`) that the auto-judge runs INSIDE each contestant's worktree BEFORE
-- the LLM deliberates. Contestants whose check fails are objectively out; the
-- pass/fail matrix + output tails are injected into the judge prompt so the
-- verdict is grounded in execution, not just aesthetics.
--
-- check_cmd: shell command run per-contestant-worktree. NULL/empty = no gate
--   (back-compat: the judge falls back to pure-diff reading, the old behaviour).

INSERT INTO schema_version VALUES (28);

ALTER TABLE fusion_batches ADD COLUMN check_cmd TEXT;
