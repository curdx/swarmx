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

/// M6c step 5: per-agent expected handoff-signal + spawn time. When
/// the agent exits without writing its handoff_signal we synthesize a
/// `<signal>.error` so downstream dependents can fail loudly instead
/// of hanging. The spawn timestamp lets us distinguish a fresh write
/// (this run's agent succeeded) from a stale leftover on disk (a
/// previous run's `<signal>` row still in the blackboard) so we don't
/// silently skip writing `.error` because yesterday's run happened to
/// produce the same key name.
///
/// Only agents whose role declares a non-empty `handoff_signal` are
/// registered — inline-only roles (critic-loop's writer / critic /
/// editor) don't get exit-fallback because there's no canonical signal
/// to mark as failed.
/// How long to wait after a worker writes its `handoff_signal` before
/// the auto-kill fires. 5 seconds is long enough for claude/codex to
/// finish printing the final scrollback + a `swarm_send_message`
/// summary back to the orchestrator (typically <2s), but short enough
/// that the UI ground-truth converges quickly. Tune up if recording
/// playback shows truncation; tune down if zombie PTYs feel sluggish.
const AUTO_KILL_GRACE_MS: u64 = 5_000;

/// Local now-ms helper; mirrors `routes::rest::now_ms` so we don't have
/// to cross-import.
fn now_ms_local() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[derive(Debug, Clone)]
pub struct ExitKey {
    /// Role name — used to name the synthesized failure key as
    /// `<role>.error` (matches the convention agents already use when
    /// they self-write a failure, e.g. `frontend.error`, so test agent
    /// prompts only need to check ONE key name regardless of whether
    /// FE crashed or self-aborted).
    pub role: String,
    /// Blackboard key the role was supposed to produce. Used (a) for
    /// the freshness check ("did the agent actually write this before
    /// dying?") and (b) to identify which agents to wake when we
    /// synthesize the error — we wake subscribers of THIS key, not of
    /// `<role>.error`, because that's what `depends_on` actually lists.
    pub handoff_signal: String,
    /// When the registration was made. A blackboard write to
    /// `handoff_signal` older than this is a leftover from a previous
    /// run on the same workspace and must NOT short-circuit the
    /// .error synthesis.
    pub spawned_at_ms: i64,
}
pub type ExitKeys = Arc<RwLock<HashMap<String, ExitKey>>>;

/// Recognises blackboard keys that should fan-out to wake the base
/// key's subscribers in addition to their literal name. Today only
/// `.error` and `.failed` suffixes get this treatment — both indicate
/// "the producer for the base key isn't coming". An empty Vec means
/// "no fan-out, treat as a regular key".
fn base_key_aliases(path: &str) -> Vec<String> {
    for suffix in [".error", ".failed"] {
        if let Some(base) = path.strip_suffix(suffix) {
            if !base.is_empty() {
                return vec![base.to_string()];
            }
        }
    }
    Vec::new()
}

/// Inserts `agent_id → keys` into the subscription table. No-op when
/// `keys` is empty (we don't bother storing zero-dep agents).
pub async fn register_wake_subs(subs: &WakeSubs, agent_id: String, keys: Vec<String>) {
    if keys.is_empty() {
        return;
    }
    let mut w = subs.write().await;
    w.insert(agent_id, keys);
}

/// Append a single key to `agent_id`'s subscription list, creating the
/// entry if it doesn't exist. Idempotent — a duplicate key is silently
/// dropped. Used by `spawn_worker` to make the spawning agent (= the
/// Magentic-One orchestrator) subscribe to the new worker's
/// `handoff_signal` without clobbering any subscriptions the
/// orchestrator already had from prior spawns.
pub async fn append_wake_sub(subs: &WakeSubs, agent_id: String, key: String) {
    if key.is_empty() {
        return;
    }
    let mut w = subs.write().await;
    let entry = w.entry(agent_id).or_default();
    if !entry.contains(&key) {
        entry.push(key);
    }
}

/// Removes an agent's subscription. Called from the kill handler so
/// blackboard writes to dead agents' depended-on keys don't try to wake
/// a registry slot that has been dropped.
pub async fn unregister_wake_subs(subs: &WakeSubs, agent_id: &str) {
    let mut w = subs.write().await;
    w.remove(agent_id);
}

/// Insert this agent's expected handoff_signal + spawn time. No-op when
/// the signal is empty (inline-only roles, planner, etc.). Called from
/// `run_spell` alongside `register_wake_subs`.
pub async fn register_exit_key(
    keys: &ExitKeys,
    agent_id: String,
    role: String,
    handoff_signal: String,
    spawned_at_ms: i64,
) {
    if handoff_signal.is_empty() {
        return;
    }
    let mut w = keys.write().await;
    w.insert(
        agent_id,
        ExitKey {
            role,
            handoff_signal,
            spawned_at_ms,
        },
    );
}

