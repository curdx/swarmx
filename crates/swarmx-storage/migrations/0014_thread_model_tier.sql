-- 0014_thread_model_tier: per-direction model override.
--
-- A workspace's global tier→concrete mapping lives in ~/.swarmx/models.json
-- (settings → 模型). But the user wants to choose WHICH model the AI they chat
-- with (the orchestrator) runs at, per direction — e.g. Opus for a hard refactor
-- direction, Haiku for a cheap chore direction — without changing the global
-- default for everything.
--
-- `model_tier` is an abstract tier (opus|sonnet|haiku) OR a concrete model id,
-- resolved per-CLI by models_config at spawn time (same resolve() the global
-- default uses). NULL = "use the global default" (the prior behaviour). It is a
-- FALLBACK at spawn: an agent whose role pins its own tier still wins; agents
-- with no tier (the orchestrator) inherit this direction tier.
INSERT INTO schema_version VALUES (14);

ALTER TABLE threads ADD COLUMN model_tier TEXT;
