//! REST DTOs.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpawnAgentRequest {
    /// Plugin id from `cli-plugins/<id>.toml`, e.g. "claude" or "codex".
    pub cli: String,
    /// Optional role label shown in the UI; defaults to the cli name.
    #[serde(default)]
    pub role: Option<String>,
    /// Optional workspace path. If absent, server allocates a temp dir.
    #[serde(default)]
    pub workspace: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpawnAgentResponse {
    pub agent_id: String,
    pub cli: String,
    pub role: String,
    pub workspace: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliPluginInfo {
    pub id: String,
    pub display_name: String,
    pub binary: String,
}

/// One entry in `GET /api/agent`. Mirrors `SpawnAgentResponse` plus the
/// fields a reconnecting UI needs to render initial status before its
/// WS Hello frame arrives. `killed_at = Some(_)` means the agent's PTY
/// has been torn down — the entry comes from the SQLite history only.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentInfo {
    pub agent_id: String,
    pub cli: String,
    pub role: String,
    pub workspace: String,
    pub shim_ready: bool,
    pub shim_exit: Option<i32>,
    /// Unix-ms timestamp the agent was killed; `None` while live.
    #[serde(default)]
    pub killed_at: Option<i64>,
    /// Unix-ms timestamp the agent was spawned. Always populated for
    /// SQLite-backed rows; `None` for in-memory-only entries (shouldn't
    /// happen in practice since spawn always writes to SQLite first).
    #[serde(default)]
    pub spawned_at: Option<i64>,
}

// ── swarm REST DTOs (M3 #18) ─────────────────────────────────────────────

/// `POST /api/message` payload. `from` is optional so a system-emitted
/// notification can omit it; the server fills in a sentinel if it's
/// missing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendMessageRequest {
    #[serde(default)]
    pub from: Option<String>,
    pub to: String,
    pub kind: String,
    pub body: String,
}

/// Returned by `POST /api/message` so the client knows the persisted row.
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

/// `PUT /api/blackboard/:path` payload. The path itself rides in the URL;
/// the body carries the new content. `agent_id` lets the caller attribute
/// the write — `None` ⇒ system / external.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WriteBlackboardRequest {
    #[serde(default)]
    pub agent_id: Option<String>,
    pub content: String,
}

/// Returned by `GET /api/blackboard/:path` (snapshot of the latest content).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlackboardSnapshot {
    pub path: String,
    pub content: String,
    pub sha256: String,
    pub at: i64,
}

/// Returned by `GET /api/blackboard` — one entry per known path, latest op.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlackboardEntry {
    pub path: String,
    pub sha256: String,
    pub at: i64,
    pub op: String,
}

// ── recording DTOs (M3 #19) ──────────────────────────────────────────────

/// One entry in `GET /api/recording`. The .cast file content is *not*
/// inlined — clients hit `GET /api/recording/:id` to stream the bytes.
/// `finalized_at = None` means the recording is still live (its agent's
/// PTY hasn't EOFed yet).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordingInfo {
    pub id: String,
    pub agent_id: String,
    pub started_at: i64,
    #[serde(default)]
    pub finalized_at: Option<i64>,
    #[serde(default)]
    pub duration_ms: Option<i64>,
    pub cols: i64,
    pub rows: i64,
    #[serde(default)]
    pub last_seq: Option<i64>,
}
