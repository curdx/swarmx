//! flockmux-storage: SQLite + FTS5 persistence for agents, messages, and
//! blackboard ops.
//!
//! All public APIs on [`Store`] are `async` — the underlying calls hop to
//! `tokio::spawn_blocking` so the rusqlite/`Connection` work stays off the
//! runtime's IO thread. Connections are pooled with r2d2; the customizer
//! in module `connection` sets WAL + busy_timeout on every checkout.
//!
//! Timestamps cross the API boundary as `i64` unix-ms. Storage doesn't pick
//! a date library — callers do.

mod connection;
pub mod models;
mod schema;
mod store;

pub use models::{
    AgentRecord, BlackboardOpRecord, ListMessagesOpts, MessageRecord, NewAgent, NewBlackboardOp,
    NewMessage, NewRecording, NewSpellRun, NewThread, NewWorker, NewWorkspace, NewWorkspaceRoot,
    RecordingRecord, SpellRunRecord, TaskRecord, ThreadRecord, UsageByAgent, UsageByDay,
    UsageByModel, WorkerRecord, WorkspaceRecord, WorkspaceRootRecord,
};
pub use store::{PruneStats, Store};
