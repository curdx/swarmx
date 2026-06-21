-- 0004_workspaces: promote workspace to a first-class entity.
--
-- Before this migration, "workspace" was a frontend-derived concept: the UI
-- grouped agents by their cwd path (agents.workspace string). That broke as
-- soon as a spell (e.g. critic-loop) spawned agents under per-agent fallback
-- cwds — each agent then appeared as its own orphan workspace tab in the
-- left nav.
--
-- The fix: workspace becomes a proper DB row (with name + accent + slug +
-- cwd metadata), and every agent carries a workspace_id FK pointing at its
-- group. cwd (agents.workspace string) is kept side-by-side as the
-- filesystem fact — it's set by the spawn layout (PerAgent / Shared) and is
-- no longer used for grouping. The two concepts are now orthogonal.
--
-- spell_runs is a lineage table — it records which spell call produced
-- which agents, so future UI can fold them into "this spell run's group"
-- without changing the mailbox / wake plumbing (which stays agent_id-flat).
--
-- ALTER TABLE ADD COLUMN: SQLite can only add nullable columns without a
-- default rewrite, so workspace_id / spell_run_id are nullable in schema
-- and enforced non-null at the application layer (see spawn_with_bookkeeping
-- after Step 3). Old rows from before Step 3 land with NULL and are tolerated.

INSERT INTO schema_version VALUES (4);

CREATE TABLE workspaces (
    id           TEXT PRIMARY KEY,
    slug         TEXT NOT NULL UNIQUE,
    name         TEXT NOT NULL,
    accent       TEXT,
    cwd          TEXT NOT NULL,
    created_at   INTEGER NOT NULL,
    deleted_at   INTEGER
);
CREATE INDEX idx_workspaces_alive ON workspaces(deleted_at) WHERE deleted_at IS NULL;

CREATE TABLE spell_runs (
    id              TEXT PRIMARY KEY,
    workspace_id    TEXT NOT NULL REFERENCES workspaces(id),
    spell_name      TEXT NOT NULL,
    task            TEXT NOT NULL,
    caller_agent_id TEXT,
    started_at      INTEGER NOT NULL
);
CREATE INDEX idx_spell_runs_workspace ON spell_runs(workspace_id);

ALTER TABLE agents ADD COLUMN workspace_id TEXT REFERENCES workspaces(id);
ALTER TABLE agents ADD COLUMN spell_run_id TEXT REFERENCES spell_runs(id);
CREATE INDEX idx_agents_workspace ON agents(workspace_id) WHERE killed_at IS NULL;
