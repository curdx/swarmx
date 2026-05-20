//! `/ws/swarm` event stream. All fan-out (lifecycle, messages, blackboard
//! changes) flows through this single broadcast channel so a subscriber can
//! see the complete cross-agent picture in one connection.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SwarmEvent {
    /// A per-agent lifecycle transition (spawning → ready → idle → exited).
    AgentState {
        agent_id: String,
        state: AgentState,
    },
    /// An agent-to-agent message was persisted. Subscribers can rehydrate
    /// the recipient's inbox without going back to SQLite.
    Message {
        id: i64,
        from_agent: String,
        to_agent: String,
        kind: String,
        body: String,
        sent_at: i64,
    },
    /// A blackboard file changed. `agent_id == None` means the change
    /// came from outside flockmux (the watcher fallback path) — e.g. the
    /// user editing the markdown file in their normal editor.
    BlackboardChanged {
        id: i64,
        agent_id: Option<String>,
        /// "write" (an agent's call) or "external" (filesystem watcher).
        op: String,
        path: String,
        sha256: String,
        at: i64,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentState {
    Spawning,
    Ready,
    Thinking,
    Idle,
    Exited,
}
