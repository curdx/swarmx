//! M6b WakeCoordinator: wake agents the moment a blackboard key they
//! declared `depends_on` is written.
//!
//! The gap this closes (observed in M6a run #3): when an agent finishes
//! a turn with an empty mailbox and its prompt told it to wait for some
//! `*.done` key, the Stop hook noop's, the agent sits idle, and later
//! writes to that key never resurrect it. wake-check is a Stop *hook* —
//! it only fires when an agent is in the act of stopping; it cannot
//! restart an already-stopped one.
//!
//! Design (validated against the 2025 blackboard-architecture revival
//! papers, Han et al. arXiv 2507.01701 and Salemi et al. arXiv
//! 2510.01285): the orchestrator owns wakeup. A single tokio task
//! subscribes to `Swarm`'s broadcast channel, watches for
//! `SwarmEvent::BlackboardChanged`, and for each subscribed agent does
//! two things:
//!
//!   1. **Mailbox write** (source of truth): `Swarm::send_message` posts
//!      a `kind="wake"` note from `"system"` to the agent. Even if the
//!      PTY kick below fails, the next time the agent stops, wake-check
//!      will see this unread note and force it to keep going. Idempotent.
//!
//!   2. **PTY kick** (belt-and-suspenders): byte-blast `\x15<short>\r`
//!      into the agent's PTY input channel. Ctrl-U clears any residual
//!      text in the TUI's input buffer; the short text + carriage return
//!      submits a fresh user turn so the agent does NOT have to wait for
//!      the next natural Stop event. Best-effort: failure is logged and
//!      not propagated.
//!
//! The writer is excluded from the wakeup set so BE doesn't wake itself
//! the instant it writes its own `backend.done`. External edits
//! (`agent_id: None`) wake everyone subscribed.

use anyhow::{anyhow, Result};
use bytes::Bytes;
use flockmux_protocol::ws_swarm::SwarmEvent;
use flockmux_swarm::{NewMessage, Swarm};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;

use crate::registry::Registry;

/// Per-agent dependency table. The wake task reads this on every
/// blackboard event; spell launch writes to it once per agent.
///
/// Per-agent (agent_id → keys) rather than inverted (key → Vec<agent_id>)
/// because cleanup on agent kill is O(1) — the common path. Lookup on
/// event is a linear scan over ≤ ~10 entries per spell, which is fine.
pub type WakeSubs = Arc<RwLock<HashMap<String, Vec<String>>>>;

/// Inserts `agent_id → keys` into the subscription table. No-op when
/// `keys` is empty (we don't bother storing zero-dep agents).
pub async fn register_wake_subs(subs: &WakeSubs, agent_id: String, keys: Vec<String>) {
    if keys.is_empty() {
        return;
    }
    let mut w = subs.write().await;
    w.insert(agent_id, keys);
}

/// Removes an agent's subscription. Called from the kill handler so
/// blackboard writes to dead agents' depended-on keys don't try to wake
/// a registry slot that has been dropped.
pub async fn unregister_wake_subs(subs: &WakeSubs, agent_id: &str) {
    let mut w = subs.write().await;
    w.remove(agent_id);
}

/// Pure function (no IO, no async) extracted for unit testing: given a
/// snapshot of the subscription table, the just-written key, and the
/// writer (if any), produce the list of agent_ids to wake. Writer is
/// excluded by design — `BE writes backend.done` should not wake BE.
pub fn select_targets(
    subs: &HashMap<String, Vec<String>>,
    key: &str,
    writer: Option<&str>,
) -> Vec<String> {
    subs.iter()
        .filter(|(aid, keys)| {
            // Skip the writer itself; tooling that legitimately watches
            // its own key would create wake-storms otherwise.
            if writer.is_some_and(|w| w == aid.as_str()) {
                return false;
            }
            keys.iter().any(|k| k == key)
        })
        .map(|(aid, _)| aid.clone())
        .collect()
}

/// Injects `\x15<short text>\r` into an agent's PTY input. Matches the
/// existing pattern in `rest.rs::run_spell` spell bootstrap injection
/// (parking_lot guard held briefly to clone the sender, sender held
/// across `await`). Best-effort: caller logs failures.
pub async fn inject_wake_kick(
    registry: &Registry,
    agent_id: &str,
    key: &str,
) -> Result<()> {
    let slot = registry
        .get(agent_id)
        .ok_or_else(|| anyhow!("no registry slot for `{agent_id}` — agent may have exited"))?;
    let input_tx = slot.lock().input_tx.clone();
    let body = format!("\x15blackboard `{key}` updated; please check\r");
    input_tx
        .send(Bytes::from(body))
        .await
        .map_err(|e| anyhow!("PTY input_tx send failed: {e}"))
}

pub struct WakeCoordinator {
    swarm: Arc<Swarm>,
    registry: Registry,
    subs: WakeSubs,
}