/// Remove this agent's exit-key registration. Called from the kill
/// handler before the registry slot is dropped — symmetric with
/// `unregister_wake_subs`.
pub async fn unregister_exit_key(keys: &ExitKeys, agent_id: &str) {
    let mut w = keys.write().await;
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

/// Pure (no IO/async) extracted for unit testing: pick the agents to auto-kill
/// after a handoff write. An agent is reaped ONLY when it produced its OWN
/// declared `handoff_signal` — i.e. the written `path` equals its
/// `handoff_signal` AND it is the `writer` (F13). Without the writer guard, two
/// agents that happen to declare the same `handoff_signal` would BOTH be killed
/// when either writes it, silently reaping a sibling that hasn't finished. An
/// unattributed write (`writer = None`, e.g. external editor / reconcile)
/// reaps no one. Returns `(agent_id, role)` pairs.
pub fn select_autokill_targets(
    exit_keys: &HashMap<String, ExitKey>,
    path: &str,
    writer: Option<&str>,
) -> Vec<(String, String)> {
    exit_keys
        .iter()
        .filter(|(aid, ek)| ek.handoff_signal == path && writer == Some(aid.as_str()))
        .map(|(aid, ek)| (aid.clone(), ek.role.clone()))
        .collect()
}

/// Diagnose the dominant silent-stall failure mode: a blackboard write that
/// IS some agent's declared `handoff_signal` yet matched ZERO `depends_on`
/// subscribers. That means a producer just shipped its completion key but
/// nothing is wired to react — almost always a key-string mismatch between the
/// producer's `handoff_signal` and a dependent's `depends_on` (a missing
/// `<workspace_id>` prefix, a trailing slash, or a typo). Wake matching is
/// exact-string, so the dependent then hangs forever with no other signal.
///
/// Returns `Some(keys_other_agents_are_waiting_on)` when this is an orphaned
/// handoff (the caller logs a warning with that context so the mismatch is
/// visible), or `None` otherwise. Pure (no IO/async) for unit testing.
/// `woke_anyone` = whether the fan-out already delivered to ≥1 subscriber.
pub fn orphaned_handoff_diagnosis(
    subs: &HashMap<String, Vec<String>>,
    handoff_signals: &[String],
    written_key: &str,
    woke_anyone: bool,
) -> Option<Vec<String>> {
    if woke_anyone {
        return None;
    }
    if !handoff_signals.iter().any(|h| h == written_key) {
        return None;
    }
    let mut waiting: Vec<String> = subs.values().flatten().cloned().collect();
    waiting.sort();
    waiting.dedup();
    Some(waiting)
}

/// Lag recovery (F12): the shared SwarmEvent broadcast can drop events under a
/// burst, and a dropped `BlackboardChanged` for a one-shot handoff key is lost
/// forever — there's no "next write" to catch up, so the dependent hangs and
/// no mailbox wake row is ever written either. On `Lagged` the coordinator
/// reconciles: given the subs snapshot and the set of `depends_on` keys we've
/// confirmed are already present on the blackboard (`satisfied`), return one
/// `(agent, key)` to re-wake per affected agent. Re-waking is benign (a
/// redundant recheck turn) and far better than a permanent stall. Pure +
/// deterministically ordered for unit testing.
pub fn agents_to_rewake(
    subs: &HashMap<String, Vec<String>>,
    satisfied: &std::collections::HashSet<String>,
) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = subs
        .iter()
        .filter_map(|(agent, keys)| {
            keys.iter()
                .find(|k| satisfied.contains(k.as_str()))
                .map(|k| (agent.clone(), k.clone()))
        })
        .collect();
    out.sort();
    out
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
    // Standard "key updated, please check" text. For TTL nudges where
    // the agent is the *producer* of the overdue key (not a subscriber),
    // the caller should use `inject_with_kick_text` directly so the
    // message reads as "you're blocking <waiter>" instead.
    let kick_text = format!("共享区 `{key}` 有更新，请查看");
    inject_with_kick_text(registry, agent_id, &kick_text, key).await
}

