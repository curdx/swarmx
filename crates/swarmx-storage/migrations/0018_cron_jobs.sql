-- 0018_cron_jobs: user-defined scheduled prompts. A tokio scheduler ticks each
-- minute, matches `cron_expr` (5-field, UTC) and delivers `prompt` to the
-- workspace's orchestrator (message + wake). `last_run_at` dedups within a
-- minute and surfaces "last fired" in the UI.
CREATE TABLE cron_jobs (
    id           TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL,
    name         TEXT NOT NULL,
    cron_expr    TEXT NOT NULL,
    prompt       TEXT NOT NULL,
    enabled      INTEGER NOT NULL DEFAULT 1,
    created_at   INTEGER NOT NULL,
    last_run_at  INTEGER
);
CREATE INDEX idx_cron_jobs_ws ON cron_jobs(workspace_id);

INSERT INTO schema_version VALUES (18);
