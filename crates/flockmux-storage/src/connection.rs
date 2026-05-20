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
        // WAL is sticky to the file but `journal_mode` is a query, not a
        // statement — using `pragma_update` would error. Use `query_row`.
        conn.query_row("PRAGMA journal_mode=WAL", [], |_| Ok(()))?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "busy_timeout", 5_000)?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        Ok(())
    }
}