impl WakeCoordinator {
    /// Spawns the wake task and returns its JoinHandle. The handle is
    /// dropped immediately by `main.rs` since the task runs for the
    /// lifetime of the process (it exits only when the broadcast channel
    /// closes, which happens at server shutdown).
    pub fn spawn(swarm: Arc<Swarm>, registry: Registry, subs: WakeSubs) -> JoinHandle<()> {
        let me = Self {
            swarm,
            registry,
            subs,
        };
        tokio::spawn(me.run())
    }

    async fn run(self) {
        use tokio::sync::broadcast::error::RecvError;
        let mut rx = self.swarm.subscribe();
        loop {
            match rx.recv().await {
                Ok(SwarmEvent::BlackboardChanged {
                    agent_id: writer,
                    path,
                    ..
                }) => {
                    let targets = {
                        let map = self.subs.read().await;
                        select_targets(&map, &path, writer.as_deref())
                    };
                    for target in targets {
                        self.deliver_wake(&target, &path).await;
                    }
                }
                Ok(_) => {} // ignore non-blackboard events
                Err(RecvError::Lagged(n)) => {
                    // Broadcast buffer overflow — should never happen in
                    // practice (we'd have to lag by hundreds of events).
                    // Log and keep going; missing one wake is recoverable
                    // because the *next* write to the same key will catch
                    // up, and the mailbox isn't lost.
                    tracing::warn!(lagged = n, "wake coordinator broadcast lagged");
                }
                Err(RecvError::Closed) => {
                    tracing::info!("wake coordinator: broadcast closed, exiting");
                    break;
                }
            }
        }
    }

    async fn deliver_wake(&self, target: &str, key: &str) {
        let now = now_ms();
        // 1) Mailbox: source of truth. If this fails the wake mechanism
        //    is broken for this delivery — we'd be hoping the agent's
        //    own polling catches the key (back to M6a behaviour). Log
        //    loudly but don't panic.
        let msg = NewMessage {
            from_agent: "system".into(),
            to_agent: target.into(),
            kind: "wake".into(),
            body: format!("blackboard `{key}` updated; please check"),
            sent_at: now,
            in_reply_to: None,
        };
        if let Err(err) = self.swarm.send_message(msg).await {
            tracing::warn!(?err, target, key, "wake send_message failed");
            // Don't even try the PTY kick if mailbox failed — the kick
            // alone won't tell Claude what changed, so it'd be confusing
            // noise. Bail.
            return;
        }

        // 2) PTY kick: belt-and-suspenders. Failures are tolerable: next
        //    Stop hook fire will see the unread mailbox entry and wake.
        if let Err(err) = inject_wake_kick(&self.registry, target, key).await {
            tracing::warn!(?err, target, key, "wake PTY inject failed");
            // Reap the stale subscription so we don't churn on every
            // future write to this key. The agent is gone anyway; if it
            // comes back, the next spell launch will re-register.
            unregister_wake_subs(&self.subs, target).await;
            return;
        }

        tracing::info!(target, key, "wake delivered");
    }
}

fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Detect cycles in a {role → depends_on} graph using BFS/DFS. Returns
/// `Ok(())` if acyclic, `Err` listing the cycle path otherwise. Called
/// from `run_spell` before any agent is spawned so we fail fast on bad
/// manifests rather than producing 3 agents that deadlock waiting on
/// each other.
///
/// Note: `depends_on` values are blackboard *keys* (e.g. `frontend.done`)
/// not role ids. To detect cycles we map each key back to the role that
/// declares its `handoff_signal` equal to that key. Keys with no
/// producing role are treated as external inputs (no cycle through them).
pub fn detect_depends_on_cycles(
    role_handoff: &HashMap<String, String>, // role_name → handoff_signal (the key it produces)
    role_depends: &HashMap<String, Vec<String>>, // role_name → depends_on keys
) -> Result<()> {
    // Reverse-lookup: which role produces this key?
    let key_to_role: HashMap<&str, &str> = role_handoff
        .iter()
        .filter(|(_, k)| !k.is_empty())
        .map(|(r, k)| (k.as_str(), r.as_str()))
        .collect();

    // For each role, do a DFS through its depended-on keys' producers.
    // If we ever revisit the starting role, we have a cycle.
    let role_names: Vec<&str> = role_depends.keys().map(String::as_str).collect();
    for start in &role_names {
        let mut stack: Vec<&str> = vec![*start];
        let mut visiting: std::collections::HashSet<&str> = std::collections::HashSet::new();
        while let Some(current) = stack.pop() {
            if !visiting.insert(current) {
                continue;
            }
            let deps = match role_depends.get(current) {
                Some(d) => d,
                None => continue,
            };
            for dep_key in deps {
                if let Some(producer) = key_to_role.get(dep_key.as_str()) {
                    if *producer == *start {
                        return Err(anyhow!(
                            "depends_on cycle: role `{start}` (via key `{dep_key}`) depends back on itself"
                        ));
                    }
                    stack.push(*producer);
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_subs(entries: &[(&str, &[&str])]) -> HashMap<String, Vec<String>> {
        entries
            .iter()
            .map(|(aid, keys)| {
                (
                    aid.to_string(),
                    keys.iter().map(|k| k.to_string()).collect(),
                )
            })
            .collect()
    }

    #[test]
    fn select_targets_empty_map_returns_empty() {
        let m: HashMap<String, Vec<String>> = HashMap::new();
        assert!(select_targets(&m, "any.key", None).is_empty());
        assert!(select_targets(&m, "any.key", Some("nobody")).is_empty());
    }

    #[test]
    fn select_targets_picks_only_subscribers_of_key() {
        let m = build_subs(&[
            ("test-a", &["frontend.done", "backend.done"]),
            ("fe-a", &[]),
            ("be-a", &[]),
        ]);
        let mut t = select_targets(&m, "backend.done", None);
        t.sort();
        assert_eq!(t, vec!["test-a".to_string()]);
    }

    #[test]
    fn select_targets_excludes_writer() {
        let m = build_subs(&[
            ("test-a", &["x.done"]),
            ("self-watcher", &["x.done"]),
        ]);
        let t = select_targets(&m, "x.done", Some("self-watcher"));
        assert_eq!(t, vec!["test-a".to_string()]);
    }

    #[test]
    fn select_targets_external_edit_wakes_all_subscribers() {
        // writer = None means an external (filesystem) edit; everyone
        // subscribed to the key should be woken.
        let m = build_subs(&[
            ("a", &["k"]),
            ("b", &["k"]),
            ("c", &["other"]),
        ]);
        let mut t = select_targets(&m, "k", None);
        t.sort();
        assert_eq!(t, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn select_targets_no_match_returns_empty() {
        let m = build_subs(&[("a", &["foo.done"])]);
        assert!(select_targets(&m, "bar.done", None).is_empty());
    }

    #[tokio::test]
    async fn register_and_unregister_round_trip() {
        let subs: WakeSubs = Arc::new(RwLock::new(HashMap::new()));
        register_wake_subs(&subs, "a".into(), vec!["k1".into(), "k2".into()]).await;
        assert_eq!(subs.read().await.get("a").map(|v| v.len()), Some(2));
        unregister_wake_subs(&subs, "a").await;
        assert!(subs.read().await.get("a").is_none());
    }

    #[tokio::test]
    async fn register_ignores_empty_keys() {
        let subs: WakeSubs = Arc::new(RwLock::new(HashMap::new()));
        register_wake_subs(&subs, "a".into(), vec![]).await;
        assert!(subs.read().await.get("a").is_none(),
            "empty depends_on shouldn't pollute the map");
    }

    #[tokio::test]
    async fn inject_wake_kick_errors_on_missing_agent() {
        let registry = Registry::new();
        let err = inject_wake_kick(&registry, "ghost", "k").await.unwrap_err();
        assert!(format!("{err:#}").contains("ghost"));
    }

    // ── cycle detection ─────────────────────────────────────────────────

    fn map(entries: &[(&str, &str)]) -> HashMap<String, String> {
        entries.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }
    fn mapv(entries: &[(&str, &[&str])]) -> HashMap<String, Vec<String>> {
        entries
            .iter()
            .map(|(k, v)| (k.to_string(), v.iter().map(|s| s.to_string()).collect()))
            .collect()
    }

    #[test]
    fn cycle_detection_passes_acyclic_fullstack_layout() {
        // The actual M6a topology: test depends on frontend+backend; nobody
        // depends on test.
        let handoff = map(&[
            ("frontend", "frontend.done"),
            ("backend", "backend.done"),
            ("test", "test.passed"),
        ]);
        let deps = mapv(&[
            ("frontend", &[]),
            ("backend", &[]),
            ("test", &["frontend.done", "backend.done"]),
        ]);
        assert!(detect_depends_on_cycles(&handoff, &deps).is_ok());
    }

    #[test]
    fn cycle_detection_catches_self_loop() {
        let handoff = map(&[("a", "a.done")]);
        let deps = mapv(&[("a", &["a.done"])]);
        let err = detect_depends_on_cycles(&handoff, &deps).unwrap_err();
        assert!(format!("{err:#}").contains("cycle"));
    }

    #[test]
    fn cycle_detection_catches_two_role_cycle() {
        let handoff = map(&[("a", "a.done"), ("b", "b.done")]);
        let deps = mapv(&[("a", &["b.done"]), ("b", &["a.done"])]);
        let err = detect_depends_on_cycles(&handoff, &deps).unwrap_err();
        assert!(format!("{err:#}").to_lowercase().contains("cycle"));
    }

    #[test]
    fn cycle_detection_ignores_unrooted_keys() {
        // depends_on points at a key nobody produces — treated as
        // external input, no cycle.
        let handoff = map(&[("a", "a.done")]);
        let deps = mapv(&[("a", &["external.signal"])]);
        assert!(detect_depends_on_cycles(&handoff, &deps).is_ok());
    }
}
