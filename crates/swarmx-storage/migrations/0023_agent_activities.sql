-- 0023_agent_activities: persist tool-level agent activity (Edit/Bash/Read…).
--
-- Until now `GET /api/agent/:id/activity` was served only from the transcript
-- tailer's in-memory ring (DashMap<agent_id, VecDeque>, cap 100, current
-- server session only). A page reload / WS reconnect — or a server restart —
-- lost it, so the drawer's Activity tab and any in-thread activity timeline
-- went blank for an agent that had already done its work (诊断3「活动刷新即
-- 空」; same lossy-WS-needs-a-REST-snapshot rule as 0022_agent_last_error).
--
-- One row per (agent_id, seq) step. UNIQUE(agent_id, seq) lets the writer
-- upsert: a `running` row is replaced in place by its later `ok`/`error`,
-- mirroring the ring's collapse-by-seq. `duration_ms` is NULL while running.
CREATE TABLE IF NOT EXISTS agent_activities (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    agent_id    TEXT    NOT NULL,
    seq         INTEGER NOT NULL,
    kind        TEXT    NOT NULL,
    label       TEXT    NOT NULL,
    phase       TEXT    NOT NULL,
    duration_ms INTEGER,
    at          INTEGER NOT NULL,
    UNIQUE(agent_id, seq)
);

CREATE INDEX IF NOT EXISTS idx_agent_activities_agent
    ON agent_activities(agent_id, seq);

INSERT INTO schema_version VALUES (23);
