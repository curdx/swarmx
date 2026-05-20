//! `Swarm`: per-process dispatch + blackboard sync.
//!
//! - **Messages**: caller hands us `NewMessage`; we persist to SQLite (the
//!   authoritative store), broadcast a `SwarmEvent::Message`, and push an
//!   `Envelope` into the recipient's in-memory `mpsc` if one is registered.
//!   Persistence is the contract — the mpsc is just a low-latency hint.
//! - **Blackboard**: `write_blackboard` writes to disk and records the op;
//!   `reconcile_external` (called by the watcher) detects user-edits that
//!   weren't initiated by us and records them too. A `seen_sha` cache
//!   prevents the watcher from re-recording our own writes.

use crate::path_safe;
use anyhow::{Context, Result};
use dashmap::DashMap;
use flockmux_protocol::ws_swarm::SwarmEvent;
use flockmux_storage::{
    BlackboardOpRecord, MessageRecord as StoreMessageRecord, NewBlackboardOp,
    NewMessage as StoreNewMessage, Store,
};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};

/// Envelope handed to a per-agent inbox. M3 only uses this in tests / future
/// MCP integration — the production path is SQLite + ws/swarm broadcast.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    pub id: i64,
    pub from_agent: String,
    pub kind: String,
    pub body: String,
    pub sent_at: i64,
}

/// What the caller passes to [`Swarm::send_message`]. Same shape as the
/// storage `NewMessage` but kept independent so we can extend later
/// (priorities, expiry, etc.) without touching the persistence layer.
#[derive(Debug, Clone)]
pub struct NewMessage {
    pub from_agent: String,
    pub to_agent: String,
    pub kind: String,
    pub body: String,
    pub sent_at: i64,
}

pub struct Swarm {
    store: Arc<Store>,
    inboxes: DashMap<String, mpsc::Sender<Envelope>>,
    blackboard_root: PathBuf,
    events_tx: broadcast::Sender<SwarmEvent>,
    /// SHA-256 of the last content we wrote to each absolute path. The
    /// watcher consults this before persisting an "external" op so it
    /// doesn't echo our own writes back into SQLite.
    seen_sha: Mutex<HashMap<PathBuf, String>>,
}

impl Swarm {
    pub fn new(store: Arc<Store>, blackboard_root: PathBuf) -> Arc<Self> {
        let (events_tx, _events_rx) = broadcast::channel(256);
        Arc::new(Self {
            store,
            inboxes: DashMap::new(),
            blackboard_root,
            events_tx,
            seen_sha: Mutex::new(HashMap::new()),
        })
    }

    pub fn store(&self) -> &Arc<Store> {
        &self.store
    }

    pub fn blackboard_root(&self) -> &Path {
        &self.blackboard_root
    }

    /// Subscribe to the cross-agent event stream. New subscribers see only
    /// events emitted *after* they subscribe (broadcast semantics).
    pub fn subscribe(&self) -> broadcast::Receiver<SwarmEvent> {
        self.events_tx.subscribe()
    }

    /// Emit a lifecycle event (called from the server's PTY-lifecycle pump).
    pub fn publish_event(&self, ev: SwarmEvent) {
        // Send-on-no-subscribers is a no-op for broadcast::Sender; ignore.
        let _ = self.events_tx.send(ev);
    }

    /// Register an inbox for `agent_id`. Returns the receive side; the
    /// caller is responsible for draining it (e.g. an MCP server bound to
    /// this agent). Capacity 32 is enough for a chatty interactive session
    /// without enabling unbounded growth.
    pub fn register_agent(&self, agent_id: String) -> mpsc::Receiver<Envelope> {
        let (tx, rx) = mpsc::channel(32);
        self.inboxes.insert(agent_id, tx);
        rx
    }

    pub fn unregister_agent(&self, agent_id: &str) {
        self.inboxes.remove(agent_id);
    }

    /// Persist + broadcast a message. Returns the persisted record.
    pub async fn send_message(&self, msg: NewMessage) -> Result<StoreMessageRecord> {
        let record = self
            .store
            .insert_message(StoreNewMessage {
                from_agent: msg.from_agent.clone(),
                to_agent: msg.to_agent.clone(),
                kind: msg.kind.clone(),
                body: msg.body.clone(),
                sent_at: msg.sent_at,
            })
            .await
            .context("store.insert_message")?;

        // Broadcast first so subscribers see it even if delivery fails.
        let _ = self.events_tx.send(SwarmEvent::Message {
            id: record.id,
            from_agent: record.from_agent.clone(),
            to_agent: record.to_agent.clone(),
            kind: record.kind.clone(),
            body: record.body.clone(),
            sent_at: record.sent_at,
        });

        // Try the in-memory inbox; if absent or full, the message stays in
        // SQLite and a future inbox will replay via `list_messages`.
        let delivered_now = if let Some(tx) = self.inboxes.get(&msg.to_agent) {
            let env = Envelope {
                id: record.id,
                from_agent: record.from_agent.clone(),
                kind: record.kind.clone(),
                body: record.body.clone(),
                sent_at: record.sent_at,
            };
            tx.try_send(env).is_ok()
        } else {
            false
        };
        if delivered_now {
            // best-effort; ignore mark_delivered errors
            if let Err(e) = self
                .store
                .mark_delivered(vec![record.id], now_ms())
                .await
            {
                tracing::warn!(?e, "mark_delivered failed");
            }
        }
        Ok(record)
    }

