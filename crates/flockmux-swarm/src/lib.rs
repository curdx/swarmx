//! flockmux-swarm: in-process message dispatch + blackboard sync.
//!
//! Built on `flockmux-storage` for persistence and `flockmux-protocol` for
//! the wire-level [`SwarmEvent`] enum. The server (`flockmux-server`) bolts
//! REST + WS handlers on top of this crate's [`Swarm`] type.

pub mod path_safe;
mod swarm;
mod watcher;

pub use swarm::{Envelope, NewMessage, Swarm};
pub use watcher::WatcherHandle;

pub use flockmux_protocol::ws_swarm::SwarmEvent;
