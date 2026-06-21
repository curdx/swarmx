-- 0007_workspace_root_parent: make workspace_roots a user-defined LOGICAL tree.
--
-- parent_id points at another workspace_roots row (or NULL). The tree is
-- logical: a node's `path` can live anywhere on disk; parent_id expresses a
-- "depends on / belongs under" relationship chosen by the user, decoupled
-- from physical path nesting. NULL parent + role='project' = a top-level peer
-- project; NULL parent + role='dependency'|'tool' = mounted under the primary
-- (workspaces.cwd). Existing rows (no parent_id) keep their meaning = under
-- the primary, so no data backfill is needed.

INSERT INTO schema_version VALUES (7);

ALTER TABLE workspace_roots ADD COLUMN parent_id TEXT REFERENCES workspace_roots(id);
