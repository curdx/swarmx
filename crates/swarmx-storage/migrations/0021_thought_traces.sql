-- 0021_thought_traces: product-level reasoning / execution summaries.
--
-- These rows are NOT raw chain-of-thought. They hold user-facing stage
-- summaries so chat can explain what happened, how long it took, and which
-- message/agent the work belonged to.
CREATE TABLE thought_traces (
    id                 TEXT PRIMARY KEY,
    trigger_message_id INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    response_message_id INTEGER REFERENCES messages(id) ON DELETE SET NULL,
    agent_id           TEXT NOT NULL,
    workspace_id       TEXT,
    thread_id          TEXT,
    status             TEXT NOT NULL CHECK (status IN ('active', 'done', 'expired', 'error')),
    started_at         INTEGER NOT NULL,
    completed_at       INTEGER,
    summary_json       TEXT NOT NULL DEFAULT '[]',
    updated_at         INTEGER NOT NULL
);

CREATE UNIQUE INDEX idx_thought_traces_trigger
    ON thought_traces(trigger_message_id);
CREATE UNIQUE INDEX idx_thought_traces_response
    ON thought_traces(response_message_id)
    WHERE response_message_id IS NOT NULL;
CREATE INDEX idx_thought_traces_agent_active
    ON thought_traces(agent_id, status, started_at DESC);
CREATE INDEX idx_thought_traces_thread
    ON thought_traces(thread_id, started_at DESC);

CREATE TABLE thought_trace_events (
    id       INTEGER PRIMARY KEY AUTOINCREMENT,
    trace_id TEXT NOT NULL REFERENCES thought_traces(id) ON DELETE CASCADE,
    phase    TEXT NOT NULL,
    label    TEXT NOT NULL,
    source   TEXT NOT NULL,
    at       INTEGER NOT NULL,
    meta     TEXT
);

CREATE INDEX idx_thought_trace_events_trace_at
    ON thought_trace_events(trace_id, at ASC);

INSERT INTO schema_version VALUES (21);
