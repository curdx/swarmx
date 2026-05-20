//! `/ws/swarm` events (M3+). Placeholder for now so the crate compiles.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SwarmEvent {
    AgentState {
        agent_id: String,
        state: AgentState,
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
