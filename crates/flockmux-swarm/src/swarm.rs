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
use flockmux_protocol::rest::{
    AgentActivityRecord, ThoughtTrace as ProtocolThoughtTrace,
    ThoughtTraceStep as ProtocolThoughtTraceStep,
};
use flockmux_protocol::ws_swarm::SwarmEvent;
use flockmux_storage::{
    BlackboardOpRecord, MessageRecord as StoreMessageRecord, NewBlackboardOp,
    NewMessage as StoreNewMessage, NewThoughtTrace, NewThoughtTraceEvent, Store,
    ThoughtTraceRecord, ThoughtTraceStep,
};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};

/// Max recent tool-level activity rows retained per agent for the drawer's
/// Activity-tab backfill (`GET /api/agent/:id/activity`). Bounded so a
/// long-running worker can't grow this unboundedly; matches the frontend's
/// in-memory cap so backfill + live stream agree on how much history exists.
const MAX_ACTIVITY_LOG: usize = 100;
const USER_AGENT_ID: &str = "user";
const SYSTEM_AGENT_ID: &str = "system";
const MAX_TRACE_AGENT_LINEAGE: usize = 8;

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
    pub in_reply_to: Option<i64>,
    /// Structured metadata for system-generated messages (see
    /// `flockmux_storage::NewMessage::meta`). `None` for agent free-text.
    pub meta: Option<serde_json::Value>,
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
    /// Per-agent bounded ring of recent tool-level activity, fed by the
    /// transcript tailer alongside the WS broadcast. Served by `GET
    /// /api/agent/:id/activity` so the drawer's Activity tab can backfill on a
    /// cold open (the WS stream is forward-only). In-memory only — dead agents'
    /// "what did it do" is covered by their recording.
    activity_log: DashMap<String, Mutex<VecDeque<AgentActivityRecord>>>,
}

fn is_user_to_agent(from: &str, to: &str) -> bool {
    from == USER_AGENT_ID && to != USER_AGENT_ID && to != SYSTEM_AGENT_ID
}

fn is_agent_to_user(from: &str, to: &str) -> bool {
    to == USER_AGENT_ID && from != USER_AGENT_ID && from != SYSTEM_AGENT_ID
}

fn start_trace_steps(at: i64) -> Vec<ThoughtTraceStep> {
    vec![
        ThoughtTraceStep {
            phase: "understand".into(),
            label: "理解用户请求".into(),
            source: "system".into(),
            at,
        },
        ThoughtTraceStep {
            phase: "context".into(),
            label: "检查当前对话、工作区和任务状态".into(),
            source: "system".into(),
            at,
        },
        ThoughtTraceStep {
            phase: "route".into(),
            label: "判断由自己处理还是分配给 worker".into(),
            source: "system".into(),
            at,
        },
    ]
}

fn done_trace_steps(at: i64) -> Vec<ThoughtTraceStep> {
    let mut steps = start_trace_steps(at);
    steps.push(ThoughtTraceStep {
        phase: "answer".into(),
        label: "整理结果并回复用户".into(),
        source: "system".into(),
        at,
    });
    steps
}

fn steps_json(steps: &[ThoughtTraceStep]) -> String {
    serde_json::to_string(steps).unwrap_or_else(|_| "[]".into())
}

fn step_to_event(step: &ThoughtTraceStep) -> NewThoughtTraceEvent {
    NewThoughtTraceEvent {
        phase: step.phase.clone(),
        label: step.label.clone(),
        source: step.source.clone(),
        at: step.at,
        meta: None,
    }
}

fn trace_to_protocol(trace: &ThoughtTraceRecord) -> ProtocolThoughtTrace {
    let summary = serde_json::from_str::<Vec<ThoughtTraceStep>>(&trace.summary_json)
        .unwrap_or_default()
        .into_iter()
        .map(|s| ProtocolThoughtTraceStep {
            phase: s.phase,
            label: s.label,
            source: s.source,
            at: s.at,
        })
        .collect();
    ProtocolThoughtTrace {
        id: trace.id.clone(),
        trigger_message_id: trace.trigger_message_id,
        response_message_id: trace.response_message_id,
        agent_id: trace.agent_id.clone(),
        workspace_id: trace.workspace_id.clone(),
        thread_id: trace.thread_id.clone(),
        status: trace.status.clone(),
        started_at: trace.started_at,
        completed_at: trace.completed_at,
        summary,
        updated_at: trace.updated_at,
    }
}