/// Body of the PTY-kick. Split out so the TTL nudge path can pass a
/// message that reflects "you're stuck producing X" instead of the
/// regular "X updated; please check" — the latter is misleading when
/// X is the recipient's OWN handoff signal.
///
/// `key_for_log` is the blackboard key this kick is about, used only
/// for the structured log fields so downstream tail / grep / dashboard
/// pipelines stay searchable by key name regardless of the message body.
pub async fn inject_with_kick_text(
    registry: &Registry,
    agent_id: &str,
    kick_text: &str,
    key_for_log: &str,
) -> Result<()> {
    let slot = registry
        .get(agent_id)
        .ok_or_else(|| anyhow!("no registry slot for `{agent_id}` — agent may have exited"))?;
    let input_tx = {
        let guard = slot.lock();
        guard.input_tx.clone()
    };

    // M6g (2026-05-24): removed the "skip-if-mid-stream" PTY quiet
    // gate that lived here previously. It was added in M6d-6 to avoid
    // polluting an in-flight turn during the TTL-nudge era. With TTL
    // gone (M6e), the only PTY injects are real BlackboardChanged
    // wakes — those signal "an agent you depend_on wrote a key", and
    // the agent receiving them should process them next turn regardless
    // of when they arrive. Worse: the quiet gate had a fatal edge case
    // (e2e #7, 2026-05-24): if PTY output just stopped within the
    // 2-second window because the AGENT JUST FINISHED a turn (not
    // because it's still streaming), the gate would skip the inject
    // — but then there's no new turn to trigger wake-check, so the
    // mailbox wake gets stranded indefinitely. The gate fundamentally
    // couldn't distinguish "still streaming" from "just stopped".
    // The simpler, correct behaviour is to always inject; claude/codex's
    // input buffer handles concurrent writes correctly during turn
    // boundaries (they were already designed for keyboard input racing
    // turn transitions).
    let _ = key_for_log; // kept in signature for log call sites if needed later

    // Why three separate writes with a delay before the final `\r`:
    //   The naive `format!("\x15…\r")` blob worked on Claude Code's TUI
    //   but failed on Codex CLI's Ratatui. Codex 0.130+ has bracketed-
    //   paste detection: a single chunk containing both text AND a
    //   terminating `\r` is treated as a paste with embedded newline
    //   — codex inserts the line into the input buffer but does NOT
    //   submit. The user (we) then had to send a SECOND `\r` to
    //   actually fire the agent. Confirmed in M6c-7 clean-e2e run on
    //   2026-05-23: BE codex sat at `>blackboard 'design.approved'
    //   updated; please check` for 16 minutes after the wake until a
    //   manual `\r` via websocat unstuck it.
    //
    //   Splitting the writes — body, sleep ~150ms, then `\r` alone —
    //   exits paste mode between the two, so the `\r` is seen as a
    //   typed keystroke and submits the buffer. This mirrors what
    //   `rest.rs` spell-bootstrap inject already does (see the
    //   "PTY paste send" path), which is why bootstrap injection has
    //   always worked for codex but wake injection did not.
    let body = format!("\x15{kick_text}");
    input_tx
        .send(Bytes::from(body))
        .await
        .map_err(|e| anyhow!("PTY input_tx send (body) failed: {e}"))?;
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    input_tx
        .send(Bytes::from_static(b"\r"))
        .await
        .map_err(|e| anyhow!("PTY input_tx send (submit \\r) failed: {e}"))
}

/// M6e: operator-triggered manual wake. Same delivery shape as the
/// BlackboardChanged-driven wake (mailbox `kind=wake` + PTY kick), but
/// the kick text says "manual wake from operator" instead of pretending
/// there's a key update. Used by the UI's ⚡ button when an operator
/// believes an agent has missed a wake or is stuck.
///
/// Mailbox is the source of truth; if it fails we bail (sending a PTY
/// kick with no context would be misleading). The PTY kick itself is
/// best-effort — failure usually means the agent has exited, in which
/// case the mailbox entry is also moot but we've already returned Ok
/// (caller wanted a fire-and-forget signal, not a delivery guarantee).
pub async fn deliver_manual_wake(
    swarm: &Swarm,
    registry: &Registry,
    target: &str,
) -> Result<()> {
    let now = now_ms();
    let msg = NewMessage {
        from_agent: "system".into(),
        to_agent: target.into(),
        kind: "wake".into(),
        body: "操作员手动唤醒——请重新检查共享区里的输入，确认就绪后继续".into(),
        sent_at: now,
        in_reply_to: None,
    };
    swarm
        .send_message(msg)
        .await
        .map_err(|e| anyhow!("manual wake mailbox send failed: {e}"))?;
    // Best-effort PTY kick. Use the existing inject_with_kick_text so
    // the M6d-6 quiet-gate protects us from polluting an in-flight
    // turn; `key_for_log` is "manual-wake" so log/grep stays clean.
    if let Err(err) = inject_with_kick_text(
        registry,
        target,
        "操作员手动唤醒——请重新检查共享区后继续",
        "manual-wake",
    )
    .await
    {
        tracing::debug!(
            ?err,
            target,
            "manual wake PTY inject failed (mailbox delivered, will catch on next Stop)"
        );
    }
    tracing::info!(target, "manual wake delivered");
    Ok(())
}

pub struct WakeCoordinator {
    swarm: Arc<Swarm>,
    registry: Registry,
    subs: WakeSubs,
    exit_keys: ExitKeys,
    /// Needed for the post-handoff auto-kill path: when a worker writes
    /// its handoff_signal we mark its DB row as killed too, otherwise
    /// the agent stays "live" forever in `list_agents`.
    store: Arc<flockmux_storage::Store>,
}

impl WakeCoordinator {
    /// Spawns the wake task and returns its JoinHandle. The handle is
    /// dropped immediately by `main.rs` since the task runs for the
    /// lifetime of the process (it exits only when the broadcast channel
    /// closes, which happens at server shutdown).
    pub fn spawn(
        swarm: Arc<Swarm>,
        registry: Registry,
        subs: WakeSubs,
        exit_keys: ExitKeys,
        store: Arc<flockmux_storage::Store>,
    ) -> JoinHandle<()> {
        let me = Self {
            swarm,
            registry,
            subs,
            exit_keys,
            store,
        };
        tokio::spawn(me.run())
    }

