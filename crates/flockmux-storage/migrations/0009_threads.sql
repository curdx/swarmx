-- 0009_threads: a workspace can hold multiple independent "directions" (threads).
--
-- Before this migration a workspace was 1 cwd = 1 chat = 1 orchestrator. Users
-- want several parallel directions inside one project (e.g. "dark-mode" and
-- "api-v2"), each with its own orchestrator + worker subtree + dual ledger, that
-- don't talk over each other AND don't overwrite each other's files.
--
-- A thread is the container for one direction:
--   - shared isolation  → thread.cwd = workspace.cwd (all threads share files;
--                          fine for serial work, the original behaviour)
--   - worktree isolation → thread.cwd = a git worktree beside the project
--                          (real file isolation; safe for parallel work)
--
-- The first thread of every workspace is the "main" thread (slug='main'),
-- created alongside the workspace and always shared (it IS the project itself).
--
-- Messages are NOT given a thread_id column: the chat is already scoped client-
-- side by "which agents belong to this view". We extend that grouping from
-- workspace to thread by tagging each AGENT with thread_id. user/system messages
-- pair with an agent on the other end, so the agent-set filter is sufficient.
--
-- Blackboard isolation is by key prefix: ledgers move from `{ws}/task.ledger.md`
-- to `{ws}/{thread_slug}/task.ledger.md`. Old workspace-level ledgers are not
-- migrated (they regenerate on next orchestrator wake).
--
-- ALTER TABLE ADD COLUMN: SQLite only adds nullable columns, so agents.thread_id
-- is nullable; a NULL thread_id means "the workspace's main thread" (legacy rows
-- and any agent spawned before this feature land here).

INSERT INTO schema_version VALUES (9);

CREATE TABLE threads (
    id           TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id),
    -- URL + blackboard-prefix identifier, unique among alive threads of a
    -- workspace. 'main' for the primary thread; 't-<rand>' for a fresh
    -- unnamed direction until the AI renames it to a real branch slug.
    slug         TEXT NOT NULL,
    -- Human label. NULL until the orchestrator auto-names the direction from
    -- the user's first message; UI shows "新方向…" while NULL.
    name         TEXT,
    -- 'shared' | 'worktree'
    isolation    TEXT NOT NULL DEFAULT 'shared',
    -- git branch backing a worktree thread (NULL for shared).
    branch       TEXT,
    -- the agent working directory for this thread.
    cwd          TEXT NOT NULL,
    -- 'ready' | 'preparing' (worktree being created) | 'failed'
    state        TEXT NOT NULL DEFAULT 'ready',
    created_at   INTEGER NOT NULL,
    deleted_at   INTEGER
);
CREATE INDEX idx_threads_workspace ON threads(workspace_id) WHERE deleted_at IS NULL;
-- slug unique per workspace among alive threads (a deleted thread frees its slug).
CREATE UNIQUE INDEX idx_threads_ws_slug_alive
    ON threads(workspace_id, slug) WHERE deleted_at IS NULL;

ALTER TABLE agents ADD COLUMN thread_id TEXT REFERENCES threads(id);
CREATE INDEX idx_agents_thread ON agents(thread_id) WHERE killed_at IS NULL;
