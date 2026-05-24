//! The `Store` API. Every public method hops to `tokio::spawn_blocking`
//! so callers can `.await` it from inside an axum handler without blocking
//! the runtime.

use crate::connection::Customizer;
use crate::models::{
    AgentRecord, BlackboardOpRecord, ListMessagesOpts, MessageRecord, NewAgent, NewBlackboardOp,
    NewMessage, NewRecording, RecordingRecord,
};
use crate::schema;
use anyhow::{Context, Result};
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::params;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Thread-safe handle to the SQLite store. Cheap to clone — wraps an `Arc`
/// over the r2d2 pool.
#[derive(Clone)]
pub struct Store {
    pool: Arc<Pool<SqliteConnectionManager>>,
}

impl Store {
    /// Open a store at `path`, running migrations if needed. Parent dir must
    /// already exist.
    pub async fn open(path: &Path) -> Result<Self> {
        let path: PathBuf = path.to_path_buf();
        tokio::task::spawn_blocking(move || Self::open_blocking(&path))
            .await
            .context("spawn_blocking open")?
    }

    fn open_blocking(path: &Path) -> Result<Self> {
        let manager = SqliteConnectionManager::file(path);
        // `min_idle(0)` keeps r2d2 from eagerly opening every connection at
        // `build()` time. Eager parallel opens race on the very first
        // `PRAGMA journal_mode=WAL`, producing "database is locked" noise in
        // the boot log. Lazy creation lets WAL get set by the first checkout
        // and persisted to the file before any sibling connection appears.
        let pool = Pool::builder()
            .max_size(8)
            .min_idle(Some(0))
            .connection_customizer(Box::new(Customizer))
            .build(manager)
            .context("build r2d2 pool")?;
        // Run migrations on a dedicated connection (not the pool — we want
        // exclusive access for the schema bring-up).
        let mut conn = pool.get().context("checkout for migrations")?;
        schema::run_migrations(&mut conn).context("run migrations")?;
        drop(conn);
        Ok(Self {
            pool: Arc::new(pool),
        })
    }

    // ── agents ────────────────────────────────────────────────────────────

