//! Per-agent registry: holds the live `PtyBridge`s keyed by agent id.
//!
//! Each agent owns:
//!  - a `PtyBridge` (the PTY + child),
//!  - a shared `PtyStream` ring buffer (M3): the PTY-reader background task
//!    appends every chunk here; one or more WS subscribers each hold their
//!    own cursor and replay/follow from the buffer. Replaces the M1
//!    single-consumer `output_rx` claim.
//!  - a clone of the input `Sender` (to inject keystrokes).
//!
//! Storage is a `DashMap<AgentId, Arc<Mutex<AgentSlot>>>` so HTTP and WS
//! handlers can claim/release independently without serialising the whole
//! map.

use crate::pty_stream::PtyStream;
use bytes::Bytes;
use dashmap::DashMap;
use flockmux_pty::PtyBridge;
use parking_lot::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tokio::sync::mpsc;

/// Lifecycle status surfaced to every (re-)attaching client so the UI can
/// render the current state without waiting for the next OSC event. Mutated
/// by the per-agent pump task whenever the OSC scanner detects a marker.
#[derive(Debug, Default, Clone, Copy)]
pub struct Lifecycle {
    pub shim_ready: bool,
    pub shim_exit: Option<i32>,
}

pub struct AgentSlot {
    pub bridge: Arc<PtyBridge>,
    /// Shared resume buffer fed by the PTY-reader pump and observed by
    /// every WS subscriber. Survives WS disconnect/reconnect.
    pub stream: Arc<PtyStream>,
    /// Shim lifecycle state captured by the pump's OSC scanner. Stored
    /// here so a fresh WS attach can be told ShimReady/ShimExit even if
    /// the OSC marker fired before the client connected.
    pub lifecycle: Arc<Mutex<Lifecycle>>,
    /// Broadcasts ShimReady/ShimExit so every attached WS writer relays
    /// the event live (in addition to the snapshot delivered via Hello).
    /// `subscribe()` is cheap and lossless for the small lifecycle volume.
    pub lifecycle_tx: tokio::sync::broadcast::Sender<LifecycleEvent>,
    pub input_tx: mpsc::Sender<Bytes>,
    pub cli: String,
    pub role: String,
    pub workspace: String,
    /// In-memory pause flag. When true, the WakeCoordinator's auto-wake
    /// path (BlackboardChanged → deliver_wake) short-circuits and does not
    /// inject PTY bytes for this agent. Manual operator wakes bypass this.
    /// Persisted lifetime = process lifetime; resets on server restart.
    pub paused: Arc<AtomicBool>,
    /// MCP-readiness gate. The agent's `flockmux-mcp` subprocess flips this to
    /// `true` (via `POST /api/agent/:id/mcp-ready`) the moment the CLI fetches
    /// its tool list — per the MCP lifecycle that means the model can now see
    /// the `swarm_*` tools. The bootstrap waits on this (with a fallback
    /// timeout) instead of a fixed sleep: the readiness-probe pattern. `watch`
    /// retains the latest value, so a ping that races the subscriber is never
    /// lost (`send_replace` updates the stored value even with no receivers).
    pub mcp_ready: tokio::sync::watch::Sender<bool>,
}

#[derive(Debug, Clone, Copy)]
pub enum LifecycleEvent {
    ShimReady,
    ShimExit(i32),
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

    /// Snapshot of every live agent. Used by `GET /api/agent` so a freshly
    /// loaded UI can reattach to agents spawned before this tab existed.
    pub fn list(&self) -> Vec<(String, Arc<Mutex<AgentSlot>>)> {
        self.inner
            .iter()
            .map(|e| (e.key().clone(), e.value().clone()))
            .collect()
    }
}
