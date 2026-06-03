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
        /// Parent message id when this is a reply. Optional so old clients
        /// (which serialize without the field) still round-trip.
        #[serde(default)]
        in_reply_to: Option<i64>,
        /// Direction (thread) this message belongs to; `None` = main/untagged.
        /// Lets the UI hard-gate a direction's chat on live messages too.
        #[serde(default)]
        thread_id: Option<String>,
        /// Structured metadata for system-generated messages (Slack
        /// `event_payload`-style), e.g. `{"subtype":"wake","reason":"blackboard",
        /// "key":"…"}` or `{"subtype":"completion","signal":"reviewer.done"}`.
        /// The UI renders/filters from `meta.subtype` instead of regex-parsing
        /// the prose `body`. `None` for agent free-text (UI falls back to
        /// heuristics there). Optional so old clients still round-trip.
        #[serde(default)]
        meta: Option<serde_json::Value>,
    },
    /// A batch of messages was marked read on behalf of `to_agent`. Used by
    /// the UI to decrement the unread badge live without a REST poll.
    MessageRead {
        ids: Vec<i64>,
        to_agent: String,
        at: i64,
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
    /// A direction (thread) was created, renamed, isolated into a worktree,
    /// degraded back to shared, or deleted — i.e. the workspace's thread list
    /// changed in a way the REST `/api/workspaces` snapshot doesn't push on its
    /// own. Subscribers (the sidebar) refetch workspaces so a direction named/
    /// isolated by `swarm_name_thread` (a server-side PATCH, not a UI action)
    /// shows its new name + branch icon live, without a reload.
    ThreadChanged {
        workspace_id: String,
        thread_id: String,
        /// "created" | "updated" | "isolated" | "deleted".
        op: String,
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
