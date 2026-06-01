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