    pub async fn record_agent_spawn(&self, rec: NewAgent) -> Result<()> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let conn = pool.get()?;
            conn.execute(
                "INSERT INTO agents (id, cli, role, workspace, spawned_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![rec.id, rec.cli, rec.role, rec.workspace, rec.spawned_at],
            )?;
            Ok(())
        })
        .await
        .context("spawn_blocking record_agent_spawn")?
    }

    pub async fn record_agent_kill(&self, id: String, at_ms: i64) -> Result<()> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let conn = pool.get()?;
            conn.execute(
                "UPDATE agents SET killed_at = ?2 WHERE id = ?1 AND killed_at IS NULL",
                params![id, at_ms],
            )?;
            Ok(())
        })
        .await
        .context("spawn_blocking record_agent_kill")?
    }

    pub async fn record_shim_ready(&self, id: String, at_ms: i64) -> Result<()> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let conn = pool.get()?;
            conn.execute(
                "UPDATE agents SET shim_ready_at = ?2 WHERE id = ?1 AND shim_ready_at IS NULL",
                params![id, at_ms],
            )?;
            Ok(())
        })
        .await
        .context("spawn_blocking record_shim_ready")?
    }

    pub async fn record_shim_exit(&self, id: String, code: i32, at_ms: i64) -> Result<()> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let conn = pool.get()?;
            conn.execute(
                "UPDATE agents SET shim_exit_at = ?2, shim_exit_code = ?3 \
                 WHERE id = ?1 AND shim_exit_at IS NULL",
                params![id, at_ms, code],
            )?;
            Ok(())
        })
        .await
        .context("spawn_blocking record_shim_exit")?
    }

    pub async fn list_agents(&self) -> Result<Vec<AgentRecord>> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || -> Result<Vec<AgentRecord>> {
            let conn = pool.get()?;
            let mut stmt = conn.prepare(
                "SELECT id, cli, role, workspace, spawned_at, killed_at, \
                        shim_ready_at, shim_exit_at, shim_exit_code \
                 FROM agents \
                 ORDER BY spawned_at ASC",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok(AgentRecord {
                    id: row.get(0)?,
                    cli: row.get(1)?,
                    role: row.get(2)?,
                    workspace: row.get(3)?,
                    spawned_at: row.get(4)?,
                    killed_at: row.get(5)?,
                    shim_ready_at: row.get(6)?,
                    shim_exit_at: row.get(7)?,
                    shim_exit_code: row.get(8)?,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
        })
        .await
        .context("spawn_blocking list_agents")?
    }

    // ── messages ─────────────────────────────────────────────────────────

    pub async fn insert_message(&self, msg: NewMessage) -> Result<MessageRecord> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || -> Result<MessageRecord> {
            let conn = pool.get()?;
            conn.execute(
                "INSERT INTO messages (from_agent, to_agent, kind, body, sent_at, in_reply_to) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    msg.from_agent,
                    msg.to_agent,
                    msg.kind,
                    msg.body,
                    msg.sent_at,
                    msg.in_reply_to
                ],
            )?;
            let id = conn.last_insert_rowid();
            Ok(MessageRecord {
                id,
                from_agent: msg.from_agent,
                to_agent: msg.to_agent,
                kind: msg.kind,
                body: msg.body,
                sent_at: msg.sent_at,
                delivered_at: None,
                read_at: None,
                in_reply_to: msg.in_reply_to,
            })
        })
        .await
        .context("spawn_blocking insert_message")?
    }

    pub async fn list_messages(&self, opts: ListMessagesOpts) -> Result<Vec<MessageRecord>> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || -> Result<Vec<MessageRecord>> {
            let conn = pool.get()?;
            // Build WHERE dynamically. We bind via positional params so we
            // can keep the cap on injection surface tight (always-string
            // values, never interpolated).
            let mut wheres: Vec<&str> = Vec::new();
            let mut bound: Vec<rusqlite::types::Value> = Vec::new();
            if let Some(to) = &opts.to_agent {
                wheres.push("to_agent = ?");
                bound.push(to.clone().into());
            }
            if let Some(from) = &opts.from_agent {
                wheres.push("from_agent = ?");
                bound.push(from.clone().into());
            }
            if opts.only_undelivered {
                wheres.push("delivered_at IS NULL");
            }
            let where_sql = if wheres.is_empty() {
                String::new()
            } else {
                format!("WHERE {}", wheres.join(" AND "))
            };
            let limit = if opts.limit <= 0 { 200 } else { opts.limit };
            bound.push(limit.into());

            let sql = format!(
                "SELECT id, from_agent, to_agent, kind, body, sent_at, delivered_at, read_at, in_reply_to \
                 FROM messages \
                 {where_sql} \
                 ORDER BY id DESC \
                 LIMIT ?"
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(bound.iter()), |row| {
                Ok(MessageRecord {
                    id: row.get(0)?,
                    from_agent: row.get(1)?,
                    to_agent: row.get(2)?,
                    kind: row.get(3)?,
                    body: row.get(4)?,
                    sent_at: row.get(5)?,
                    delivered_at: row.get(6)?,
                    read_at: row.get(7)?,
                    in_reply_to: row.get(8)?,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
        })
        .await
        .context("spawn_blocking list_messages")?
    }

    pub async fn search_messages(&self, query: String) -> Result<Vec<MessageRecord>> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || -> Result<Vec<MessageRecord>> {
            let conn = pool.get()?;
            // Join messages_fts → messages on rowid; order by FTS rank.
            let mut stmt = conn.prepare(
                "SELECT m.id, m.from_agent, m.to_agent, m.kind, m.body, m.sent_at, \
                        m.delivered_at, m.read_at, m.in_reply_to \
                 FROM messages_fts \
                 JOIN messages m ON m.id = messages_fts.rowid \
                 WHERE messages_fts MATCH ?1 \
                 ORDER BY rank \
                 LIMIT 200",
            )?;
            let rows = stmt.query_map(params![query], |row| {
                Ok(MessageRecord {
                    id: row.get(0)?,
                    from_agent: row.get(1)?,
                    to_agent: row.get(2)?,
                    kind: row.get(3)?,
                    body: row.get(4)?,
                    sent_at: row.get(5)?,
                    delivered_at: row.get(6)?,
                    read_at: row.get(7)?,
                    in_reply_to: row.get(8)?,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
        })
        .await
        .context("spawn_blocking search_messages")?
    }

    pub async fn mark_delivered(&self, ids: Vec<i64>, at_ms: i64) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let mut conn = pool.get()?;
            let tx = conn.transaction()?;
            {
                let mut stmt = tx.prepare(
                    "UPDATE messages SET delivered_at = ?1 \
                     WHERE id = ?2 AND delivered_at IS NULL",
                )?;
                for id in &ids {
                    stmt.execute(params![at_ms, id])?;
                }
            }
            tx.commit()?;
            Ok(())
        })
        .await
        .context("spawn_blocking mark_delivered")?
    }

    /// Mark messages as read on behalf of `to_agent`. Refuses cross-agent
    /// marks (the WHERE `to_agent = ?` clause) and is idempotent
    /// (`read_at IS NULL`). Returns the ids actually updated this call so
    /// the swarm can broadcast a tight `MessageRead` event.
    pub async fn mark_read(
        &self,
        ids: Vec<i64>,
        to_agent: String,
        at_ms: i64,
    ) -> Result<Vec<i64>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || -> Result<Vec<i64>> {
            let mut conn = pool.get()?;
            let tx = conn.transaction()?;
            let mut marked = Vec::with_capacity(ids.len());
            {
                let mut stmt = tx.prepare(
                    "UPDATE messages SET read_at = ?1 \
                     WHERE id = ?2 AND to_agent = ?3 AND read_at IS NULL \
                     RETURNING id",
                )?;
                for id in &ids {
                    let mut rows = stmt.query(params![at_ms, id, to_agent])?;
                    if let Some(row) = rows.next()? {
                        marked.push(row.get::<_, i64>(0)?);
                    }
                }
            }
            tx.commit()?;
            Ok(marked)
        })
        .await
        .context("spawn_blocking mark_read")?
    }

    /// Count messages for `to_agent` that have not yet been read.
    pub async fn count_unread(&self, to_agent: String) -> Result<i64> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || -> Result<i64> {
            let conn = pool.get()?;
            let n: i64 = conn.query_row(
                "SELECT COUNT(*) FROM messages WHERE to_agent = ?1 AND read_at IS NULL",
                params![to_agent],
                |row| row.get(0),
            )?;
            Ok(n)
        })
        .await
        .context("spawn_blocking count_unread")?
    }

    /// M6f: atomically find all unread `kind="wake"` messages for `to_agent`,
    /// mark them as read, and return their ids. This is `wake_check`'s
    /// primary signal — it MUST be atomic (single transaction) so a wake
    /// arriving between the SELECT and the UPDATE wouldn't get lost.
    ///
    /// Why this is separate from generic `mark_read`: wake messages are
    /// system triggers, not human-readable mail. `swarm_list_messages`
    /// deliberately skips marking them read (see tools.rs comment), so
    /// nothing else touches them. This method is the ONLY thing that
    /// closes the loop, ensuring each wake fires `wake_check` exactly
    /// once.
    pub async fn consume_wakes(&self, to_agent: String, at_ms: i64) -> Result<Vec<i64>> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || -> Result<Vec<i64>> {
            let mut conn = pool.get()?;
            let tx = conn.transaction()?;
            let marked: Vec<i64> = {
                let mut stmt = tx.prepare(
                    "UPDATE messages SET read_at = ?1 \
                     WHERE to_agent = ?2 AND kind = 'wake' AND read_at IS NULL \
                     RETURNING id",
                )?;
                let rows = stmt
                    .query_map(params![at_ms, to_agent], |row| row.get::<_, i64>(0))?;
                rows.collect::<rusqlite::Result<Vec<i64>>>()?
            };
            tx.commit()?;
            Ok(marked)
        })
        .await
        .context("spawn_blocking consume_wakes")?
    }

    // ── blackboard ───────────────────────────────────────────────────────

    pub async fn insert_blackboard_op(&self, op: NewBlackboardOp) -> Result<BlackboardOpRecord> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || -> Result<BlackboardOpRecord> {
            let conn = pool.get()?;
            conn.execute(
                "INSERT INTO blackboard_ops (agent_id, op, path, content, sha256, at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![op.agent_id, op.op, op.path, op.content, op.sha256, op.at],
            )?;
            let id = conn.last_insert_rowid();
            Ok(BlackboardOpRecord {
                id,
                agent_id: op.agent_id,
                op: op.op,
                path: op.path,
                content: op.content,
                sha256: op.sha256,
                at: op.at,
            })
        })
        .await
        .context("spawn_blocking insert_blackboard_op")?
    }

    /// Returns the latest op for each distinct path. If `path` is `Some`,
    /// only that path's history is returned (most-recent first).
    pub async fn list_blackboard_ops(
        &self,
        path: Option<String>,
    ) -> Result<Vec<BlackboardOpRecord>> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || -> Result<Vec<BlackboardOpRecord>> {
            let conn = pool.get()?;
            let (sql, bound): (&str, Vec<rusqlite::types::Value>) = match &path {
                Some(p) => (
                    "SELECT id, agent_id, op, path, content, sha256, at \
                     FROM blackboard_ops WHERE path = ?1 \
                     ORDER BY id DESC LIMIT 200",
                    vec![p.clone().into()],
                ),
                None => (
                    // latest per path
                    "SELECT b.id, b.agent_id, b.op, b.path, b.content, b.sha256, b.at \
                     FROM blackboard_ops b \
                     JOIN ( \
                         SELECT path, MAX(id) AS max_id FROM blackboard_ops GROUP BY path \
                     ) latest ON latest.max_id = b.id \
                     ORDER BY b.at DESC LIMIT 200",
                    Vec::new(),
                ),
            };
            let mut stmt = conn.prepare(sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(bound.iter()), |row| {
                Ok(BlackboardOpRecord {
                    id: row.get(0)?,
                    agent_id: row.get(1)?,
                    op: row.get(2)?,
                    path: row.get(3)?,
                    content: row.get(4)?,
                    sha256: row.get(5)?,
                    at: row.get(6)?,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
        })
        .await
        .context("spawn_blocking list_blackboard_ops")?
    }

    // ── pty recordings ───────────────────────────────────────────────────

    pub async fn record_recording_start(&self, rec: NewRecording) -> Result<()> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let conn = pool.get()?;
            conn.execute(
                "INSERT INTO pty_recordings (id, agent_id, path, started_at, cols, rows) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![rec.id, rec.agent_id, rec.path, rec.started_at, rec.cols, rec.rows],
            )?;
            Ok(())
        })
        .await
        .context("spawn_blocking record_recording_start")?
    }

    pub async fn record_recording_finalize(
        &self,
        id: String,
        finalized_at: i64,
        duration_ms: i64,
        last_seq: i64,
    ) -> Result<()> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let conn = pool.get()?;
            conn.execute(
                "UPDATE pty_recordings \
                 SET finalized_at = ?2, duration_ms = ?3, last_seq = ?4 \
                 WHERE id = ?1 AND finalized_at IS NULL",
                params![id, finalized_at, duration_ms, last_seq],
            )?;
            Ok(())
        })
        .await
        .context("spawn_blocking record_recording_finalize")?
    }

    /// Mark agents whose PTY died with the server (rows with NULL
    /// `killed_at`) as killed at `at_ms`. Companion to
    /// `mark_orphan_recordings_finalized`: after a crash restart the
    /// in-memory registry is empty, but `/api/agent` still returns these
    /// rows — without settling them, the UI's reattach reconnects WS to a
    /// non-existent PTY and shows "WS closed (code 1005)" forever.
    pub async fn mark_orphan_agents_killed(&self, at_ms: i64) -> Result<usize> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || -> Result<usize> {
            let conn = pool.get()?;
            let n = conn.execute(
                "UPDATE agents SET killed_at = ?1 WHERE killed_at IS NULL",
                params![at_ms],
            )?;
            Ok(n)
        })
        .await
        .context("spawn_blocking mark_orphan_agents_killed")?
    }

    /// Finalize any recording rows left in the "live" state — i.e. the
    /// previous server died (crash, SIGKILL, container restart) before its
    /// recorder task could call `record_recording_finalize`. Without this,
    /// the panel shows orphans as "● live" forever. We mark `finalized_at`
    /// so the row visibly settles; `duration_ms` and `last_seq` stay NULL
    /// since we cannot recover them accurately from a half-flushed .cast.
    pub async fn mark_orphan_recordings_finalized(&self, at_ms: i64) -> Result<usize> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || -> Result<usize> {
            let conn = pool.get()?;
            let n = conn.execute(
                "UPDATE pty_recordings SET finalized_at = ?1 \
                 WHERE finalized_at IS NULL",
                params![at_ms],
            )?;
            Ok(n)
        })
        .await
        .context("spawn_blocking mark_orphan_recordings_finalized")?
    }

    pub async fn list_recordings(&self, agent_id: Option<String>) -> Result<Vec<RecordingRecord>> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || -> Result<Vec<RecordingRecord>> {
            let conn = pool.get()?;
            let (sql, bound): (&str, Vec<rusqlite::types::Value>) = match &agent_id {
                Some(a) => (
                    "SELECT id, agent_id, path, started_at, finalized_at, duration_ms, \
                            cols, rows, last_seq \
                     FROM pty_recordings WHERE agent_id = ?1 \
                     ORDER BY started_at DESC LIMIT 200",
                    vec![a.clone().into()],
                ),
                None => (
                    "SELECT id, agent_id, path, started_at, finalized_at, duration_ms, \
                            cols, rows, last_seq \
                     FROM pty_recordings \
                     ORDER BY started_at DESC LIMIT 200",
                    Vec::new(),
                ),
            };
            let mut stmt = conn.prepare(sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(bound.iter()), |row| {
                Ok(RecordingRecord {
                    id: row.get(0)?,
                    agent_id: row.get(1)?,
                    path: row.get(2)?,
                    started_at: row.get(3)?,
                    finalized_at: row.get(4)?,
                    duration_ms: row.get(5)?,
                    cols: row.get(6)?,
                    rows: row.get(7)?,
                    last_seq: row.get(8)?,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
        })
        .await
        .context("spawn_blocking list_recordings")?
    }

    pub async fn get_recording(&self, id: String) -> Result<Option<RecordingRecord>> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || -> Result<Option<RecordingRecord>> {
            let conn = pool.get()?;
            let mut stmt = conn.prepare(
                "SELECT id, agent_id, path, started_at, finalized_at, duration_ms, \
                        cols, rows, last_seq \
                 FROM pty_recordings WHERE id = ?1",
            )?;
            let mut rows = stmt.query(params![id])?;
            if let Some(row) = rows.next()? {
                Ok(Some(RecordingRecord {
                    id: row.get(0)?,
                    agent_id: row.get(1)?,
                    path: row.get(2)?,
                    started_at: row.get(3)?,
                    finalized_at: row.get(4)?,
                    duration_ms: row.get(5)?,
                    cols: row.get(6)?,
                    rows: row.get(7)?,
                    last_seq: row.get(8)?,
                }))
            } else {
                Ok(None)
            }
        })
        .await
        .context("spawn_blocking get_recording")?
    }

    pub async fn search_blackboard(&self, query: String) -> Result<Vec<BlackboardOpRecord>> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || -> Result<Vec<BlackboardOpRecord>> {
            let conn = pool.get()?;
            let mut stmt = conn.prepare(
                "SELECT b.id, b.agent_id, b.op, b.path, b.content, b.sha256, b.at \
                 FROM blackboard_fts \
                 JOIN blackboard_ops b ON b.id = blackboard_fts.rowid \
                 WHERE blackboard_fts MATCH ?1 \
                 ORDER BY rank \
                 LIMIT 200",
            )?;
            let rows = stmt.query_map(params![query], |row| {
                Ok(BlackboardOpRecord {
                    id: row.get(0)?,
                    agent_id: row.get(1)?,
                    op: row.get(2)?,
                    path: row.get(3)?,
                    content: row.get(4)?,
                    sha256: row.get(5)?,
                    at: row.get(6)?,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
        })
        .await
        .context("spawn_blocking search_blackboard")?
    }
}
