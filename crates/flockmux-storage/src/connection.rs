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
        // child pointing at a missing parent). The FK columns are intentionally
        // declared with NO ACTION (the default), NOT ON DELETE CASCADE, and
        // that's deliberate: no code path *physically* deletes a FK-parent row.
        // workspaces are soft-deleted (deleted_at), agents are killed
        // (killed_at), spell_runs/messages are append-only. The ONE physical
        // delete — `delete_workspace_root` — walks + removes its own subtree
        // explicitly (it predates FK pragmas and stays self-contained). So a
        // CASCADE would be dormant, and adding one to existing tables needs a
        // risky SQLite table-rebuild — not worth it until a hard-delete path
        // actually exists. (Audit F-storage: reviewed + intentionally NO ACTION.)
        conn.pragma_update(None, "foreign_keys", "ON")?;
        Ok(())
    }
}