fn short_trace_label(label: &str) -> String {
    const MAX_CHARS: usize = 96;
    let trimmed = label.trim();
    let mut out = String::new();
    for (idx, ch) in trimmed.chars().enumerate() {
        if idx >= MAX_CHARS {
            out.push('…');
            break;
        }
        out.push(ch);
    }
    out
}

fn activity_to_trace_event(rec: &AgentActivityRecord) -> Option<NewThoughtTraceEvent> {
    let (phase, prefix) = match rec.phase.as_str() {
        "ok" => ("tool_ok", "完成工具"),
        "error" => ("tool_error", "工具失败"),
        _ => return None,
    };
    Some(NewThoughtTraceEvent {
        phase: phase.into(),
        label: format!("{prefix}: {}", short_trace_label(&rec.label)),
        source: "agent".into(),
        at: rec.at,
        meta: Some(serde_json::json!({
            "agent_id": rec.agent_id,
            "kind": rec.kind,
            "seq": rec.seq,
            "duration_ms": rec.duration_ms,
        })),
    })
}

async fn trace_agent_candidates(store: Arc<Store>, agent_id: String) -> Vec<String> {
    let mut out = vec![agent_id.clone()];
    let mut cur = agent_id;
    for _ in 0..MAX_TRACE_AGENT_LINEAGE {
        let rows = match store.list_workers_by_ids(vec![cur.clone()]).await {
            Ok(rows) => rows,
            Err(err) => {
                tracing::debug!(?err, agent = %cur, "thought trace parent lookup failed");
                break;
            }
        };
        let Some(worker) = rows.get(&cur) else {
            break;
        };
        if worker.parent_agent_id.is_empty() || out.contains(&worker.parent_agent_id) {
            break;
        }
        cur = worker.parent_agent_id.clone();
        out.push(cur.clone());
    }
    out
}

impl Swarm {
    pub fn new(store: Arc<Store>, blackboard_root: PathBuf) -> Arc<Self> {
        // Capacity for the shared SwarmEvent ring (messages + blackboard +
        // lifecycle all flow through here). 1024 gives the WakeCoordinator
        // ample headroom against a message/blackboard burst so it rarely
        // `Lagged`s; if it ever does, the coordinator reconciles depends_on
        // against the blackboard and re-wakes (F12), so a drop is recoverable
        // rather than a permanent stall.
        let (events_tx, _events_rx) = broadcast::channel(1024);
        Arc::new(Self {
            store,
            inboxes: DashMap::new(),
            blackboard_root,
            events_tx,
            seen_sha: Mutex::new(HashMap::new()),
            activity_log: DashMap::new(),
        })
    }

    /// Record one tool-level activity for `agent_id` into the bounded in-memory
    /// ring the `GET /api/agent/:id/activity` backfill serves. Collapses by
    /// `seq` so a `running` row is replaced in place by its later `ok`/`error`
    /// (the same dedupe the UI does), keeping one row per step. Called by the
    /// transcript tailer alongside `publish_event`.
    pub fn record_activity(&self, agent_id: &str, rec: AgentActivityRecord) {
        let trace_event = activity_to_trace_event(&rec);
        let entry = self
            .activity_log
            .entry(agent_id.to_string())
            .or_insert_with(|| Mutex::new(VecDeque::new()));
        let mut q = entry.lock();
        if let Some(slot) = q.iter_mut().find(|r| r.seq == rec.seq) {
            *slot = rec.clone();
        } else {
            if q.len() >= MAX_ACTIVITY_LOG {
                q.pop_front();
            }
            q.push_back(rec.clone());
        }
        drop(q);
        // P1: persist this step so `GET /api/agent/:id/activity` survives a cold
        // load / reconnect / restart (the ring is per-server-session). Upsert by
        // (agent_id, seq) mirrors the ring's collapse-by-seq. Fire-and-forget,
        // like the thought-trace persist below.
        {
            let store = self.store.clone();
            let aid = agent_id.to_string();
            let (seq, kind, label, phase, duration_ms, at) = (
                rec.seq,
                rec.kind.clone(),
                rec.label.clone(),
                rec.phase.clone(),
                rec.duration_ms,
                rec.at,
            );
            tokio::spawn(async move {
                if let Err(err) = store
                    .insert_agent_activity(aid, seq, kind, label, phase, duration_ms, at)
                    .await
                {
                    tracing::warn!(?err, "persist agent activity failed");
                }
            });
        }
        if let Some(event) = trace_event {
            let store = self.store.clone();
            let agent_id = agent_id.to_string();
            tokio::spawn(async move {
                let candidates = trace_agent_candidates(store.clone(), agent_id).await;
                if let Err(err) = store.append_thought_trace_event(candidates, event).await {
                    tracing::warn!(?err, "append thought trace activity failed");
                }
            });
        }
    }

