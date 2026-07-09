//! r2d2 `CustomizeConnection` that primes every checked-out connection
//! with the PRAGMAs we depend on.
//!
//! Why a customizer (not a one-shot init): r2d2 holds a pool of N
//! connections; `journal_mode=WAL` is per-database (persists across the
//! whole file) but `busy_timeout` and `foreign_keys` are *per-connection*,
//! so we need to set them on every fresh connection the pool opens.

use r2d2::CustomizeConnection;
use r2d2_sqlite::rusqlite::Connection;

#[derive(Debug)]
pub(crate) struct Customizer;

impl CustomizeConnection<Connection, r2d2_sqlite::rusqlite::Error> for Customizer {
    fn on_acquire(&self, conn: &mut Connection) -> Result<(), r2d2_sqlite::rusqlite::Error> {
        // Set busy_timeout FIRST so the subsequent PRAGMA journal_mode=WAL
        // can wait its turn instead of erroring with SQLITE_BUSY when the
        // pool spins up multiple fresh connections in parallel.
        conn.pragma_update(None, "busy_timeout", 5_000)?;
        // WAL is sticky to the file but `journal_mode` is a query, not a
        // statement — `pragma_update` would error. Use `query_row`.
        conn.query_row("PRAGMA journal_mode=WAL", [], |_| Ok(()))?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        // foreign_keys=ON enforces referential integrity (you can't insert a
        // child pointing at a missing parent). Most FK columns are NO ACTION
        // (the default), but `thought_traces` (migration 0021) intentionally
        // uses ON DELETE CASCADE (trigger_message_id, trace_id) and ON DELETE
        // SET NULL (response_message_id). These cascades are LIVE, not dormant:
        // `prune_expired` physically deletes delivered+read messages, which
        // cascade-deletes their thought_traces (and thence thought_trace_events)
        // — the intended cleanup of orphaned traces. Other physical deletes:
        // `delete_workspace_root` walks+removes its own subtree, and
        // `delete_blackboard_prefix` removes blackboard_ops rows by prefix.
        // workspaces are otherwise soft-deleted (deleted_at) and agents killed
        // (killed_at). Adding new cascades to old tables still needs a SQLite
        // table-rebuild, so weigh that before extending them.
        conn.pragma_update(None, "foreign_keys", "ON")?;
        Ok(())
    }
}
