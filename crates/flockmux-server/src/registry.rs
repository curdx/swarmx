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
use tokio::process::Child;
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
    /// The transport carrying this agent's I/O: a PTY (claude/codex, scraped
    /// over a pseudo-terminal) or a structured ACP stdio session (opencode,
    /// driven as JSON-RPC where flockmux owns the turn loop). PTY-specific
    /// handles live inside the `Pty` variant; consumers go through the
    /// `AgentSlot` methods (`is_alive`, `pty_stream`, `pty_input`, …) so they
    /// don't care which transport backs the agent.
    pub channel: AgentChannel,
    /// Shim lifecycle state captured by the pump's OSC scanner. Stored
    /// here so a fresh WS attach can be told ShimReady/ShimExit even if
    /// the OSC marker fired before the client connected.
    pub lifecycle: Arc<Mutex<Lifecycle>>,
    /// Broadcasts ShimReady/ShimExit so every attached WS writer relays
    /// the event live (in addition to the snapshot delivered via Hello).
    /// `subscribe()` is cheap and lossless for the small lifecycle volume.
    pub lifecycle_tx: tokio::sync::broadcast::Sender<LifecycleEvent>,
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

/// How an agent's I/O is transported. `Pty` for the interactive CLIs
/// (claude/codex) scraped over a pseudo-terminal; future structured transports
/// (ACP / opencode) add a variant here and the `AgentSlot` accessor methods
/// gain an arm — every consumer keeps going through those methods.
pub enum AgentChannel {
    Pty {
        bridge: Arc<PtyBridge>,
        /// Shared resume buffer fed by the PTY-reader pump and observed by
        /// every WS subscriber. Survives WS disconnect/reconnect.
        stream: Arc<PtyStream>,
        /// Inject keystrokes into the PTY (wake "kicks", terminal WS input).
        input_tx: mpsc::Sender<Bytes>,
    },
    /// ACP (structured JSON-RPC over stdio): no PTY, no terminal view.
    /// `child` is the piped `opencode acp` process (liveness via `try_wait`,
    /// teardown via `start_kill`); `prompt_tx` delivers each turn's prompt text
    /// (the bootstrap first turn, then wakes) to the driver loop, which runs it
    /// as one `session/prompt`.
    Acp {
        child: Arc<Mutex<Child>>,
        prompt_tx: mpsc::UnboundedSender<String>,
    },
}

impl AgentSlot {
    /// Is the agent's underlying process still running?
    pub fn is_alive(&self) -> bool {
        match &self.channel {
            AgentChannel::Pty { bridge, .. } => bridge.is_alive(),
            AgentChannel::Acp { child, .. } => {
                // `try_wait` reaps without blocking; `Ok(None)` = still running.
                child.lock().try_wait().map(|o| o.is_none()).unwrap_or(false)
            }
        }
    }

    /// The process exit code if it has already exited, else `None`.
    pub fn try_exit_code(&self) -> Option<i32> {
        match &self.channel {
            AgentChannel::Pty { bridge, .. } => bridge.try_exit_code(),
            AgentChannel::Acp { child, .. } => {
                child.lock().try_wait().ok().flatten().and_then(|s| s.code())
            }
        }
    }

    /// Terminate the agent's underlying process.
    pub fn kill(&self) {
        match &self.channel {
            AgentChannel::Pty { bridge, .. } => bridge.kill(),
            AgentChannel::Acp { child, .. } => {
                let _ = child.lock().start_kill();
            }
        }
    }

    /// The PTY bridge, for transports that have one (terminal WS attach).
    /// `None` for structured transports (ACP) with no terminal.
    pub fn pty_bridge(&self) -> Option<Arc<PtyBridge>> {
        match &self.channel {
            AgentChannel::Pty { bridge, .. } => Some(bridge.clone()),
            AgentChannel::Acp { .. } => None,
        }
    }

    /// The PTY output ring buffer, or `None` for transports with no terminal
    /// view (ACP) — the terminal WS endpoints serve nothing for those.
    pub fn pty_stream(&self) -> Option<Arc<PtyStream>> {
        match &self.channel {
            AgentChannel::Pty { stream, .. } => Some(stream.clone()),
            AgentChannel::Acp { .. } => None,
        }
    }

    /// The PTY stdin sender for byte injection (wake kick / terminal input),
    /// or `None` for transports that aren't keystroke-driven (ACP).
    pub fn pty_input(&self) -> Option<mpsc::Sender<Bytes>> {
        match &self.channel {
            AgentChannel::Pty { input_tx, .. } => Some(input_tx.clone()),
            AgentChannel::Acp { .. } => None,
        }
    }

    /// Deliver a turn's prompt text to a structured (ACP) agent's driver loop
    /// (the bootstrap first turn, then each wake). Returns `false` if this isn't
    /// an ACP agent or the driver has already stopped (channel closed).
    pub fn deliver_acp_prompt(&self, text: String) -> bool {
        match &self.channel {
            AgentChannel::Acp { prompt_tx, .. } => prompt_tx.send(text).is_ok(),
            AgentChannel::Pty { .. } => false,
        }
    }
}

#[derive(Debug, Clone)]
pub enum LifecycleEvent {
    ShimReady,
    ShimExit(i32),
    /// The CLI is alive but printed a banner meaning it *cannot actually do
    /// work* — the canonical case is claude's "Not logged in · Run /login"
    /// when no OAuth credential is present, or a rate-limit/quota notice. The
    /// PTY pump's `HealthScanner` raises this on the first match so the
    /// lifecycle subscriber can flip the agent to `AgentState::Error` and ride
    /// the human-facing detail on a system `AgentActivity` — replacing the fake
    /// "online" green dot + "暂无消息" with an honest, actionable failure card.
    ///
    /// Carries a `String` payload, so this enum is `Clone` (not `Copy`); the
    /// broadcast channel only requires `Clone`.
    HealthFail { reason: String, kind: String },
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