    /// Write `content` to a path relative to the blackboard root. Records
    /// the op in SQLite and broadcasts.
    pub async fn write_blackboard(
        &self,
        agent_id: Option<String>,
        rel_path: &str,
        content: &str,
    ) -> Result<BlackboardOpRecord> {
        let target = path_safe::resolve_for_write(&self.blackboard_root, rel_path)
            .context("resolve write path")?;

        let sha = sha256_hex(content.as_bytes());
        // Prime the seen-sha cache BEFORE writing so the watcher (which
        // may fire on the very next iteration) sees the new hash already.
        self.seen_sha
            .lock()
            .insert(target.clone(), sha.clone());

        // Run the actual fs write off the runtime thread.
        let target_for_write = target.clone();
        let content_for_write = content.to_owned();
        tokio::task::spawn_blocking(move || -> Result<()> {
            std::fs::write(&target_for_write, content_for_write.as_bytes())
                .with_context(|| format!("write blackboard {}", target_for_write.display()))
        })
        .await
        .context("spawn_blocking fs::write")??;

        let rel_owned = rel_path.to_string();
        let now = now_ms();
        let record = self
            .store
            .insert_blackboard_op(NewBlackboardOp {
                agent_id: agent_id.clone(),
                op: "write".into(),
                path: rel_owned.clone(),
                content: content.to_string(),
                sha256: sha.clone(),
                at: now,
            })
            .await
            .context("store.insert_blackboard_op")?;

        let _ = self.events_tx.send(SwarmEvent::BlackboardChanged {
            id: record.id,
            agent_id,
            op: "write".into(),
            path: rel_owned,
            sha256: sha,
            at: now,
        });

        Ok(record)
    }

    /// Read the latest content of a blackboard file.
    pub async fn read_blackboard(&self, rel_path: &str) -> Result<Option<String>> {
        let target = match path_safe::resolve_existing(&self.blackboard_root, rel_path) {
            Ok(p) => p,
            // Missing file is None, not an error — the route maps it to 404.
            Err(e) => {
                tracing::debug!(?e, "read_blackboard: resolve failed");
                return Ok(None);
            }
        };
        tokio::task::spawn_blocking(move || -> Result<Option<String>> {
            match std::fs::read_to_string(&target) {
                Ok(s) => Ok(Some(s)),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(anyhow::anyhow!("read {}: {}", target.display(), e)),
            }
        })
        .await
        .context("spawn_blocking fs::read")?
    }

    /// Called by the watcher when an external filesystem event fires.
    /// Compares the file's current SHA-256 with the cached one — if they
    /// match, we wrote this and skip; otherwise we record an "external" op
    /// and broadcast it.
    pub(crate) async fn reconcile_external(self: &Arc<Self>, abs_path: &Path) -> Result<()> {
        // Must be under the blackboard root and inside the canonical form.
        let canon_root = self
            .blackboard_root
            .canonicalize()
            .context("canonicalize blackboard root")?;
        let canon_path = match abs_path.canonicalize() {
            Ok(p) => p,
            // File may have been deleted between the event firing and us
            // reading it; treat as nothing to reconcile.
            Err(_) => return Ok(()),
        };
        if !canon_path.starts_with(&canon_root) {
            return Ok(());
        }

        let rel = match canon_path.strip_prefix(&canon_root) {
            Ok(r) => r.to_string_lossy().into_owned(),
            Err(_) => return Ok(()),
        };

        let content = match tokio::task::spawn_blocking({
            let p = canon_path.clone();
            move || std::fs::read_to_string(p)
        })
        .await
        .context("spawn_blocking fs::read_to_string")?
        {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(anyhow::anyhow!(e).context("fs::read_to_string")),
        };

        let sha = sha256_hex(content.as_bytes());
        let is_self_write = {
            let cache = self.seen_sha.lock();
            cache
                .get(&canon_path)
                .map(|cached| cached == &sha)
                .unwrap_or(false)
        };
        if is_self_write {
            tracing::trace!(path = %canon_path.display(), "watcher: self-write, skipping");
            return Ok(());
        }

        // Refresh cache so the next external event matching this content
        // (e.g. user saved the same content twice) doesn't double-record.
        self.seen_sha
            .lock()
            .insert(canon_path.clone(), sha.clone());

        let now = now_ms();
        let record = self
            .store
            .insert_blackboard_op(NewBlackboardOp {
                agent_id: None,
                op: "external".into(),
                path: rel.clone(),
                content,
                sha256: sha.clone(),
                at: now,
            })
            .await
            .context("store.insert_blackboard_op(external)")?;

        let _ = self.events_tx.send(SwarmEvent::BlackboardChanged {
            id: record.id,
            agent_id: None,
            op: "external".into(),
            path: rel,
            sha256: sha,
            at: now,
        });

        Ok(())
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    let out = h.finalize();
    let mut s = String::with_capacity(out.len() * 2);
    for b in out {
        use std::fmt::Write;
        let _ = write!(s, "{b:02x}");
    }
    s
}

fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
