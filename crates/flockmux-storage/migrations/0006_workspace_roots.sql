-- 0006_workspace_roots: attach dependency-source folders to a workspace.
--
-- A workspace's primary project dir stays in workspaces.cwd. This table holds
-- ADDITIONAL source roots the user attaches (e.g. an internal lib / tool repo
-- the primary project depends on) so AI agents can read their source directly
-- instead of decompiling/guessing. role = "dependency" | "tool" (free text).
-- Rows are kept even after the workspace is soft-deleted (harmless; always
-- queried by workspace_id).

INSERT INTO schema_version VALUES (6);

CREATE TABLE workspace_roots (
    id           TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id),
    path         TEXT NOT NULL,
    role         TEXT NOT NULL,
    label        TEXT,
    created_at   INTEGER NOT NULL
);
CREATE INDEX idx_workspace_roots_ws ON workspace_roots(workspace_id);
