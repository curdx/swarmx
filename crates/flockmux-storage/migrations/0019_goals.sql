-- 0019_goals: first-class goal state per workspace/direction.
--
-- The orchestrator already writes task/progress ledgers, but those are markdown
-- artifacts. This table gives the control plane a typed place for the current
-- objective, success criteria, budget, and terminal status so the UI and future
-- MCP tools can reason about "what are we trying to finish?" without parsing
-- prose.
CREATE TABLE goals (
    id               TEXT PRIMARY KEY,
    workspace_id     TEXT NOT NULL REFERENCES workspaces(id),
    thread_id        TEXT REFERENCES threads(id),
    objective        TEXT NOT NULL,
    success_criteria TEXT NOT NULL DEFAULT '[]',
    status           TEXT NOT NULL DEFAULT 'active',
    budget_tokens    INTEGER,
    created_at       INTEGER NOT NULL,
    updated_at       INTEGER NOT NULL,
    completed_at     INTEGER
);

CREATE INDEX idx_goals_workspace_thread
    ON goals(workspace_id, thread_id, updated_at DESC);
CREATE INDEX idx_goals_status
    ON goals(status, updated_at DESC);

INSERT INTO schema_version VALUES (19);
