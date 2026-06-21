//! Wire protocol for swarmx: WebSocket frames + REST DTOs.
//!
//! - `ws_pty`: `/ws/pty/:agent_id` — binary [seq][bytes] + JSON control.
//! - `ws_swarm`: `/ws/swarm` — JSON event stream (used from M3+).
//! - `rest`: REST DTOs.

pub mod rest;
pub mod ws_pty;
pub mod ws_swarm;
