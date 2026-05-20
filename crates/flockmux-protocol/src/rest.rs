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

/// One entry in `GET /api/agent`. Mirrors `SpawnAgentResponse` plus
/// `shim_ready` / `shim_exit` so a reconnecting UI can render initial
/// status before its WS Hello frame arrives.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentInfo {
    pub agent_id: String,
    pub cli: String,
    pub role: String,
    pub workspace: String,
    pub shim_ready: bool,
    pub shim_exit: Option<i32>,
}
