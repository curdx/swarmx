//! Migration runner. Single-shot for now (only 0001_init) but structured so
//! future migrations slot in as additional `include_str!` entries.
//!
//! Each migration runs inside `BEGIN IMMEDIATE` so a concurrent reader
//! cannot observe a half-applied schema (SQLite WAL allows readers during
//! writes, and our triggers + FTS5 are not atomic-without-tx).

use anyhow::{Context, Result};
use rusqlite::Connection;

const MIGRATION_0001: &str = include_str!("../migrations/0001_init.sql");
const MIGRATION_0002: &str = include_str!("../migrations/0002_pty_recordings.sql");
const MIGRATION_0003: &str = include_str!("../migrations/0003_in_reply_to.sql");
const MIGRATION_0004: &str = include_str!("../migrations/0004_workspaces.sql");
const MIGRATION_0005: &str = include_str!("../migrations/0005_workers.sql");
const MIGRATION_0006: &str = include_str!("../migrations/0006_workspace_roots.sql");
const MIGRATION_0007: &str = include_str!("../migrations/0007_workspace_root_parent.sql");
const MIGRATION_0008: &str = include_str!("../migrations/0008_blackboard_path_id.sql");
const MIGRATION_0009: &str = include_str!("../migrations/0009_threads.sql");
const MIGRATION_0010: &str = include_str!("../migrations/0010_message_thread_id.sql");
const MIGRATION_0011: &str = include_str!("../migrations/0011_worker_role_typed_handoff.sql");
const MIGRATION_0012: &str = include_str!("../migrations/0012_message_meta.sql");
const MIGRATION_0013: &str = include_str!("../migrations/0013_agent_last_activity.sql");
const MIGRATION_0014: &str = include_str!("../migrations/0014_thread_model_tier.sql");
const MIGRATION_0015: &str = include_str!("../migrations/0015_thread_reasoning_effort.sql");
const MIGRATION_0016: &str = include_str!("../migrations/0016_agent_usage.sql");
const MIGRATION_0017: &str = include_str!("../migrations/0017_worker_task_status.sql");
const MIGRATION_0018: &str = include_str!("../migrations/0018_cron_jobs.sql");
const MIGRATION_0019: &str = include_str!("../migrations/0019_goals.sql");
const MIGRATION_0020: &str = include_str!("../migrations/0020_goal_evidence.sql");
const MIGRATION_0021: &str = include_str!("../migrations/0021_thought_traces.sql");
const MIGRATION_0022: &str = include_str!("../migrations/0022_agent_last_error.sql");
const MIGRATION_0023: &str = include_str!("../migrations/0023_agent_activities.sql");

pub(crate) fn run_migrations(conn: &mut Connection) -> Result<()> {
    let current = current_version(conn).unwrap_or(0);
    tracing::debug!(current, "running flockmux-storage migrations");

    if current < 1 {
        apply(conn, 1, MIGRATION_0001).context("apply migration 0001")?;
    }
    if current < 2 {
        apply(conn, 2, MIGRATION_0002).context("apply migration 0002")?;
    }
    if current < 3 {
        apply(conn, 3, MIGRATION_0003).context("apply migration 0003")?;
    }
    if current < 4 {
        apply(conn, 4, MIGRATION_0004).context("apply migration 0004")?;
    }
    if current < 5 {
        apply(conn, 5, MIGRATION_0005).context("apply migration 0005")?;
    }
    if current < 6 {
        apply(conn, 6, MIGRATION_0006).context("apply migration 0006")?;
    }
    if current < 7 {
        apply(conn, 7, MIGRATION_0007).context("apply migration 0007")?;
    }
    if current < 8 {
        apply(conn, 8, MIGRATION_0008).context("apply migration 0008")?;
    }
    if current < 9 {
        apply(conn, 9, MIGRATION_0009).context("apply migration 0009")?;
    }
    if current < 10 {
        apply(conn, 10, MIGRATION_0010).context("apply migration 0010")?;
    }
    if current < 11 {
        apply(conn, 11, MIGRATION_0011).context("apply migration 0011")?;
    }
    if current < 12 {
        apply(conn, 12, MIGRATION_0012).context("apply migration 0012")?;
    }
    if current < 13 {
        apply(conn, 13, MIGRATION_0013).context("apply migration 0013")?;
    }
    if current < 14 {
        apply(conn, 14, MIGRATION_0014).context("apply migration 0014")?;
    }
    if current < 15 {
        apply(conn, 15, MIGRATION_0015).context("apply migration 0015")?;
    }
    if current < 16 {
        apply(conn, 16, MIGRATION_0016).context("apply migration 0016")?;
    }
    if current < 17 {
        apply(conn, 17, MIGRATION_0017).context("apply migration 0017")?;
    }
    if current < 18 {
        apply(conn, 18, MIGRATION_0018).context("apply migration 0018")?;
    }
    if current < 19 {
        apply(conn, 19, MIGRATION_0019).context("apply migration 0019")?;
    }
    if current < 20 {
        apply(conn, 20, MIGRATION_0020).context("apply migration 0020")?;
    }
    if current < 21 {
        apply(conn, 21, MIGRATION_0021).context("apply migration 0021")?;
    }
    if current < 22 {
        apply(conn, 22, MIGRATION_0022).context("apply migration 0022")?;
    }
    if current < 23 {
        apply(conn, 23, MIGRATION_0023).context("apply migration 0023")?;
    }
    Ok(())
}

fn current_version(conn: &Connection) -> Result<i64> {
    // schema_version may not exist yet (fresh DB).
    let exists: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='schema_version'",
        [],
        |row| row.get(0),
    )?;
    if exists == 0 {
        return Ok(0);
    }
    let v: i64 = conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_version",
        [],
        |row| row.get(0),
    )?;
    Ok(v)
}

fn apply(conn: &mut Connection, version: i64, sql: &str) -> Result<()> {
    let tx = conn.transaction()?;
    // execute_batch handles the multi-statement migration in one call.
    tx.execute_batch(sql)
        .with_context(|| format!("execute migration {version}"))?;
    tx.commit()?;
    Ok(())
}
