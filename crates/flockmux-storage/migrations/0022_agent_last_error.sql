-- 0022_agent_last_error: persist the last "alive but can't work" reason.
--
-- The pump's HealthScanner detects auth/quota banners ("Not logged in · Run
-- /login", rate-limit notices) and the first-response watchdog detects an
-- orchestrator that never spoke; both publish AgentState::Error + a system
-- AgentActivity over the swarm WS. But that stream lives only in browser memory
-- — a page reload loses it, so the honest failure card would vanish on refresh
-- and the agent would look "online" again. Persist the reason here so
-- `GET /api/agent` carries it and the UI can re-render the failure card on a
-- cold load (the WS feed is lossy with no resume — every health fact needs a
-- REST snapshot, per the existing architecture rule).
--
-- All nullable, no DEFAULT: existing rows and healthy agents stay NULL. Safe
-- ADD COLUMN, no table rewrite. `last_error_kind` is the coarse class
-- (auth | rate_limit | fatal) steering which remedy buttons the card offers;
-- `last_error_at` is the unix-ms the failure was recorded.
ALTER TABLE agents ADD COLUMN last_error TEXT;
ALTER TABLE agents ADD COLUMN last_error_kind TEXT;
ALTER TABLE agents ADD COLUMN last_error_at INTEGER;

INSERT INTO schema_version VALUES (22);
