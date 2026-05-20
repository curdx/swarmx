//! Row + DTO types shared between the storage API and its callers.

use serde::{Deserialize, Serialize};

/// New agent record (insert payload for [`crate::Store::record_agent_spawn`]).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewAgent {
    pub id: String,
    pub cli: String,
    pub role: String,
    pub workspace: String,
    /// Unix-ms timestamp of the spawn event.
    pub spawned_at: i64,
}

/// Full agent row as stored in `agents`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRecord {
    pub id: String,
    pub cli: String,
    pub role: String,
    pub workspace: String,
    pub spawned_at: i64,
    pub killed_at: Option<i64>,
    pub shim_ready_at: Option<i64>,
    pub shim_exit_at: Option<i64>,
    pub shim_exit_code: Option<i32>,
}

/// New message (insert payload for [`crate::Store::insert_message`]).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewMessage {
    pub from_agent: String,
    pub to_agent: String,
    pub kind: String,
    pub body: String,
    pub sent_at: i64,
}

/// Full message row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageRecord {
    pub id: i64,
    pub from_agent: String,
    pub to_agent: String,
    pub kind: String,
    pub body: String,
    pub sent_at: i64,
    pub delivered_at: Option<i64>,
    pub read_at: Option<i64>,
}

/// Filter options for [`crate::Store::list_messages`].
///
/// `to_agent=None` means "any recipient". `limit` is required to bound result
/// size — callers should pick a sensible default (e.g. 200).
#[derive(Debug, Clone, Default)]
pub struct ListMessagesOpts {
    pub to_agent: Option<String>,
    pub from_agent: Option<String>,
    pub only_undelivered: bool,
    pub limit: i64,
}

/// New blackboard op (insert payload for [`crate::Store::insert_blackboard_op`]).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewBlackboardOp {
    /// Optional — `None` means the op came from an external editor (watcher
    /// path), not from an agent's `write_blackboard` call.
    pub agent_id: Option<String>,
    /// "write" | "external" — open string so callers can extend later.
    pub op: String,
    /// Path relative to the blackboard root, e.g. "tasks.md".
    pub path: String,
    pub content: String,
    pub sha256: String,
    pub at: i64,
}

/// Full blackboard op row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlackboardOpRecord {
    pub id: i64,
    pub agent_id: Option<String>,
    pub op: String,
    pub path: String,
    pub content: String,
    pub sha256: String,
    pub at: i64,
}
