//! The `Store` API. Every public method hops to `tokio::spawn_blocking`
//! so callers can `.await` it from inside an axum handler without blocking
//! the runtime.

use crate::connection::Customizer;
use crate::models::{
    AgentRecord, BlackboardOpRecord, ListMessagesOpts, MessageRecord, NewAgent, NewBlackboardOp,
    NewMessage, NewRecording, NewSpellRun, NewWorker, NewWorkspace, NewWorkspaceRoot,
    RecordingRecord, SpellRunRecord, WorkerRecord, WorkspaceRecord, WorkspaceRootRecord,
};
use crate::schema;
use anyhow::{Context, Result};
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::{params, Connection, Error as SqliteError, ErrorCode};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// True if `e` is a `SQLITE_BUSY` / `SQLITE_LOCKED` failure — the only
/// errors `with_busy_retry` re-runs on.
fn is_busy(e: &SqliteError) -> bool {
    matches!(
        e,
        SqliteError::SqliteFailure(f, _)
            if matches!(f.code, ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked)
    )
}

/// Run a DB op, re-running it on SQLITE_BUSY / SQLITE_LOCKED that `busy_timeout`
/// didn't absorb (WAL checkpoint / snapshot edge cases). Safe to retry: a BUSY
/// is returned BEFORE the statement/transaction takes effect, so no partial
/// write is left behind. Up to 5 attempts, backoff 10/20/40/80 ms.
///
/// Owns the pool checkout (a fresh connection per attempt) so the `op` closure
/// receives a `&mut Connection` and never touches `pool.get()` directly.
fn with_busy_retry<T>(
    pool: &Pool<SqliteConnectionManager>,
    mut op: impl FnMut(&mut Connection) -> rusqlite::Result<T>,
) -> Result<T> {
    const MAX_ATTEMPTS: u32 = 5;
    let mut attempt: u32 = 0;
    loop {
        let mut conn = pool.get()?;
        match op(&mut conn) {
            Ok(v) => return Ok(v),
            Err(e) if is_busy(&e) && attempt + 1 < MAX_ATTEMPTS => {
                std::thread::sleep(std::time::Duration::from_millis(10u64 << attempt));
                attempt += 1;
            }
            Err(e) => return Err(e.into()),
        }
    }
}

/// What a single [`Store::prune_expired`] pass removed. All counts are rows
/// actually deleted; `recording_files_removed` is the subset of pruned
/// recordings whose `.cast` file was also unlinked from disk (best-effort).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PruneStats {
    pub blackboard_ops: usize,
    pub messages: usize,
    pub recordings: usize,
    pub recording_files_removed: usize,
}

