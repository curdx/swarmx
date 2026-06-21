-- 0013_agent_last_activity: persist each agent's last tool-level activity time.
--
-- The transcript tailer already tails a worker's CLI session JSONL and emits
-- AgentActivity over the swarm WS, but that stream lives only in browser memory
-- — a page reload loses it, so the member-dot "is this agent stuck or just
-- idle?" heuristic went blind after refresh and showed a stale green "online"
-- dot for a silently-wedged worker (QA finding F3). Persist the timestamp here
-- so `GET /api/agent` carries it and the UI can tell "wedged" from "idle" even
-- on a cold load.
--
-- Nullable + no DEFAULT: existing rows and agents that never produced a tool
-- event stay NULL (the UI falls back to spawned_at / message-stream heuristics
-- there). Safe ADD COLUMN, no table rewrite. The tailer updates it
-- monotonically (only ever forward) at most once per ~700ms poll per live
-- worker, so write volume is negligible.
ALTER TABLE agents ADD COLUMN last_activity_at INTEGER;

INSERT INTO schema_version VALUES (13);