    /// Run loop: subscribe to swarm broadcast and react to two event
    /// kinds — BlackboardChanged (wake subscribers of the written key)
    /// and AgentState::Exited (M6c-5 .error fallback for producer death).
    ///
    /// Note (M6e, 2026-05-23): the earlier M6d-5/5b/5c TTL scanner was
    /// removed after 5 e2e runs + research across 4 sibling projects
    /// (golutra, swarm-ide, openclaw, hermes-agent) showed the design
    /// was structurally wrong: "PTY quiet for N min" is a transient
    /// observation, not a stable property (Chandy-Lamport 1985), and
    /// nudging a producer mid-stream demonstrably caused LLM agents to
    /// fabricate handoff signals (MAST FM-3.1 "Premature Termination",
    /// arXiv 2503.13657). The blackboard event + M6c-5 .error fallback
    /// together cover every observed failure mode; truly stuck agents
    /// are surfaced through the UI's manual ⚡ wake button (operator
    /// hatch, modeled after swarm-ide's stop-all and openclaw's
    /// `doctor --fix`).
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
                    // Build the set of keys this write should fan out to.
                    // For a `<X>.error` or `<X>.failed` write, base key
                    // subscribers (agents that depend_on `<X>`) also get
                    // woken — that's the M6c step 5 "producer died, give
                    // up" path. Their role prompts already check for
                    // .error/.failed and branch accordingly.
                    let mut keys_to_fan: Vec<String> = vec![path.clone()];
                    keys_to_fan.extend(base_key_aliases(&path));

                    // Snapshot subs once; iterate fan-out keys against it.
                    let map = self.subs.read().await.clone();
                    // De-dup targets across fan-out keys so an agent
                    // doesn't get N redundant kicks if it happens to
                    // subscribe to both `<X>` and `<X>.error`.
                    let mut delivered: std::collections::HashSet<String> = Default::default();
                    for key in &keys_to_fan {
                        for t in select_targets(&map, key, writer.as_deref()) {
                            if delivered.insert(t.clone()) {
                                self.deliver_wake(&t, &path).await;
                            }
                        }
                    }

                    // Diagnose the dominant silent stall (F3): nobody was woken
                    // by this write. If the key IS some agent's declared
                    // handoff_signal, a producer just "finished" but no
                    // dependent is wired to it — a depends_on/handoff_signal key
                    // mismatch that would otherwise hang the dependent with zero
                    // diagnostics. Only read exit_keys on this rare zero-wake
                    // path, so the common (matched) case stays cheap.
                    if delivered.is_empty() {
                        let handoffs: Vec<String> = {
                            let ek = self.exit_keys.read().await;
                            ek.values().map(|e| e.handoff_signal.clone()).collect()
                        };
                        if let Some(waiting) =
                            orphaned_handoff_diagnosis(&map, &handoffs, &path, false)
                        {
                            tracing::warn!(
                                handoff = %path,
                                waiting_on = ?waiting,
                                "handoff signal written but NO agent depends_on it — likely a \
                                 depends_on/handoff_signal key mismatch; the dependent will hang \
                                 forever. Verify the keys match EXACTLY (workspace_id prefix, \
                                 trailing slash, spelling)."
                            );
                        }
                    }

