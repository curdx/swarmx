-- 0002_pty_recordings: per-agent asciicast v2 recording metadata.
--
-- The cast file itself lives on disk (path column). SQLite only tracks the
-- pointer + lifecycle timestamps so the UI can list recordings without
-- scanning the recordings directory.
--
-- `id` is the recording's own identifier — independent of agent_id so a
-- future re-record / split-segment feature can keep history per agent.
-- For M3 we mint one recording per agent at spawn time.

INSERT INTO schema_version VALUES (2);

CREATE TABLE pty_recordings (
    id            TEXT PRIMARY KEY,
    agent_id      TEXT NOT NULL,
    path          TEXT NOT NULL,
    started_at    INTEGER NOT NULL,
    finalized_at  INTEGER,
    duration_ms   INTEGER,
    cols          INTEGER NOT NULL,
    rows          INTEGER NOT NULL,
    last_seq      INTEGER
);
CREATE INDEX idx_recordings_agent ON pty_recordings(agent_id);
CREATE INDEX idx_recordings_started ON pty_recordings(started_at);