impl PruneStats {
    /// True if nothing was deleted — lets the caller skip a log line.
    pub fn is_empty(&self) -> bool {
        self.blackboard_ops == 0 && self.messages == 0 && self.recordings == 0
    }
}

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
        tokio::task::spawn_blocking(move || with_busy_retry(&pool, |conn| -> rusqlite::Result<()> {
            conn.execute(
                "INSERT INTO agents (id, cli, role, workspace, spawned_at, workspace_id, spell_run_id) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    rec.id,
                    rec.cli,
                    rec.role,
                    rec.workspace,
                    rec.spawned_at,
                    rec.workspace_id,
                    rec.spell_run_id,
                ],
            )?;
            Ok(())
        }))
        .await
        .context("spawn_blocking record_agent_spawn")?
    }

    pub async fn record_agent_kill(&self, id: String, at_ms: i64) -> Result<()> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || with_busy_retry(&pool, |conn| -> rusqlite::Result<()> {
            conn.execute(
                "UPDATE agents SET killed_at = ?2 WHERE id = ?1 AND killed_at IS NULL",
                params![id, at_ms],
            )?;
            Ok(())
        }))
        .await
        .context("spawn_blocking record_agent_kill")?
    }

    pub async fn record_shim_ready(&self, id: String, at_ms: i64) -> Result<()> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || with_busy_retry(&pool, |conn| -> rusqlite::Result<()> {
            conn.execute(
                "UPDATE agents SET shim_ready_at = ?2 WHERE id = ?1 AND shim_ready_at IS NULL",
                params![id, at_ms],
            )?;
            Ok(())
        }))
        .await
        .context("spawn_blocking record_shim_ready")?
    }

    pub async fn record_shim_exit(&self, id: String, code: i32, at_ms: i64) -> Result<()> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || with_busy_retry(&pool, |conn| -> rusqlite::Result<()> {
            conn.execute(
                "UPDATE agents SET shim_exit_at = ?2, shim_exit_code = ?3 \
                 WHERE id = ?1 AND shim_exit_at IS NULL",
                params![id, at_ms, code],
            )?;
            Ok(())
        }))
        .await
        .context("spawn_blocking record_shim_exit")?
    }

    pub async fn list_agents(&self) -> Result<Vec<AgentRecord>> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || with_busy_retry(&pool, |conn| -> rusqlite::Result<Vec<AgentRecord>> {
            let mut stmt = conn.prepare(
                "SELECT id, cli, role, workspace, spawned_at, killed_at, \
                        shim_ready_at, shim_exit_at, shim_exit_code, \
                        workspace_id, spell_run_id \
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
                    workspace_id: row.get(9)?,
                    spell_run_id: row.get(10)?,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
        }))
        .await
        .context("spawn_blocking list_agents")?
    }

    // ── messages ─────────────────────────────────────────────────────────

    pub async fn insert_message(&self, msg: NewMessage) -> Result<MessageRecord> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || with_busy_retry(&pool, |conn| -> rusqlite::Result<MessageRecord> {
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
            // Clone (don't move) out of the captured `msg`: the retry closure
            // is `FnMut`, so it may run more than once and can't consume it.
            Ok(MessageRecord {
                id,
                from_agent: msg.from_agent.clone(),
                to_agent: msg.to_agent.clone(),
                kind: msg.kind.clone(),
                body: msg.body.clone(),
                sent_at: msg.sent_at,
                delivered_at: None,
                read_at: None,
                in_reply_to: msg.in_reply_to,
            })
        }))
        .await
        .context("spawn_blocking insert_message")?
    }

    pub async fn list_messages(&self, opts: ListMessagesOpts) -> Result<Vec<MessageRecord>> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || with_busy_retry(&pool, |conn| -> rusqlite::Result<Vec<MessageRecord>> {
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
            rows.collect::<rusqlite::Result<Vec<_>>>()
        }))
        .await
        .context("spawn_blocking list_messages")?
    }

    pub async fn search_messages(&self, query: String) -> Result<Vec<MessageRecord>> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || with_busy_retry(&pool, |conn| -> rusqlite::Result<Vec<MessageRecord>> {
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
            rows.collect::<rusqlite::Result<Vec<_>>>()
        }))
        .await
        .context("spawn_blocking search_messages")?
    }

    pub async fn mark_delivered(&self, ids: Vec<i64>, at_ms: i64) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || with_busy_retry(&pool, |conn| -> rusqlite::Result<()> {
            // Single statement instead of N (one execute per id). `?` for at_ms
            // then one per id; params bound positionally via params_from_iter.
            let placeholders = vec!["?"; ids.len()].join(",");
            let sql = format!(
                "UPDATE messages SET delivered_at = ? \
                 WHERE delivered_at IS NULL AND id IN ({placeholders})"
            );
            let mut binds: Vec<rusqlite::types::Value> = Vec::with_capacity(ids.len() + 1);
            binds.push(at_ms.into());
            binds.extend(ids.iter().map(|id| (*id).into()));
            conn.execute(&sql, rusqlite::params_from_iter(binds.iter()))?;
            Ok(())
        }))
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
        tokio::task::spawn_blocking(move || with_busy_retry(&pool, |conn| -> rusqlite::Result<Vec<i64>> {
            // Single UPDATE ... RETURNING instead of N round-trips. Params bound
            // positionally: at_ms, to_agent, then one per id.
            let placeholders = vec!["?"; ids.len()].join(",");
            let sql = format!(
                "UPDATE messages SET read_at = ? \
                 WHERE read_at IS NULL AND to_agent = ? AND id IN ({placeholders}) \
                 RETURNING id"
            );
            let mut binds: Vec<rusqlite::types::Value> = Vec::with_capacity(ids.len() + 2);
            binds.push(at_ms.into());
            binds.push(to_agent.clone().into());
            binds.extend(ids.iter().map(|id| (*id).into()));
            let mut stmt = conn.prepare(&sql)?;
            let rows =
                stmt.query_map(rusqlite::params_from_iter(binds.iter()), |r| r.get::<_, i64>(0))?;
            rows.collect::<rusqlite::Result<Vec<i64>>>()
        }))
        .await
        .context("spawn_blocking mark_read")?
    }

    /// Count messages for `to_agent` that have not yet been read.
    pub async fn count_unread(&self, to_agent: String) -> Result<i64> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || with_busy_retry(&pool, |conn| -> rusqlite::Result<i64> {
            let n: i64 = conn.query_row(
                "SELECT COUNT(*) FROM messages WHERE to_agent = ?1 AND read_at IS NULL",
                params![to_agent],
                |row| row.get(0),
            )?;
            Ok(n)
        }))
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
        tokio::task::spawn_blocking(move || with_busy_retry(&pool, |conn| -> rusqlite::Result<Vec<i64>> {
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
        }))
        .await
        .context("spawn_blocking consume_wakes")?
    }

    // ── blackboard ───────────────────────────────────────────────────────

    pub async fn insert_blackboard_op(&self, op: NewBlackboardOp) -> Result<BlackboardOpRecord> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || with_busy_retry(&pool, |conn| -> rusqlite::Result<BlackboardOpRecord> {
            conn.execute(
                "INSERT INTO blackboard_ops (agent_id, op, path, content, sha256, at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![op.agent_id, op.op, op.path, op.content, op.sha256, op.at],
            )?;
            let id = conn.last_insert_rowid();
            // Clone (don't move) out of the captured `op`: the retry closure
            // is `FnMut`, so it may run more than once and can't consume it.
            Ok(BlackboardOpRecord {
                id,
                agent_id: op.agent_id.clone(),
                op: op.op.clone(),
                path: op.path.clone(),
                content: op.content.clone(),
                sha256: op.sha256.clone(),
                at: op.at,
            })
        }))
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
        tokio::task::spawn_blocking(move || with_busy_retry(&pool, |conn| -> rusqlite::Result<Vec<BlackboardOpRecord>> {
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
            rows.collect::<rusqlite::Result<Vec<_>>>()
        }))
        .await
        .context("spawn_blocking list_blackboard_ops")?
    }

    // ── pty recordings ───────────────────────────────────────────────────

    pub async fn record_recording_start(&self, rec: NewRecording) -> Result<()> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || with_busy_retry(&pool, |conn| -> rusqlite::Result<()> {
            conn.execute(
                "INSERT INTO pty_recordings (id, agent_id, path, started_at, cols, rows) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![rec.id, rec.agent_id, rec.path, rec.started_at, rec.cols, rec.rows],
            )?;
            Ok(())
        }))
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
        tokio::task::spawn_blocking(move || with_busy_retry(&pool, |conn| -> rusqlite::Result<()> {
            conn.execute(
                "UPDATE pty_recordings \
                 SET finalized_at = ?2, duration_ms = ?3, last_seq = ?4 \
                 WHERE id = ?1 AND finalized_at IS NULL",
                params![id, finalized_at, duration_ms, last_seq],
            )?;
            Ok(())
        }))
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
        tokio::task::spawn_blocking(move || with_busy_retry(&pool, |conn| -> rusqlite::Result<usize> {
            let n = conn.execute(
                "UPDATE agents SET killed_at = ?1 WHERE killed_at IS NULL",
                params![at_ms],
            )?;
            Ok(n)
        }))
        .await
        .context("spawn_blocking mark_orphan_agents_killed")?
    }

    /// Finalize any recording rows left in the "live" state — i.e. the
    /// previous server died (crash, SIGKILL, container restart) before its
    /// recorder task could call `record_recording_finalize`. Without this,
    /// the panel shows orphans as "● live" forever.
    ///
    /// We also backfill `duration_ms` from the wall-clock span
    /// (`finalized_at - started_at`). It's not the exact .cast playback
    /// length (the file may be half-flushed), but it's a sane approximation
    /// — far better than leaving it NULL, which rendered every restart-orphan
    /// recording with no duration in the Replays list. `last_seq` stays NULL
    /// (the reattach path treats NULL as "replay from head"). Only fill rows
    /// that don't already have a duration so a genuinely-finalized row isn't
    /// clobbered.
    pub async fn mark_orphan_recordings_finalized(&self, at_ms: i64) -> Result<usize> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || with_busy_retry(&pool, |conn| -> rusqlite::Result<usize> {
            let n = conn.execute(
                "UPDATE pty_recordings \
                 SET finalized_at = ?1, \
                     duration_ms = CASE \
                       WHEN duration_ms IS NOT NULL THEN duration_ms \
                       WHEN ?1 > started_at THEN ?1 - started_at \
                       ELSE 0 END \
                 WHERE finalized_at IS NULL",
                params![at_ms],
            )?;
            Ok(n)
        }))
        .await
        .context("spawn_blocking mark_orphan_recordings_finalized")?
    }

    pub async fn list_recordings(&self, agent_id: Option<String>) -> Result<Vec<RecordingRecord>> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || with_busy_retry(&pool, |conn| -> rusqlite::Result<Vec<RecordingRecord>> {
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
            rows.collect::<rusqlite::Result<Vec<_>>>()
        }))
        .await
        .context("spawn_blocking list_recordings")?
    }

    pub async fn get_recording(&self, id: String) -> Result<Option<RecordingRecord>> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || with_busy_retry(&pool, |conn| -> rusqlite::Result<Option<RecordingRecord>> {
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
        }))
        .await
        .context("spawn_blocking get_recording")?
    }

    // ── retention / prune (F5) ───────────────────────────────────────────

    /// Delete rows older than `cutoff_ms` from the three append-only tables,
    /// preserving everything still load-bearing. Returns what was removed.
    ///
    /// Safety constraints (see the audit's F5):
    /// - **blackboard_ops**: only superseded history (`at < cutoff` AND there
    ///   exists a newer row for the same path) is deleted. The latest row per
    ///   path is ALWAYS kept regardless of age — `list_blackboard_ops(None)`
    ///   (discovery) and `reconcile_oplog_from_disk` both read latest-per-path,
    ///   so dropping it would make the key vanish. The FTS index is kept in
    ///   sync by the `blackboard_ad` AFTER DELETE trigger.
    /// - **messages**: only consumed wakes (`kind='wake' AND read_at NOT NULL`)
    ///   and delivered+read normal messages are deleted. Un-consumed wakes
    ///   (`read_at IS NULL`) are NEVER deleted — the WakeCoordinator depends on
    ///   them. Rows still referenced by another message's `in_reply_to` are
    ///   skipped so the delete can't trip the (immediate) FK constraint;
    ///   they age out on a later pass once their children are gone.
    /// - **pty_recordings**: only finalized rows (`finalized_at NOT NULL`) old
    ///   enough are deleted, and their `.cast` file is unlinked best-effort.
    ///   Live recordings are left alone.
    ///
    /// Idempotent and crash-safe: the three deletes run in one transaction,
    /// and a partial run just removes fewer rows next time. After committing
    /// we checkpoint+truncate the WAL and `PRAGMA optimize` (best-effort) so
    /// freed pages don't leave the file bloated.
    pub async fn prune_expired(&self, cutoff_ms: i64) -> Result<PruneStats> {
        let pool = self.pool.clone();
        let (mut stats, files): (PruneStats, Vec<String>) =
            tokio::task::spawn_blocking(move || -> Result<(PruneStats, Vec<String>)> {
                with_busy_retry(&pool, |conn| -> rusqlite::Result<(PruneStats, Vec<String>)> {
                    let tx = conn.transaction()?;

                    // Collect .cast paths BEFORE deleting the rows so we can
                    // unlink them after the tx commits.
                    let files: Vec<String> = {
                        let mut stmt = tx.prepare(
                            "SELECT path FROM pty_recordings \
                             WHERE finalized_at IS NOT NULL AND started_at < ?1",
                        )?;
                        let rows = stmt
                            .query_map(params![cutoff_ms], |r| r.get::<_, String>(0))?;
                        rows.collect::<rusqlite::Result<Vec<_>>>()?
                    };

                    let recordings = tx.execute(
                        "DELETE FROM pty_recordings \
                         WHERE finalized_at IS NOT NULL AND started_at < ?1",
                        params![cutoff_ms],
                    )?;

                    // Superseded blackboard history only — never the latest
                    // row for a path (id < MAX(id) for that path).
                    let blackboard_ops = tx.execute(
                        "DELETE FROM blackboard_ops \
                         WHERE at < ?1 \
                           AND id < (SELECT MAX(id) FROM blackboard_ops b2 \
                                     WHERE b2.path = blackboard_ops.path)",
                        params![cutoff_ms],
                    )?;

                    // Consumed wakes + delivered/read normal messages, and only
                    // ones not referenced as a thread parent (FK-safe).
                    let messages = tx.execute(
                        "DELETE FROM messages \
                         WHERE sent_at < ?1 \
                           AND id NOT IN ( \
                               SELECT in_reply_to FROM messages \
                               WHERE in_reply_to IS NOT NULL) \
                           AND ( \
                               (kind = 'wake' AND read_at IS NOT NULL) \
                            OR (kind != 'wake' AND delivered_at IS NOT NULL \
                                AND read_at IS NOT NULL))",
                        params![cutoff_ms],
                    )?;

                    tx.commit()?;
                    Ok((
                        PruneStats {
                            blackboard_ops,
                            messages,
                            recordings,
                            recording_files_removed: 0,
                        },
                        files,
                    ))
                })
            })
            .await
            .context("spawn_blocking prune_expired")??;

        // Unlink pruned .cast files (filesystem, outside the DB tx).
        let mut removed = 0usize;
        for f in &files {
            match std::fs::remove_file(f) {
                Ok(()) => removed += 1,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => tracing::warn!(path = %f, ?e, "prune: failed to unlink .cast file"),
            }
        }
        stats.recording_files_removed = removed;

        // Post-prune hygiene (best-effort): bound the WAL and refresh planner
        // stats. Not part of the prune contract — failures are logged, ignored.
        if !stats.is_empty() {
            let pool2 = self.pool.clone();
            let _ = tokio::task::spawn_blocking(move || {
                if let Ok(conn) = pool2.get() {
                    if let Err(e) =
                        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); PRAGMA optimize;")
                    {
                        tracing::warn!(?e, "prune: post-prune WAL checkpoint/optimize failed");
                    }
                }
            })
            .await;
        }

        Ok(stats)
    }

    // ── workspaces ───────────────────────────────────────────────────────

    /// Insert a workspace. `id` is a hex-encoded random 16-byte value
    /// generated server-side via SQLite's `randomblob` so the storage
    /// crate stays uuid-free. `slug` is the first 8 hex chars of the same
    /// blob — used by the frontend as the URL identifier `/chat/:slug`.
    /// Returns the persisted row (with the generated id / slug / timestamps).
    pub async fn create_workspace(
        &self,
        rec: NewWorkspace,
        created_at: i64,
    ) -> Result<WorkspaceRecord> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || with_busy_retry(&pool, |conn| -> rusqlite::Result<WorkspaceRecord> {
            // INSERT with deterministic generation: lower(hex(randomblob(16)))
            // = 32-char id, substr(...,1,8) = the slug. RETURNING gives us
            // both back so the handler doesn't have to query again.
            let mut stmt = conn.prepare(
                "INSERT INTO workspaces (id, slug, name, cwd, accent, created_at) \
                 VALUES (lower(hex(randomblob(16))), \
                         substr(lower(hex(randomblob(16))), 1, 8), \
                         ?1, ?2, ?3, ?4) \
                 RETURNING id, slug, name, cwd, accent, created_at, deleted_at",
            )?;
            let mut rows = stmt.query(params![rec.name, rec.cwd, rec.accent, created_at])?;
            let row = rows.next()?.ok_or(rusqlite::Error::QueryReturnedNoRows)?;
            Ok(WorkspaceRecord {
                id: row.get(0)?,
                slug: row.get(1)?,
                name: row.get(2)?,
                cwd: row.get(3)?,
                accent: row.get(4)?,
                created_at: row.get(5)?,
                deleted_at: row.get(6)?,
            })
        }))
        .await
        .context("spawn_blocking create_workspace")?
    }

    /// Return all workspaces, optionally including soft-deleted ones.
    /// Ordered by creation time descending (newest first) so the UI's
    /// left nav puts fresh work at the top.
    pub async fn list_workspaces(&self, include_deleted: bool) -> Result<Vec<WorkspaceRecord>> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || with_busy_retry(&pool, |conn| -> rusqlite::Result<Vec<WorkspaceRecord>> {
            let sql = if include_deleted {
                "SELECT id, slug, name, cwd, accent, created_at, deleted_at \
                 FROM workspaces \
                 ORDER BY created_at DESC"
            } else {
                "SELECT id, slug, name, cwd, accent, created_at, deleted_at \
                 FROM workspaces WHERE deleted_at IS NULL \
                 ORDER BY created_at DESC"
            };
            let mut stmt = conn.prepare(sql)?;
            let rows = stmt.query_map([], |row| {
                Ok(WorkspaceRecord {
                    id: row.get(0)?,
                    slug: row.get(1)?,
                    name: row.get(2)?,
                    cwd: row.get(3)?,
                    accent: row.get(4)?,
                    created_at: row.get(5)?,
                    deleted_at: row.get(6)?,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
        }))
        .await
        .context("spawn_blocking list_workspaces")?
    }

    /// Look up a single workspace by its primary key. Returns `None` if
    /// not found (including soft-deleted rows — callers that care should
    /// inspect `deleted_at`).
    pub async fn get_workspace_by_id(&self, id: String) -> Result<Option<WorkspaceRecord>> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || with_busy_retry(&pool, |conn| -> rusqlite::Result<Option<WorkspaceRecord>> {
            let mut stmt = conn.prepare(
                "SELECT id, slug, name, cwd, accent, created_at, deleted_at \
                 FROM workspaces WHERE id = ?1",
            )?;
            let mut rows = stmt.query(params![id])?;
            if let Some(row) = rows.next()? {
                Ok(Some(WorkspaceRecord {
                    id: row.get(0)?,
                    slug: row.get(1)?,
                    name: row.get(2)?,
                    cwd: row.get(3)?,
                    accent: row.get(4)?,
                    created_at: row.get(5)?,
                    deleted_at: row.get(6)?,
                }))
            } else {
                Ok(None)
            }
        }))
        .await
        .context("spawn_blocking get_workspace_by_id")?
    }

    /// Mark a workspace deleted. Idempotent — re-deleting a row leaves
    /// the existing `deleted_at` untouched. Returns the number of rows
    /// whose `deleted_at` actually transitioned from NULL → set.
    pub async fn soft_delete_workspace(&self, id: String, at_ms: i64) -> Result<usize> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || with_busy_retry(&pool, |conn| -> rusqlite::Result<usize> {
            let n = conn.execute(
                "UPDATE workspaces SET deleted_at = ?2 \
                 WHERE id = ?1 AND deleted_at IS NULL",
                params![id, at_ms],
            )?;
            Ok(n)
        }))
        .await
        .context("spawn_blocking soft_delete_workspace")?
    }

    /// Look up the workspace_id of a given agent — the reverse direction
    /// of `agents.workspace_id`. The spell runner uses this to inherit
    /// the caller agent's workspace when MCP `swarm_run_spell` fires.
    /// Returns `None` if the agent isn't found, or if its workspace_id
    /// is NULL (pre-Step-3 rows or legacy `+ Claude` clicks).
    pub async fn get_workspace_id_for_agent(
        &self,
        agent_id: String,
    ) -> Result<Option<String>> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || with_busy_retry(&pool, |conn| -> rusqlite::Result<Option<String>> {
            let mut stmt = conn.prepare(
                "SELECT workspace_id FROM agents WHERE id = ?1",
            )?;
            let mut rows = stmt.query(params![agent_id])?;
            if let Some(row) = rows.next()? {
                let val: Option<String> = row.get(0)?;
                Ok(val)
            } else {
                Ok(None)
            }
        }))
        .await
        .context("spawn_blocking get_workspace_id_for_agent")?
    }

    /// Attach a dependency-source root folder to a workspace. `id` is minted
    /// server-side via `lower(hex(randomblob(16)))` (same uuid-free trick as
    /// `create_workspace`). Returns the persisted row. The workspace's primary
    /// project dir lives in `workspaces.cwd`; this table only holds the extra
    /// attached roots.
    pub async fn add_workspace_root(
        &self,
        rec: NewWorkspaceRoot,
        created_at: i64,
    ) -> Result<WorkspaceRootRecord> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || with_busy_retry(&pool, |conn| -> rusqlite::Result<WorkspaceRootRecord> {
            let mut stmt = conn.prepare(
                "INSERT INTO workspace_roots \
                     (id, workspace_id, path, role, label, parent_id, created_at) \
                 VALUES (lower(hex(randomblob(16))), ?1, ?2, ?3, ?4, ?5, ?6) \
                 RETURNING id, workspace_id, path, role, label, parent_id, created_at",
            )?;
            let mut rows = stmt.query(params![
                rec.workspace_id,
                rec.path,
                rec.role,
                rec.label,
                rec.parent_id,
                created_at,
            ])?;
            let row = rows.next()?.ok_or(rusqlite::Error::QueryReturnedNoRows)?;
            Ok(WorkspaceRootRecord {
                id: row.get(0)?,
                workspace_id: row.get(1)?,
                path: row.get(2)?,
                role: row.get(3)?,
                label: row.get(4)?,
                parent_id: row.get(5)?,
                created_at: row.get(6)?,
            })
        }))
        .await
        .context("spawn_blocking add_workspace_root")?
    }

    /// Return every attached root across all workspaces, ordered by
    /// `created_at` ASC. The list handler groups these by `workspace_id` in a
    /// single pass so it can attach roots to each workspace without N+1.
    pub async fn list_all_workspace_roots(&self) -> Result<Vec<WorkspaceRootRecord>> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || with_busy_retry(&pool, |conn| -> rusqlite::Result<Vec<WorkspaceRootRecord>> {
            let mut stmt = conn.prepare(
                "SELECT id, workspace_id, path, role, label, parent_id, created_at \
                 FROM workspace_roots \
                 ORDER BY created_at ASC",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok(WorkspaceRootRecord {
                    id: row.get(0)?,
                    workspace_id: row.get(1)?,
                    path: row.get(2)?,
                    role: row.get(3)?,
                    label: row.get(4)?,
                    parent_id: row.get(5)?,
                    created_at: row.get(6)?,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
        }))
        .await
        .context("spawn_blocking list_all_workspace_roots")?
    }

    /// Return the attached roots for a single workspace, ordered by
    /// `created_at` ASC.
    pub async fn list_workspace_roots(
        &self,
        workspace_id: String,
    ) -> Result<Vec<WorkspaceRootRecord>> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || with_busy_retry(&pool, |conn| -> rusqlite::Result<Vec<WorkspaceRootRecord>> {
            let mut stmt = conn.prepare(
                "SELECT id, workspace_id, path, role, label, parent_id, created_at \
                 FROM workspace_roots WHERE workspace_id = ?1 \
                 ORDER BY created_at ASC",
            )?;
            let rows = stmt.query_map(params![workspace_id], |row| {
                Ok(WorkspaceRootRecord {
                    id: row.get(0)?,
                    workspace_id: row.get(1)?,
                    path: row.get(2)?,
                    role: row.get(3)?,
                    label: row.get(4)?,
                    parent_id: row.get(5)?,
                    created_at: row.get(6)?,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
        }))
        .await
        .context("spawn_blocking list_workspace_roots")?
    }

    /// Look up a single workspace root by its primary key. Returns `None` if
    /// not found. Used to validate a `parent_id` belongs to the same
    /// workspace before attaching a child node under it.
    pub async fn get_workspace_root(
        &self,
        id: String,
    ) -> Result<Option<WorkspaceRootRecord>> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || with_busy_retry(&pool, |conn| -> rusqlite::Result<Option<WorkspaceRootRecord>> {
            let mut stmt = conn.prepare(
                "SELECT id, workspace_id, path, role, label, parent_id, created_at \
                 FROM workspace_roots WHERE id = ?1",
            )?;
            let mut rows = stmt.query(params![id])?;
            if let Some(row) = rows.next()? {
                Ok(Some(WorkspaceRootRecord {
                    id: row.get(0)?,
                    workspace_id: row.get(1)?,
                    path: row.get(2)?,
                    role: row.get(3)?,
                    label: row.get(4)?,
                    parent_id: row.get(5)?,
                    created_at: row.get(6)?,
                }))
            } else {
                Ok(None)
            }
        }))
        .await
        .context("spawn_blocking get_workspace_root")?
    }

    /// Detach a node from a workspace's logical tree by its `id`, CASCADING
    /// to all of its descendants (any row whose `parent_id` chain leads back
    /// to `id`). Returns the total number of rows deleted (0 if nothing
    /// matched the `(workspace_id, id)` pair). The caller refreshes the
    /// workspace's managed context block afterwards.
    ///
    /// SQLite has no recursive cascade on a self-referencing FK, so we
    /// collect the descendant set in memory first (BFS over `parent_id`),
    /// then issue a single bulk `DELETE ... WHERE id IN (...)` — all inside
    /// one `spawn_blocking` on the same connection.
    pub async fn delete_workspace_root(
        &self,
        workspace_id: String,
        id: String,
    ) -> Result<usize> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || with_busy_retry(&pool, |conn| -> rusqlite::Result<usize> {
            // Load every (id, parent_id) edge for this workspace once so we
            // can walk the tree without N round-trips.
            let mut stmt = conn.prepare(
                "SELECT id, parent_id FROM workspace_roots WHERE workspace_id = ?1",
            )?;
            let edges: Vec<(String, Option<String>)> = stmt
                .query_map(params![workspace_id], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            drop(stmt);

            // The target must exist in this workspace; if not, nothing to do.
            if !edges.iter().any(|(rid, _)| *rid == id) {
                return Ok(0);
            }

            // BFS: start with {id}; repeatedly pull in rows whose parent_id is
            // already in the collected set, until no new ids appear.
            let mut collected: std::collections::HashSet<String> =
                std::collections::HashSet::new();
            collected.insert(id.clone());
            loop {
                let mut added = false;
                for (rid, parent) in &edges {
                    if let Some(p) = parent {
                        if collected.contains(p) && !collected.contains(rid) {
                            collected.insert(rid.clone());
                            added = true;
                        }
                    }
                }
                if !added {
                    break;
                }
            }

            // Single bulk delete scoped to this workspace.
            let ids: Vec<String> = collected.into_iter().collect();
            let placeholders =
                std::iter::repeat("?").take(ids.len()).collect::<Vec<_>>().join(",");
            let sql = format!(
                "DELETE FROM workspace_roots \
                 WHERE workspace_id = ? AND id IN ({placeholders})"
            );
            let mut bound: Vec<rusqlite::types::Value> = Vec::with_capacity(ids.len() + 1);
            // Clone (don't move) the captured `workspace_id`: the retry closure
            // is `FnMut` and may run more than once.
            bound.push(workspace_id.clone().into());
            for i in ids {
                bound.push(i.into());
            }
            let n = conn.execute(&sql, rusqlite::params_from_iter(bound.iter()))?;
            Ok(n)
        }))
        .await
        .context("spawn_blocking delete_workspace_root")?
    }

    // ── spell runs ───────────────────────────────────────────────────────

    /// Record a spell run for lineage tracking. Mirrors `create_workspace`
    /// in using SQLite's `randomblob` to mint the id, so this crate
    /// doesn't take a uuid dependency.
    pub async fn create_spell_run(&self, rec: NewSpellRun) -> Result<SpellRunRecord> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || with_busy_retry(&pool, |conn| -> rusqlite::Result<SpellRunRecord> {
            let mut stmt = conn.prepare(
                "INSERT INTO spell_runs (id, workspace_id, spell_name, task, caller_agent_id, started_at) \
                 VALUES (lower(hex(randomblob(16))), ?1, ?2, ?3, ?4, ?5) \
                 RETURNING id, workspace_id, spell_name, task, caller_agent_id, started_at",
            )?;
            let mut rows = stmt.query(params![
                rec.workspace_id,
                rec.spell_name,
                rec.task,
                rec.caller_agent_id,
                rec.started_at,
            ])?;
            let row = rows.next()?.ok_or(rusqlite::Error::QueryReturnedNoRows)?;
            Ok(SpellRunRecord {
                id: row.get(0)?,
                workspace_id: row.get(1)?,
                spell_name: row.get(2)?,
                task: row.get(3)?,
                caller_agent_id: row.get(4)?,
                started_at: row.get(5)?,
            })
        }))
        .await
        .context("spawn_blocking create_spell_run")?
    }

    /// 注册一个 orchestrator 派出来的 ad-hoc worker。注意 agent_id 必须
    /// 先在 `agents` 表存在(`record_agent_spawn` 先跑)— 这里只补 worker
    /// metadata。
    pub async fn record_worker(&self, rec: NewWorker) -> Result<()> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || with_busy_retry(&pool, |conn| -> rusqlite::Result<()> {
            // Clone (don't move) the optional columns out of the captured `rec`:
            // the retry closure is `FnMut` and may run more than once.
            let handoff_signal = if rec.handoff_signal.is_empty() {
                None
            } else {
                Some(rec.handoff_signal.clone())
            };
            let depends_on_json = if rec.depends_on_json.is_empty() || rec.depends_on_json == "[]" {
                None
            } else {
                Some(rec.depends_on_json.clone())
            };
            conn.execute(
                "INSERT INTO workers (agent_id, parent_agent_id, role_label, system_prompt, \
                 handoff_signal, depends_on_json, spawned_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    rec.agent_id,
                    rec.parent_agent_id,
                    rec.role_label,
                    rec.system_prompt,
                    handoff_signal,
                    depends_on_json,
                    rec.spawned_at,
                ],
            )?;
            Ok(())
        }))
        .await
        .context("spawn_blocking record_worker")?
    }

    /// Batch lookup workers by their agent_id. Returns a map keyed by
    /// agent_id; missing ids are absent from the map. Used by `list_agents`
    /// to derive `parent_agent_id` + `role_label` in one round-trip.
    pub async fn list_workers_by_ids(
        &self,
        ids: Vec<String>,
    ) -> Result<std::collections::HashMap<String, WorkerRecord>> {
        if ids.is_empty() {
            return Ok(std::collections::HashMap::new());
        }
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || {
            with_busy_retry(
                &pool,
                |conn| -> rusqlite::Result<std::collections::HashMap<String, WorkerRecord>> {
                    let placeholders =
                        std::iter::repeat("?").take(ids.len()).collect::<Vec<_>>().join(",");
                    let sql = format!(
                        "SELECT agent_id, parent_agent_id, role_label, system_prompt, \
                                handoff_signal, depends_on_json, spawned_at \
                         FROM workers WHERE agent_id IN ({placeholders})"
                    );
                    let mut stmt = conn.prepare(&sql)?;
                    let rows = stmt.query_map(rusqlite::params_from_iter(ids.iter()), |row| {
                        let handoff: Option<String> = row.get(4)?;
                        let deps: Option<String> = row.get(5)?;
                        Ok(WorkerRecord {
                            agent_id: row.get(0)?,
                            parent_agent_id: row.get(1)?,
                            role_label: row.get(2)?,
                            system_prompt: row.get(3)?,
                            handoff_signal: handoff.unwrap_or_default(),
                            depends_on_json: deps.unwrap_or_else(|| "[]".to_string()),
                            spawned_at: row.get(6)?,
                        })
                    })?;
                    let mut out = std::collections::HashMap::with_capacity(ids.len());
                    for rec in rows {
                        let rec = rec?;
                        out.insert(rec.agent_id.clone(), rec);
                    }
                    Ok(out)
                },
            )
        })
        .await
        .context("spawn_blocking list_workers_by_ids")?
    }

    /// Batch lookup spell_runs by id. Returns a map keyed by id; missing ids
    /// are simply absent from the result (callers handle the None case).
    /// Used by `list_agents` to derive each agent's `parent_agent_id` from
    /// its `spell_run.caller_agent_id` in one round-trip instead of N+1.
    pub async fn list_spell_runs_by_ids(
        &self,
        ids: Vec<String>,
    ) -> Result<std::collections::HashMap<String, SpellRunRecord>> {
        if ids.is_empty() {
            return Ok(std::collections::HashMap::new());
        }
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || {
            with_busy_retry(
                &pool,
                |conn| -> rusqlite::Result<std::collections::HashMap<String, SpellRunRecord>> {
                    let placeholders =
                        std::iter::repeat("?").take(ids.len()).collect::<Vec<_>>().join(",");
                    let sql = format!(
                        "SELECT id, workspace_id, spell_name, task, caller_agent_id, started_at \
                         FROM spell_runs WHERE id IN ({placeholders})"
                    );
                    let mut stmt = conn.prepare(&sql)?;
                    let rows = stmt.query_map(rusqlite::params_from_iter(ids.iter()), |row| {
                        Ok(SpellRunRecord {
                            id: row.get(0)?,
                            workspace_id: row.get(1)?,
                            spell_name: row.get(2)?,
                            task: row.get(3)?,
                            caller_agent_id: row.get(4)?,
                            started_at: row.get(5)?,
                        })
                    })?;
                    let mut out = std::collections::HashMap::with_capacity(ids.len());
                    for rec in rows {
                        let rec = rec?;
                        out.insert(rec.id.clone(), rec);
                    }
                    Ok(out)
                },
            )
        })
        .await
        .context("spawn_blocking list_spell_runs_by_ids")?
    }

    pub async fn search_blackboard(&self, query: String) -> Result<Vec<BlackboardOpRecord>> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || with_busy_retry(&pool, |conn| -> rusqlite::Result<Vec<BlackboardOpRecord>> {
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
            rows.collect::<rusqlite::Result<Vec<_>>>()
        }))
        .await
        .context("spawn_blocking search_blackboard")?
    }
}