                    // Post-handoff auto-kill: if this blackboard write
                    // matches some agent's registered handoff_signal,
                    // that worker has done its job. claude/codex CLIs
                    // don't self-exit after STOPping a reply — their
                    // PTY sits idle forever, leaking process + per-agent
                    // MCP config + a phantom "alive" row in the UI. We
                    // tear that down on a small grace delay (let the
                    // worker finish printing its final scroll, let the
                    // recording flush) so the agent list and DAG return
                    // to ground truth without operator action.
                    self.maybe_auto_kill_on_handoff(&path, writer.as_deref()).await;
                }
                Ok(SwarmEvent::AgentState { agent_id, state }) => {
                    if matches!(state, flockmux_protocol::ws_swarm::AgentState::Exited) {
                        self.handle_agent_exit(&agent_id).await;
                    }
                }
                Ok(_) => {} // ignore the rest (message, message_read)
                Err(RecvError::Lagged(n)) => {
                    // Broadcast overflow: the coordinator fell behind a burst
                    // and the ring dropped events. A dropped BlackboardChanged
                    // for a one-shot handoff key has NO "next write" to catch
                    // up (and no mailbox row was written), so the dependent
                    // would hang forever (F12). Recover by reconciling every
                    // depends_on against the blackboard and re-waking anything
                    // already satisfied.
                    tracing::warn!(
                        lagged = n,
                        "wake coordinator broadcast lagged; reconciling depends_on against the blackboard"
                    );
                    self.reconcile_after_lag().await;
                }
                Err(RecvError::Closed) => {
                    tracing::info!("wake coordinator: broadcast closed, exiting");
                    break;
                }
            }
        }
    }

    /// Auto-kill a worker that just produced its `handoff_signal`.
    /// Reverse-scan `exit_keys` for any agent whose `handoff_signal`
    /// matches `path`. claude/codex CLIs don't STOP their PTY on their
    /// own — once the reply is printed they enter an idle prompt
    /// waiting for next input. Without this, every worker the user
    /// ever spawned stays "alive" in the registry / agent list / DAG
    /// canvas, eating per-agent MCP config files + a phantom PTY.
    ///
    /// We delay the kill by `AUTO_KILL_GRACE_MS` so:
    ///   - the worker can finish printing whatever it's still streaming
    ///   - the asciinema recording's last frames get flushed
    ///   - if the LLM wrote the signal too eagerly mid-thought and
    ///     immediately writes something else (rare), we don't yank it
    ///
    /// Race safety: we re-check the registry on the delayed tick.
    /// The agent may have been manually killed in the meantime, or its
    /// exit_keys entry may have been claimed by `handle_agent_exit`.
    async fn maybe_auto_kill_on_handoff(&self, path: &str, writer: Option<&str>) {
        // Capture (agent_id, role_label) pairs so the farewell message
        // can sign off with the worker's role instead of an opaque UUID.
        // Only the agent that WROTE its own handoff_signal is reaped (F13):
        // a sibling sharing the same signal string must not be killed when
        // this one finishes. See `select_autokill_targets`.
        let targets: Vec<(String, String)> = {
            let map = self.exit_keys.read().await;
            select_autokill_targets(&map, path, writer)
        };
        if targets.is_empty() {
            return;
        }
        for (agent_id, role) in targets {
            let registry = self.registry.clone();
            let swarm = self.swarm.clone();
            let subs = self.subs.clone();
            let exit_keys = self.exit_keys.clone();
            let store = self.store.clone();
            let sig = path.to_string();
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(AUTO_KILL_GRACE_MS))
                    .await;
                // Re-check: agent might already be gone.
                let slot = match registry.remove(&agent_id) {
                    Some(s) => s,
                    None => return,
                };
                // FAREWELL MESSAGE: before we tear down the worker, post a
                // short note to the user in the workspace chat. Without
                // this, the worker silently disappears from the member
                // list and users have no idea who to talk to next — they
                // think the project has stopped responding. Magentic-One's
                // PM-style design assumes the orchestrator is "obviously"
                // the one to follow up with, but new users have no such
                // intuition. One sentence in the dying worker's voice
                // fixes the entire confusion class.
                let signal_label = sig
                    .rsplit_once('/')
                    .map(|(_, last)| last)
                    .unwrap_or(&sig);
                let body = format!(
                    "✓ 已交付 {signal_label} 并解散。继续改 / 加新需求,直接跟 orchestrator 说就行,我俩看同一份 ledger,它清楚我刚才干了啥。",
                );
                let farewell = NewMessage {
                    from_agent: agent_id.clone(),
                    to_agent: "user".into(),
                    kind: "farewell".into(),
                    body,
                    sent_at: now_ms_local(),
                    in_reply_to: None,
                };
                if let Err(e) = swarm.send_message(farewell).await {
                    tracing::warn!(?e, agent = %agent_id, "auto-kill: farewell send failed");
                }
                {
                    let s = slot.lock();
                    s.bridge.kill();
                }
                swarm.unregister_agent(&agent_id);
                unregister_wake_subs(&subs, &agent_id).await;
                unregister_exit_key(&exit_keys, &agent_id).await;
                if let Err(e) = store
                    .record_agent_kill(agent_id.clone(), now_ms_local())
                    .await
                {
                    tracing::warn!(?e, agent = %agent_id, "auto-kill: record_agent_kill failed");
                }
                swarm.publish_event(SwarmEvent::AgentState {
                    agent_id: agent_id.clone(),
                    state: flockmux_protocol::ws_swarm::AgentState::Exited,
                });
                tracing::info!(
                    agent = %agent_id,
                    role = %role,
                    handoff = %sig,
                    "auto-killed worker after handoff_signal"
                );
            });
        }
    }

    /// Producer-died fallback. When an agent transitions to Exited we
    /// look up the `handoff_signal` it was supposed to produce; if that
    /// key isn't on the blackboard yet, write `<signal>.error` so
    /// downstream dependents (test agent waiting on `frontend.done`,
    /// etc.) can detect the upstream failure and branch instead of
    /// hanging forever.
    ///
    /// Best-effort: every failure path is logged and swallowed. We
    /// always clean up the exit_keys entry so a duplicate Exited event
    /// (kill-then-natural-exit race) doesn't try to write again.
    async fn handle_agent_exit(&self, agent_id: &str) {
        let ek = {
            let map = self.exit_keys.read().await;
            match map.get(agent_id) {
                Some(k) if !k.handoff_signal.is_empty() => k.clone(),
                _ => return, // no registered handoff or already cleaned up
            }
        };
        unregister_exit_key(&self.exit_keys, agent_id).await;
        let signal = ek.handoff_signal.clone();

        // Did THIS run's agent write the signal? Query the
        // blackboard_ops history for the path; if the latest write's
        // `at` is newer than our spawn time, we're done — that's our
        // agent's commit. Older `at` means the row is left over from a
        // previous run on the same workspace, and the current agent
        // crashed before producing its own; that's the case we owe an
        // `.error` for.
        let store = self.swarm.store();
        let fresh = match store
            .list_blackboard_ops(Some(signal.clone()))
            .await
        {
            Ok(rows) => rows
                .first()
                .map(|r| r.at >= ek.spawned_at_ms)
                .unwrap_or(false),
            Err(err) => {
                tracing::warn!(
                    ?err,
                    agent_id,
                    signal,
                    "list_blackboard_ops failed; assuming agent didn't write the signal"
                );
                false
            }
        };
        if fresh {
            tracing::debug!(
                agent_id,
                signal,
                "agent exited with handoff signal already written; no .error needed"
            );
            return;
        }

        // Naming (P0-A): the failure key is the producer's MINTED handoff key
        // + `.error` (e.g. `ws/dir/frontend.done.error`), identical to what a
        // worker is told to write on voluntary failure (see
        // `build_worker_prompt`). One convention for both crash and graceful
        // abort, and `base_key_aliases` fans `<signal>.error` → `<signal>` so
        // even a passive consumer waiting on the success key is woken. `signal`
        // is already the fully-scoped minted key, so no bare `<role>` drift.
        let error_key = format!("{signal}.error");
        let body = serde_json::json!({
            "agent_id": agent_id,
            "role": ek.role,
            "signal": signal,
            "reason": "agent exited without writing its handoff signal",
            "at": now_ms(),
        });
        let body_str = serde_json::to_string_pretty(&body).unwrap_or_else(|_| body.to_string());
        match self.swarm.write_blackboard(Some("system".into()), &error_key, &body_str).await {
            Ok(_) => {
                tracing::info!(
                    agent_id,
                    signal,
                    error_key,
                    "agent exited without producing signal; wrote .error fallback"
                );
            }
            Err(err) => {
                tracing::warn!(
                    ?err,
                    agent_id,
                    error_key,
                    "failed to write .error fallback"
                );
                // Don't bail — still try to wake subscribers below so
                // they at least get a mailbox note describing the
                // upstream failure, even if the .error file is missing.
            }
        }

        // Critical: explicitly wake subscribers of the ORIGINAL signal,
        // not of the .error key. depends_on lists keys like
        // "frontend.done", not "frontend.error" — without this direct
        // dispatch, the .error write alone would not unblock anyone
        // (the BlackboardChanged broadcast for `<role>.error` matches
        // nobody's subscription).
        let writer = Some(agent_id.to_string());
        let targets = {
            let map = self.subs.read().await;
            select_targets(&map, &signal, writer.as_deref())
        };
        for target in targets {
            self.deliver_wake(&target, &error_key).await;
        }
    }

    /// Is `key` — or its `.error` / `.failed` failure alias — present on the
    /// blackboard right now? Failure aliases mirror the normal dispatch, which
    /// also wakes `depends_on = K` subscribers when `K.error` is written.
    async fn key_or_alias_written(&self, key: &str) -> bool {
        for probe in [
            key.to_string(),
            format!("{key}.error"),
            format!("{key}.failed"),
        ] {
            if matches!(self.swarm.read_blackboard(&probe).await, Ok(Some(_))) {
                return true;
            }
        }
        false
    }

    /// Recover from a broadcast `Lagged` (F12). The coordinator can't know
    /// which events were dropped, so reconcile against ground truth: for every
    /// registered `depends_on` key that's already satisfied on the blackboard,
    /// re-wake the waiting agent. A one-shot handoff wake that was dropped is
    /// otherwise lost forever (no next write, no mailbox row), hanging the
    /// dependent. Re-waking an already-active agent just costs a recheck turn.
    async fn reconcile_after_lag(&self) {
        let map = self.subs.read().await.clone();
        let mut satisfied: std::collections::HashSet<String> = Default::default();
        let mut checked: std::collections::HashSet<String> = Default::default();
        for keys in map.values() {
            for key in keys {
                if !checked.insert(key.clone()) {
                    continue;
                }
                if self.key_or_alias_written(key).await {
                    satisfied.insert(key.clone());
                }
            }
        }
        let rewake = agents_to_rewake(&map, &satisfied);
        if !rewake.is_empty() {
            tracing::warn!(
                count = rewake.len(),
                "wake coordinator: re-waking dependents after broadcast lag \
                 (their awaited key was already on the blackboard)"
            );
        }
        for (agent, key) in rewake {
            self.deliver_wake(&agent, &key).await;
        }
    }

    async fn deliver_wake(&self, target: &str, key: &str) {
        // M-pause: if the operator paused this agent, swallow auto-wakes.
        // The mailbox is intentionally NOT written either — paused means
        // "leave me alone, don't accumulate noise that I'll have to
        // hand-trim on resume." On resume the operator's deliver_manual_wake
        // call will write a single fresh wake entry pointing the agent at
        // the blackboard, which is more useful than N stale per-key
        // entries from this paused window. Manual wakes bypass this gate
        // (they go through deliver_manual_wake, not deliver_wake).
        if let Some(slot) = self.registry.get(target) {
            if slot.lock().paused.load(std::sync::atomic::Ordering::Relaxed) {
                tracing::debug!(target, key, "wake skipped: agent is paused");
                return;
            }
        }
        let now = now_ms();
        // 1) Mailbox: source of truth. If this fails the wake mechanism
        //    is broken for this delivery — we'd be hoping the agent's
        //    own polling catches the key (back to M6a behaviour). Log
        //    loudly but don't panic.
        let msg = NewMessage {
            from_agent: "system".into(),
            to_agent: target.into(),
            kind: "wake".into(),
            body: format!("共享区 `{key}` 有更新，请查看"),
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

    fn build_exit_keys(entries: &[(&str, &str, &str)]) -> HashMap<String, ExitKey> {
        entries
            .iter()
            .map(|(aid, role, sig)| {
                (
                    aid.to_string(),
                    ExitKey {
                        role: role.to_string(),
                        handoff_signal: sig.to_string(),
                        spawned_at_ms: 0,
                    },
                )
            })
            .collect()
    }

    #[test]
    fn autokill_reaps_only_the_writer_not_siblings_sharing_signal() {
        // F13: worker-a and worker-b BOTH declare handoff "demo/out.done".
        // worker-a writes it → only worker-a is reaped; worker-b (possibly
        // still working) is left alone.
        let ek = build_exit_keys(&[
            ("worker-a", "writer", "demo/out.done"),
            ("worker-b", "writer", "demo/out.done"),
        ]);
        let t = select_autokill_targets(&ek, "demo/out.done", Some("worker-a"));
        assert_eq!(t, vec![("worker-a".to_string(), "writer".to_string())]);
    }

    #[test]
    fn autokill_unattributed_write_reaps_nobody() {
        // writer = None (external editor / reconcile) — never auto-kill.
        let ek = build_exit_keys(&[("worker-a", "writer", "demo/out.done")]);
        assert!(select_autokill_targets(&ek, "demo/out.done", None).is_empty());
    }

    #[test]
    fn autokill_writer_with_unrelated_path_reaps_nobody() {
        // worker-a wrote some OTHER key, not its own handoff_signal → not done.
        let ek = build_exit_keys(&[("worker-a", "writer", "demo/out.done")]);
        assert!(select_autokill_targets(&ek, "demo/progress.md", Some("worker-a")).is_empty());
    }

    #[test]
    fn autokill_empty_map_returns_empty() {
        let ek: HashMap<String, ExitKey> = HashMap::new();
        assert!(select_autokill_targets(&ek, "x", Some("a")).is_empty());
    }

    #[test]
    fn orphaned_handoff_warns_on_depends_on_mismatch() {
        // Producer's handoff is "ws-42/api.done"; the dependent drifted to
        // "api.done" (dropped the workspace prefix) → nobody matches → orphan.
        // Returns the keys agents ARE waiting on, for the warning context.
        let subs = build_subs(&[("be", &["api.done"])]);
        let handoffs = vec!["ws-42/api.done".to_string()];
        let got = orphaned_handoff_diagnosis(&subs, &handoffs, "ws-42/api.done", false);
        assert_eq!(got, Some(vec!["api.done".to_string()]));
    }

    #[test]
    fn orphaned_handoff_silent_when_a_subscriber_matched() {
        // woke_anyone = true (the fan-out delivered) → never warn.
        let subs = build_subs(&[("be", &["ws/api.done"])]);
        let handoffs = vec!["ws/api.done".to_string()];
        assert_eq!(
            orphaned_handoff_diagnosis(&subs, &handoffs, "ws/api.done", true),
            None
        );
    }

    #[test]
    fn orphaned_handoff_silent_for_non_handoff_writes() {
        // A routine scratch/ledger write that isn't any agent's handoff_signal
        // must NOT warn, even with zero subscribers — keeps the signal noise-free.
        let subs = build_subs(&[("be", &["ws/api.done"])]);
        let handoffs = vec!["ws/api.done".to_string()];
        assert_eq!(
            orphaned_handoff_diagnosis(&subs, &handoffs, "ws/progress.ledger.md", false),
            None
        );
    }

    #[test]
    fn agents_to_rewake_picks_only_satisfied_dependents() {
        // be + qa depend on a satisfied key → re-wake; fe's key is not
        // satisfied → skip. Output is deterministically sorted.
        let subs = build_subs(&[
            ("be", &["ws/api.done"]),
            ("fe", &["ws/ui.done"]),
            ("qa", &["ws/api.done", "ws/ui.done"]),
        ]);
        let mut satisfied = std::collections::HashSet::new();
        satisfied.insert("ws/api.done".to_string());
        let got = agents_to_rewake(&subs, &satisfied);
        assert_eq!(
            got,
            vec![
                ("be".to_string(), "ws/api.done".to_string()),
                ("qa".to_string(), "ws/api.done".to_string()),
            ]
        );
    }

    #[test]
    fn agents_to_rewake_empty_when_nothing_satisfied() {
        let subs = build_subs(&[("be", &["ws/api.done"])]);
        let satisfied = std::collections::HashSet::new();
        assert!(agents_to_rewake(&subs, &satisfied).is_empty());
    }

    #[test]
    fn select_targets_no_match_returns_empty() {
        let m = build_subs(&[("a", &["foo.done"])]);
        assert!(select_targets(&m, "bar.done", None).is_empty());
    }

    // ── P0-A: minted keys match exactly; drift no longer silently no-wakes ──

    #[test]
    fn minted_key_matches_exactly_drift_does_not() {
        // Consumer subscribes to the canonical minted key.
        let minted = "ws_ab12/dark-mode/frontend.done";
        let m = build_subs(&[("consumer", &[minted])]);
        // The producer's minted write wakes it.
        assert_eq!(select_targets(&m, minted, Some("frontend")), vec!["consumer"]);
        // A drifted key (missing the workspace/thread prefix — the exact F3
        // failure) matches NOTHING. Under the old free-string scheme this is
        // how a dependent hung forever; under P0-A both sides are server-minted
        // so this drift can't be produced, and if it somehow were, it's inert.
        assert!(select_targets(&m, "frontend.done", None).is_empty());
    }

    #[test]
    fn minted_error_key_fans_out_to_the_success_key() {
        // A worker (or the death fallback) writing `<minted>.error` must wake
        // the consumers that wait on `<minted>` (the success key), via the
        // base-key alias fan-out — that's the fail-LOUD path.
        let minted = "ws_ab12/dark-mode/frontend.done";
        let error_key = format!("{minted}.error");
        assert_eq!(base_key_aliases(&error_key), vec![minted.to_string()]);

        let m = build_subs(&[("consumer", &[minted])]);
        // Simulate the BlackboardChanged fan: literal key + its base aliases.
        let mut woke: Vec<String> = Vec::new();
        let mut keys = vec![error_key.clone()];
        keys.extend(base_key_aliases(&error_key));
        for k in &keys {
            woke.extend(select_targets(&m, k, Some("frontend")));
        }
        assert_eq!(woke, vec!["consumer"], "the .done waiter is woken on .error");
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

    // ── M6c step 5: exit_keys + .error/.failed fan-out ──────────────────

    #[tokio::test]
    async fn exit_key_register_and_unregister() {
        let keys: ExitKeys = Arc::new(RwLock::new(HashMap::new()));
        register_exit_key(
            &keys,
            "a".into(),
            "frontend".into(),
            "frontend.done".into(),
            1_700_000_000_000,
        )
        .await;
        let stored = keys.read().await.get("a").cloned();
        assert_eq!(stored.as_ref().map(|k| k.role.as_str()), Some("frontend"));
        assert_eq!(stored.as_ref().map(|k| k.handoff_signal.as_str()), Some("frontend.done"));
        assert_eq!(stored.map(|k| k.spawned_at_ms), Some(1_700_000_000_000));
        unregister_exit_key(&keys, "a").await;
        assert!(keys.read().await.get("a").is_none());
    }

    #[tokio::test]
    async fn exit_key_register_ignores_empty_signal() {
        // planner has no handoff_signal; we shouldn't pollute the map.
        let keys: ExitKeys = Arc::new(RwLock::new(HashMap::new()));
        register_exit_key(
            &keys,
            "planner-a".into(),
            "planner".into(),
            "".into(),
            1_700_000_000_000,
        )
        .await;
        assert!(
            keys.read().await.get("planner-a").is_none(),
            "empty handoff_signal shouldn't pollute exit_keys"
        );
    }

    #[test]
    fn base_key_aliases_strips_error_suffix() {
        assert_eq!(base_key_aliases("frontend.done.error"), vec!["frontend.done"]);
    }

    #[test]
    fn base_key_aliases_strips_failed_suffix() {
        assert_eq!(base_key_aliases("backend.done.failed"), vec!["backend.done"]);
    }

    #[test]
    fn base_key_aliases_passes_through_plain_key() {
        // Regular key (no suffix) → no fan-out, the wake loop wakes only
        // the literal-key subscribers as before.
        assert!(base_key_aliases("frontend.done").is_empty());
        assert!(base_key_aliases("api.spec").is_empty());
    }

    #[test]
    fn base_key_aliases_handles_bare_suffix() {
        // ".error" with empty base — definitely not a real handoff key
        // anyone subscribed to. Empty Vec → no fan-out.
        assert!(base_key_aliases(".error").is_empty());
        assert!(base_key_aliases(".failed").is_empty());
    }

    #[test]
    fn fanout_wakes_base_key_subscribers_on_error() {
        // dependent subscribes to "frontend.done"; .error write should
        // reach them via base_key_aliases → select_targets("frontend.done").
        let map = build_subs(&[("test-a", &["frontend.done"])]);
        // Direct hit on the .error key — no subscribers.
        assert!(select_targets(&map, "frontend.done.error", None).is_empty());
        // But the aliased base key picks up the dependent.
        let aliases = base_key_aliases("frontend.done.error");
        assert_eq!(aliases, vec!["frontend.done"]);
        let woken = select_targets(&map, &aliases[0], None);
        assert_eq!(woken, vec!["test-a".to_string()]);
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

    // M6d-6 PTY activity-based inject gate tests were removed in M6g
    // (2026-05-24). The gate fundamentally couldn't distinguish
    // "agent still streaming" from "agent just finished a turn", and
    // the latter case stranded wakes indefinitely (e2e #7). The gate
    // existed to protect against TTL-nudge pollution during M6d-5;
    // with TTL removed (M6e), the gate's protection has no use case
    // left and its edge case caused real bugs. See M6g commit for
    // details.
}
