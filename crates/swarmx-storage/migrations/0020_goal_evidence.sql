-- 0020_goal_evidence: audit trail proving why a goal is active/blocked/done.
--
-- Goals hold the objective and terminal status. Evidence rows hold the
-- observed facts that justify that status: worker output, blackboard paths,
-- verification commands, notes, and future automated checks.
CREATE TABLE goal_evidence (
    id              TEXT PRIMARY KEY,
    goal_id         TEXT NOT NULL REFERENCES goals(id),
    kind            TEXT NOT NULL,
    summary         TEXT NOT NULL,
    source_agent_id TEXT,
    blackboard_path TEXT,
    command         TEXT,
    created_at      INTEGER NOT NULL
);

CREATE INDEX idx_goal_evidence_goal_created
    ON goal_evidence(goal_id, created_at DESC);
CREATE INDEX idx_goal_evidence_kind
    ON goal_evidence(kind, created_at DESC);

INSERT INTO schema_version VALUES (20);
