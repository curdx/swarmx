-- 0015_thread_reasoning_effort: per-direction reasoning/thinking effort.
--
-- Orthogonal to model_tier (0014): the model is WHICH brain, effort is HOW HARD
-- it thinks. Both Claude Code and Codex converged (2026) on discrete effort
-- levels — Claude `--effort low|medium|high|xhigh|max`, Codex
-- `-c model_reasoning_effort=minimal|low|medium|high|xhigh`. We store an ABSTRACT
-- level (low|medium|high|max) and each CLI plugin maps it to its concrete value
-- at spawn (effort_levels in the manifest). NULL = omit = the model's own
-- default (high on Opus 4.8 / medium on Codex). Both CLIs degrade gracefully if
-- a level outranks the model, so a single value is safe across models.
INSERT INTO schema_version VALUES (15);

ALTER TABLE threads ADD COLUMN reasoning_effort TEXT;
