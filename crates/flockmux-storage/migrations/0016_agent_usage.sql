-- 0016_agent_usage: per-turn token usage captured from the worker's CLI session
-- JSONL (the transcript tailer already reads that file for activity, so this is
-- a near-free add). Powers the Usage/Cost observability page — flockmux has NO
-- token/cost data otherwise, because claude/codex never report usage back over
-- our PTY transport.
--
-- One row per usage event:
--   claude — each assistant turn's `message.usage`
--   codex  — each `token_count` event's `last_token_usage` (per-turn delta)
-- `model` is whatever the CLI reported (nullable). Cost is NOT stored: it's
-- derived at query time from a pricing table in the server, so re-pricing never
-- needs a migration. agent_id is loose (no FK) so a late-arriving usage row for
-- an already-reaped agent still records.
CREATE TABLE agent_usage (
    id                 INTEGER PRIMARY KEY,
    agent_id           TEXT NOT NULL,
    model              TEXT,
    input_tokens       INTEGER NOT NULL DEFAULT 0,
    output_tokens      INTEGER NOT NULL DEFAULT 0,
    cache_read_tokens  INTEGER NOT NULL DEFAULT 0,
    cache_write_tokens INTEGER NOT NULL DEFAULT 0,
    at                 INTEGER NOT NULL
);
CREATE INDEX idx_agent_usage_agent ON agent_usage(agent_id);
CREATE INDEX idx_agent_usage_at ON agent_usage(at);

INSERT INTO schema_version VALUES (16);
