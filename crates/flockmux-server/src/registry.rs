//! Per-agent registry: holds the live `PtyBridge`s keyed by agent id.
//!
//! Each agent owns:
//!  - a `PtyBridge` (the PTY + child),
//!  - the *taken* output `Receiver` (since mpsc is single-consumer it lives
//!    here until the WS handler grabs it; subsequent connects to the same
//!    agent are refused for M1, "attach existing PTY" lands in M2),
//!  - a clone of the input `Sender` (to inject keystrokes).
//!
//! Storage is a `DashMap<AgentId, Arc<Mutex<AgentSlot>>>` so HTTP and WS
//! handlers can claim/release independently without serialising the whole
//! map.

use bytes::Bytes;
use dashmap::DashMap;
use flockmux_pty::PtyBridge;
use parking_lot::Mutex;
use std::sync::Arc;
use tokio::sync::mpsc;

pub struct AgentSlot {
    pub bridge: Arc<PtyBridge>,
    /// `Some` until a WS handler grabs the output stream; then `None` and
    /// subsequent attaches get a friendly error.
    pub output_rx: Option<mpsc::Receiver<Bytes>>,
    pub input_tx: mpsc::Sender<Bytes>,
    pub cli: String,
    pub role: String,
    pub workspace: String,
}

#[derive(Default, Clone)]
pub struct Registry {
    inner: Arc<DashMap<String, Arc<Mutex<AgentSlot>>>>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&self, agent_id: String, slot: AgentSlot) {
        self.inner.insert(agent_id, Arc::new(Mutex::new(slot)));
    }

    pub fn get(&self, agent_id: &str) -> Option<Arc<Mutex<AgentSlot>>> {
        self.inner.get(agent_id).map(|e| e.clone())
    }

    pub fn remove(&self, agent_id: &str) -> Option<Arc<Mutex<AgentSlot>>> {
        self.inner.remove(agent_id).map(|(_, v)| v)
    }
}
