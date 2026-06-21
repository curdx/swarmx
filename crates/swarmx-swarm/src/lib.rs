//! swarmx-swarm: in-process message dispatch + blackboard sync.
//!
//! Built on `swarmx-storage` for persistence and `swarmx-protocol` for
//! the wire-level [`SwarmEvent`] enum. The server (`swarmx-server`) bolts
//! REST + WS handlers on top of this crate's [`Swarm`] type.

pub mod path_safe;
mod swarm;
mod watcher;

pub use swarm::{Envelope, NewMessage, Swarm};
pub use watcher::WatcherHandle;

pub use swarmx_protocol::ws_swarm::SwarmEvent;
