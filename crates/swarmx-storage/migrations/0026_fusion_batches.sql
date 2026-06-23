-- 0026_fusion_batches: a "fusion" is a multi-model competition — one need is
-- implemented independently by N contestants (each its OWN isolated direction /
-- worktree), then a judge direction reviews the N diffs and synthesizes a merged
-- result. This differs fundamentally from the collaborative swarm (workers share
-- one direction's worktree + blackboard and divide labour): contestants must NOT
-- see each other's work, so each is a separate direction and blackboard isolation
-- is by direction prefix (see list_blackboard_ops_scoped).
--
-- This table is the binding that groups those otherwise-independent directions
-- into one competition, so the UI can fold them into a single "competition" view
-- and the judge knows which contestants to read.
--
-- contestant_thread_ids_json: JSON array of thread.id, one per contestant.
-- judge_thread_id: the privileged direction that cross-reads all contestants'
--   diffs + blackboard (nullable until the judge stage is reached / created).
-- need: the verbatim requirement sent IDENTICALLY to every contestant (fairness).
-- status: 'running' (contestants working) | 'judging' | 'done' | 'failed'.

INSERT INTO schema_version VALUES (26);

CREATE TABLE fusion_batches (
    id                          TEXT PRIMARY KEY,
    workspace_id                TEXT NOT NULL REFERENCES workspaces(id),
    -- short human/url slug for the batch, unique among alive batches of a ws.
    slug                        TEXT NOT NULL,
    need                        TEXT NOT NULL,
    contestant_thread_ids_json  TEXT NOT NULL,
    judge_thread_id             TEXT,
    status                      TEXT NOT NULL DEFAULT 'running',
    created_at                  INTEGER NOT NULL,
    deleted_at                  INTEGER
);
CREATE INDEX idx_fusion_batches_workspace
    ON fusion_batches(workspace_id) WHERE deleted_at IS NULL;
CREATE UNIQUE INDEX idx_fusion_batches_ws_slug_alive
    ON fusion_batches(workspace_id, slug) WHERE deleted_at IS NULL;