    /// Recent activity for `agent_id`, oldest-first (the drawer renders newest
    /// at the bottom). Empty for agents we don't tail or that haven't acted.
    pub fn recent_activity(&self, agent_id: &str) -> Vec<AgentActivityRecord> {
        self.activity_log
            .get(agent_id)
            .map(|e| e.lock().iter().cloned().collect())
            .unwrap_or_default()
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
    ///
    /// The single choke point for ALL sends, so it's where a message's
    /// direction (`thread_id`) is derived: the sender's thread if it's a known
    /// agent, else the recipient's. That keeps the chat hard-scoped per
    /// direction without the caller (or the agent) having to know its thread.
    pub async fn send_message(&self, msg: NewMessage) -> Result<StoreMessageRecord> {
        let thread_id = match self.store.agent_thread_id(msg.from_agent.clone()).await {
            Ok(Some(t)) => Some(t),
            // Sender isn't a thread-bound agent (e.g. "user"/"system") — fall
            // back to the recipient's direction so a user→orchestrator message
            // is tagged with that orchestrator's direction.
            _ => self
                .store
                .agent_thread_id(msg.to_agent.clone())
                .await
                .ok()
                .flatten(),
        };
        let record = self
            .store
            .insert_message_threaded(
                StoreNewMessage {
                    from_agent: msg.from_agent.clone(),
                    to_agent: msg.to_agent.clone(),
                    kind: msg.kind.clone(),
                    body: msg.body.clone(),
                    sent_at: msg.sent_at,
                    in_reply_to: msg.in_reply_to,
                    meta: msg.meta.clone(),
                },
                thread_id,
            )
            .await
            .context("store.insert_message")?;
        let mut record = record;

        if is_user_to_agent(&record.from_agent, &record.to_agent) {
            let workspace_id = self
                .store
                .agent_workspace_id(record.to_agent.clone())
                .await
                .ok()
                .flatten();
            let steps = start_trace_steps(record.sent_at);
            let events = steps
                .iter()
                .map(step_to_event)
                .collect::<Vec<NewThoughtTraceEvent>>();
            match self
                .store
                .start_thought_trace(
                    NewThoughtTrace {
                        trigger_message_id: record.id,
                        agent_id: record.to_agent.clone(),
                        workspace_id,
                        thread_id: record.thread_id.clone(),
                        started_at: record.sent_at,
                        summary_json: steps_json(&steps),
                    },
                    events,
                )
                .await
            {
                Ok(trace) => record.thought_trace = Some(trace),
                Err(err) => {
                    tracing::warn!(?err, message_id = record.id, "start thought trace failed")
                }
            }
        } else if is_agent_to_user(&record.from_agent, &record.to_agent) {
            let steps = done_trace_steps(record.sent_at);
            let events = steps
                .iter()
                .map(step_to_event)
                .collect::<Vec<NewThoughtTraceEvent>>();
            match self
                .store
                .complete_latest_thought_trace(
                    record.from_agent.clone(),
                    record.thread_id.clone(),
                    record.id,
                    record.sent_at,
                    steps_json(&steps),
                    events,
                )
                .await
            {
                Ok(Some(trace)) => record.thought_trace = Some(trace),
                Ok(None) => {}
                Err(err) => tracing::warn!(
                    ?err,
                    message_id = record.id,
                    "complete thought trace failed"
                ),
            }
        }

        // Broadcast first so subscribers see it even if delivery fails.
        let _ = self.events_tx.send(SwarmEvent::Message {
            id: record.id,
            from_agent: record.from_agent.clone(),
            to_agent: record.to_agent.clone(),
            kind: record.kind.clone(),
            body: record.body.clone(),
            sent_at: record.sent_at,
            in_reply_to: record.in_reply_to,
            thread_id: record.thread_id.clone(),
            meta: record.meta.clone(),
            thought_trace: record.thought_trace.as_ref().map(trace_to_protocol),
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
            if let Err(e) = self.store.mark_delivered(vec![record.id], now_ms()).await {
                tracing::warn!(?e, "mark_delivered failed");
            }
        }
        Ok(record)
    }

    /// Mark a batch of messages as read on behalf of `to_agent`. Returns
    /// the ids that this call actually updated (idempotent — repeats are a
    /// no-op). Broadcasts a `MessageRead` event so subscribers (UI badge,
    /// future read-receipts UI) can decrement live.
    pub async fn mark_read(&self, to_agent: String, ids: Vec<i64>) -> Result<Vec<i64>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let at = now_ms();
        let marked = self
            .store
            .mark_read(ids, to_agent.clone(), at)
            .await
            .context("store.mark_read")?;
        if !marked.is_empty() {
            let _ = self.events_tx.send(SwarmEvent::MessageRead {
                ids: marked.clone(),
                to_agent,
                at,
            });
        }
        Ok(marked)
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
        self.seen_sha.lock().insert(target.clone(), sha.clone());

        // Run the actual fs write off the runtime thread. Atomic write
        // (tmp → fsync → rename) so a crash mid-write can't leave a truncated
        // blackboard file — orchestrator/worker read these to make decisions,
        // a half-written task.ledger.md would corrupt that. The temp name ends
        // in `.tmp` so the blackboard watcher ignores it (watcher.rs skips
        // *.tmp / *.swp) rather than firing a spurious self-loop event.
        let target_for_write = target.clone();
        let content_for_write = content.to_owned();
        tokio::task::spawn_blocking(move || -> Result<()> {
            use std::io::Write;
            static TMP_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let seq = TMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let fname = target_for_write
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "blackboard".into());
            let mut tmp = target_for_write.clone();
            // Unique per (pid, seq) so concurrent writes to the same path don't
            // clobber each other's temp; same dir so the rename is atomic.
            tmp.set_file_name(format!(
                ".{fname}.flockmux.{}.{seq}.tmp",
                std::process::id()
            ));

            let write_then_rename = || -> Result<()> {
                {
                    let mut f = std::fs::File::create(&tmp)
                        .with_context(|| format!("create tmp {}", tmp.display()))?;
                    f.write_all(content_for_write.as_bytes())
                        .with_context(|| format!("write tmp {}", tmp.display()))?;
                    f.sync_all().ok();
                }
                std::fs::rename(&tmp, &target_for_write).with_context(|| {
                    format!("rename {} -> {}", tmp.display(), target_for_write.display())
                })
            };
            let res = write_then_rename();
            if res.is_err() {
                let _ = std::fs::remove_file(&tmp); // best-effort cleanup
            }
            res
        })
        .await
        .context("spawn_blocking atomic write")??;

        let rel_owned = rel_path.to_string();
        let now = now_ms();
        // The content is now durably on disk. The op-log insert is for history
        // + discovery (swarm_list_blackboard) + the FTS index — it is NOT
        // load-bearing for the wake. So a failed insert (SQLITE_BUSY past the
        // retry budget, disk-full, …) must NOT swallow the BlackboardChanged:
        // dropping it would silently strand every depends_on subscriber (F6).
        // On failure we log, still broadcast (with a sentinel id = -1), and
        // still return Ok — the write genuinely happened; only the op-log row
        // is missing (a future reconcile can backfill it).
        match self
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
        {
            Ok(record) => {
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
            Err(e) => {
                tracing::warn!(
                    ?e,
                    path = %rel_owned,
                    "blackboard op-log insert failed; content IS on disk — broadcasting the \
                     wake anyway (id=-1) so dependents aren't stranded (F6)"
                );
                let _ = self.events_tx.send(SwarmEvent::BlackboardChanged {
                    id: -1,
                    agent_id: agent_id.clone(),
                    op: "write".into(),
                    path: rel_owned.clone(),
                    sha256: sha.clone(),
                    at: now,
                });
                Ok(BlackboardOpRecord {
                    id: -1,
                    agent_id,
                    op: "write".into(),
                    path: rel_owned,
                    content: content.to_string(),
                    sha256: sha,
                    at: now,
                })
            }
        }
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

    /// Boot-time op-log reconcile (completes F6). A failed `insert_blackboard_op`
    /// leaves a file on disk with NO op-log row — `write_blackboard` keeps the
    /// content and still broadcasts the wake, but the row (and thus the
    /// `swarm_list_blackboard` discovery entry) is lost. After a restart that
    /// path would be invisible to discovery. Here we walk the blackboard tree
    /// and backfill an op row for any file the op-log doesn't know about.
    /// Idempotent (only genuinely-missing paths), and does NOT broadcast (boot:
    /// no live subscribers, and a reconcile is not a fresh write event).
    /// Returns the number of rows backfilled.
    pub async fn reconcile_oplog_from_disk(&self) -> Result<usize> {
        // One query: every path the op-log already knows (latest-per-path).
        let known: std::collections::HashSet<String> = self
            .store
            .list_blackboard_ops(None)
            .await
            .context("list_blackboard_ops for reconcile")?
            .into_iter()
            .map(|r| r.path)
            .collect();

        let root = self.blackboard_root.clone();
        let files = tokio::task::spawn_blocking(move || collect_blackboard_files(&root))
            .await
            .context("spawn_blocking collect_blackboard_files")??;

        let mut backfilled = 0usize;
        for (rel, content, sha) in files {
            if known.contains(&rel) {
                continue;
            }
            let at = now_ms();
            match self
                .store
                .insert_blackboard_op(NewBlackboardOp {
                    agent_id: None,
                    op: "reconcile".into(),
                    path: rel.clone(),
                    content,
                    sha256: sha,
                    at,
                })
                .await
            {
                Ok(_) => {
                    backfilled += 1;
                    tracing::info!(path = %rel, "reconcile: backfilled blackboard op row missing from the log");
                }
                Err(e) => tracing::warn!(?e, path = %rel, "reconcile: backfill insert failed"),
            }
        }
        Ok(backfilled)
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
        self.seen_sha.lock().insert(canon_path.clone(), sha.clone());

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

/// Recursively list every regular file under `root` as
/// `(rel_path, content, sha256)`, with `/`-separated relative paths matching
/// the keys agents pass to the blackboard. Skips dot-prefixed entries
/// (editor / `.git` noise) and unreadable/binary files. Sync — invoked inside
/// `spawn_blocking` by the boot reconcile.
fn collect_blackboard_files(root: &Path) -> Result<Vec<(String, String, String)>> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let rd = match std::fs::read_dir(&dir) {
            Ok(r) => r,
            Err(_) => continue,
        };
        for entry in rd.flatten() {
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            let path = entry.path();
            let ft = match entry.file_type() {
                Ok(t) => t,
                Err(_) => continue,
            };
            if ft.is_dir() {
                stack.push(path);
                continue;
            }
            if !ft.is_file() {
                continue;
            }
            let rel = match path.strip_prefix(root) {
                Ok(r) => r.to_string_lossy().into_owned(),
                Err(_) => continue,
            };
            match std::fs::read_to_string(&path) {
                Ok(content) => {
                    let sha = sha256_hex(content.as_bytes());
                    out.push((rel, content, sha));
                }
                Err(_) => continue, // binary / unreadable — not a text blackboard key
            }
        }
    }
    Ok(out)
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
