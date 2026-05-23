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

/// M6d-5 (rev. M6d-5b): rate-limit table for TTL nudges. Keyed by
/// `(waiter_agent_id, key)` → unix-ms when we LAST fired a nudge for
/// this pair. Initially empty; entries only appear after a nudge has
/// actually been sent.
///
/// The earlier M6d-5 design stored "subscription registration time"
/// here and aged it out at a fixed threshold. That gave false
/// positives in chained spells: e.g. `fullstack-feature-strict`'s
/// critic subscribes to `fixer.done` at spell launch, but the
/// strict pipeline (FE/BE work → critic round 1 → fixer round 1)
/// can easily run past 5 minutes before fixer even starts — so the
/// 5-minute wall-clock TTL would fire while fixer was perfectly
/// healthy, just waiting its turn.
///
/// The new design measures *producer* quiet time instead: the
/// scanner reads each producer's `PtyStream::last_append_ms` and
/// only nudges when the producer itself has been silent for ≥ the
/// threshold. This map exists solely to prevent re-nudging the same
/// (waiter, key) pair every tick once it's eligible — it's a
/// debounce, not a state machine.
///
/// Entries are dropped:
///   - lazily when the relevant key actually lands on the blackboard
///     (BlackboardChanged handler prunes matching rows)
///   - eagerly when the waiter is killed
///   - on the next eligibility check that's older than the threshold
///     (`should_nudge_again` returns true again — same as a fresh entry)
pub type WakeNudged = Arc<RwLock<HashMap<(String, String), i64>>>;

/// How long a subscription can sit pending before TTL fires a nudge.
/// 5 minutes was picked as "longer than a healthy agent's full turn,
/// shorter than enough time for the operator to walk away and forget
/// the spell is running." Increase if you have spells that legitimately
/// take longer between handoffs; decrease if you want louder feedback.
const TTL_THRESHOLD_MS: i64 = 5 * 60 * 1000;

/// Periodic scan interval. Cheaper than the threshold (no need to
/// sample at TTL precision) but small enough that the alert lands
/// within a minute of the deadline.
const TTL_TICK_SECS: u64 = 60;

/// M6d-6: how recent the last PTY output chunk must be for the agent
/// to be treated as "still streaming". 2 seconds is a comfortable
/// upper bound on the gap between consecutive chunks of a generating
/// LLM turn: claude/codex emit in tight bursts during text streaming,
/// and during tool-call sequences the gap is small too. Anything
/// longer is almost certainly an idle prompt, where injecting is the
/// right move. Tune up if false-positives appear (rare); tune down if
/// false-negatives (injection mid-stream) appear.
const PTY_QUIET_MS_FOR_INJECT: i64 = 2_000;

/// M6d-6: pure helper extracted for testing. Returns true when the
/// PTY has been quiet long enough that a wake-inject is safe.
/// `last_append_ms == 0` means the stream has never seen output —
/// safe to inject (no in-flight turn exists yet).
pub fn pty_quiet_enough_to_inject(last_append_ms: i64, now_ms: i64) -> bool {
    if last_append_ms == 0 {
        return true;
    }
    now_ms.saturating_sub(last_append_ms) >= PTY_QUIET_MS_FOR_INJECT
}

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

/// Removes an agent's subscription. Called from the kill handler so
/// blackboard writes to dead agents' depended-on keys don't try to wake
/// a registry slot that has been dropped.
pub async fn unregister_wake_subs(subs: &WakeSubs, agent_id: &str) {
    let mut w = subs.write().await;
    w.remove(agent_id);
}

/// M6d-5b: drop every nudge-rate-limit row for a single waiter.
/// Called when the waiter is killed: any future nudges that would
/// have referenced this waiter would point at a dead agent anyway,
/// so the rate-limit row is moot.
pub async fn unregister_wake_nudged(nudged: &WakeNudged, agent_id: &str) {
    let mut w = nudged.write().await;
    w.retain(|(aid, _), _| aid != agent_id);
}

/// M6d-5b: drop every nudge-rate-limit row for a key the moment it
/// lands on the blackboard. The waiter is unblocked, so there's no
/// reason to keep a row that could prevent a future *unrelated*
/// nudge (if the same role somehow re-subscribed). Matches across
/// all waiters of the key — they're all unblocked by the same write.
pub async fn prune_wake_nudged_by_key(nudged: &WakeNudged, key: &str) {
    let mut w = nudged.write().await;
    w.retain(|(_, k), _| k != key);
}

/// M6d-5c: pure decision helper for the TTL scanner. Returns the
/// list of (waiter, key, producer_aid, producer_role) triples that
/// should fire a nudge right now.
///
/// Selection rules — ALL must hold for a target to fire:
///   1. **subscription exists** — waiter is subscribed to `key`
///      (key appears in `subs[waiter]`).
///   2. **in-spell producer** — some agent declares `key` as its
///      `handoff_signal`. External-input keys (human-gate signals
///      like `design.approved`) are skipped.
///   3. **not self-loop** — producer ≠ waiter (defence-in-depth).
///   4. **key still missing** — `key` is NOT currently on the
///      blackboard. Without this check the v1 design re-nudged
///      finished producers every time they went quiet after writing
///      their handoff — observed in 2026-05-23 e2e #2 where BE was
///      told "you owe critic backend.done" 5 minutes after BE had
///      already written it, and BE codex then *re-wrote* backend.done
///      to placate the message, cascading into spurious wakes.
///   5. **producer's own deps satisfied** — if producer subscribes
///      to anything (`subs.get(producer) -> Some(deps)`), every key
///      in `deps` must be on the blackboard. A producer that's
///      idle BECAUSE its own inputs aren't ready is not stuck — it
///      shouldn't be acting yet. Observed in 2026-05-23 e2e #2 where
///      fixer (deps=[review.completed]) was nudged before critic
///      finished round 1, and fixer fabricated a fake `fixer.done`
///      to placate the message.
///   6. **producer PTY actually started** — `last_append_ms > 0`.
///      Zero means the stream has never seen output; producer is
///      still booting, not stuck.
///   7. **producer PTY quiet ≥ threshold** — `now - last_append ≥
///      threshold_ms`. The original M6d-5b condition.
///   8. **rate limit** — either we've never nudged this (waiter,
///      key) pair, OR the last nudge was ≥ `threshold_ms` ago.
///
/// All inputs are owned snapshots so this can be unit-tested without
/// touching the registry or the runtime. `subs` doubles as the
/// producer-deps source — wake_subs is keyed by every live agent
/// that declared any depends_on, including agents that are themselves
/// producers of other keys.
pub fn select_ttl_targets(
    subs: &HashMap<String, Vec<String>>,
    exit_keys: &HashMap<String, ExitKey>,
    producer_last_append: &HashMap<String, i64>,
    wake_nudged: &HashMap<(String, String), i64>,
    keys_on_blackboard: &std::collections::HashSet<String>,
    now_ms: i64,
    threshold_ms: i64,
) -> Vec<TtlNudgeTarget> {
    // Invert exit_keys: handoff_signal → (producer_aid, producer_role).
    // Empty handoff signals are filtered out — those roles (planner,
    // inline-only critics) never produce a blackboard key in the
    // first place. If two agents somehow claim the same signal, the
    // last one wins; that's an invariant violation worth panicking
    // on in tests, but in production we just pick one.
    let mut key_to_producer: HashMap<&str, (&str, &str)> = HashMap::new();
    for (aid, ek) in exit_keys {
        if !ek.handoff_signal.is_empty() {
            key_to_producer.insert(ek.handoff_signal.as_str(), (aid.as_str(), ek.role.as_str()));
        }
    }

    let mut targets = Vec::new();
    for (waiter, keys) in subs {
        for key in keys {
            // (4) Key already produced → consumer is no longer waiting.
            // Cheap snapshot lookup — no need to descend into
            // producer state at all.
            if keys_on_blackboard.contains(key) {
                continue;
            }
            let Some(&(producer_aid, producer_role)) = key_to_producer.get(key.as_str()) else {
                continue; // (2) external input
            };
            if producer_aid == waiter.as_str() {
                continue; // (3) self-loop guard
            }
            // (5) Producer's own deps satisfied? wake_subs is the
            // resolved-deps source: every live agent that subscribes
            // to anything appears here. Absence = no deps = trivially
            // satisfied (e.g. backend with depends_on=[]).
            let producer_deps_ok = match subs.get(producer_aid) {
                None => true,
                Some(deps) => deps.iter().all(|k| keys_on_blackboard.contains(k)),
            };
            if !producer_deps_ok {
                continue;
            }
            let last_append = producer_last_append.get(producer_aid).copied().unwrap_or(0);
            if last_append == 0 {
                continue; // (6) producer hasn't started yet
            }
            if now_ms.saturating_sub(last_append) < threshold_ms {
                continue; // (7) producer is actively streaming
            }
            // (8) Rate limit
            let key_pair = (waiter.clone(), key.clone());
            if let Some(&last_nudge) = wake_nudged.get(&key_pair) {
                if now_ms.saturating_sub(last_nudge) < threshold_ms {
                    continue;
                }
            }
            targets.push(TtlNudgeTarget {
                waiter: waiter.clone(),
                key: key.clone(),
                producer_aid: producer_aid.to_string(),
                producer_role: producer_role.to_string(),
                producer_last_append_ms: last_append,
            });
        }
    }
    targets
}

/// Output of `select_ttl_targets`. Holds enough to build the nudge
/// message (role names, elapsed quiet time) without re-locking the
/// state maps in the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TtlNudgeTarget {
    pub waiter: String,
    pub key: String,
    pub producer_aid: String,
    pub producer_role: String,
    pub producer_last_append_ms: i64,
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
    let kick_text = format!("blackboard `{key}` updated; please check");
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
    // M6d-6: hold the lock only long enough to clone the input sender
    // AND grab a stream handle for the activity check. Both fields are
    // cheap to clone (Arc / mpsc::Sender). Avoids any await while
    // holding the parking_lot mutex.
    let (input_tx, stream) = {
        let guard = slot.lock();
        (guard.input_tx.clone(), guard.stream.clone())
    };

    // M6d-6: skip the destructive PTY kick when output has flowed
    // recently — the agent is mid-turn and the mailbox alone will
    // catch the wake on its next Stop hook. Mailbox is already
    // delivered by the caller; this is the "don't pollute the buffer"
    // gate. Return Ok so the caller doesn't reap the subscription as
    // dead.
    let now = now_ms();
    let last = stream.last_append_ms();
    if !pty_quiet_enough_to_inject(last, now) {
        tracing::info!(
            agent_id,
            key = key_for_log,
            last_output_ms_ago = now.saturating_sub(last),
            "skipping wake PTY-inject; agent appears mid-stream (mailbox already delivered)"
        );
        return Ok(());
    }

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

pub struct WakeCoordinator {
    swarm: Arc<Swarm>,
    registry: Registry,
    subs: WakeSubs,
    exit_keys: ExitKeys,
    /// M6d-5b: per-pair rate-limit for TTL nudges. Populated lazily —
    /// only after a nudge actually fires — so the scanner doesn't
    /// re-nudge the same (waiter, key) every minute while the
    /// producer stays quiet. Pruned on BlackboardChanged (key landed)
    /// and on AgentState::Exited (waiter died).
    wake_nudged: WakeNudged,
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
        wake_nudged: WakeNudged,
    ) -> JoinHandle<()> {
        let me = Self {
            swarm,
            registry,
            subs,
            exit_keys,
            wake_nudged,
        };
        tokio::spawn(me.run())
    }

    async fn run(self) {
        use tokio::sync::broadcast::error::RecvError;
        let mut rx = self.swarm.subscribe();
        // M6d-5: separate ticker for the TTL scanner. Using
        // `tokio::select!` keeps both event-driven and time-driven
        // work in the same task — one less JoinHandle to manage, and
        // the borrow on `self` stays single-owner.
        let mut ttl_tick =
            tokio::time::interval(std::time::Duration::from_secs(TTL_TICK_SECS));
        // Skip the immediate first tick (intervals fire at t=0 by
        // default). No subscriber will have been added before the
        // coordinator starts, so the first tick would always no-op
        // anyway — this just keeps the logs cleaner.
        ttl_tick.tick().await;

        loop {
            tokio::select! {
                msg = rx.recv() => match msg {
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

                        // M6d-5b: prune nudge-rate-limit rows for any
                        // key that just landed so a future re-subscription
                        // on the same key doesn't get spuriously rate-
                        // limited by yesterday's nudge. We prune for
                        // every fan-out key (including aliases) because
                        // a `frontend.done.error` resolves
                        // `frontend.done`-subscribers in the same tick.
                        for key in &keys_to_fan {
                            prune_wake_nudged_by_key(&self.wake_nudged, key).await;
                        }

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
                    }
                    Ok(SwarmEvent::AgentState { agent_id, state }) => {
                        if matches!(state, flockmux_protocol::ws_swarm::AgentState::Exited) {
                            self.handle_agent_exit(&agent_id).await;
                            // M6d-5b: agent gone — drop any nudge-rate-
                            // limit rows it owned as waiter. Producer-
                            // side cleanup is handled by handle_agent_exit's
                            // existing exit_keys path (it writes .error
                            // which prunes wake_nudged via the
                            // BlackboardChanged arm above).
                            unregister_wake_nudged(&self.wake_nudged, &agent_id).await;
                        }
                    }
                    Ok(_) => {} // ignore the rest (message, message_read)
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
                },
                _ = ttl_tick.tick() => {
                    self.scan_ttl().await;
                }
            }
        }
    }

    /// M6d-5b: walk the subscription table, check each producer's PTY
    /// quiet time, and fire a nudge for every (waiter, key) whose
    /// producer has been silent ≥ `TTL_THRESHOLD_MS`. Selection logic
    /// lives in the pure `select_ttl_targets`; this function only
    /// gathers snapshots and dispatches messages.
    ///
    /// What this catches: a producer agent that's alive (registry
    /// slot exists) but whose PTY hasn't emitted a chunk in 5+
    /// minutes — i.e. genuinely stuck (waiting for user input,
    /// looping on a confirm prompt, mid-network-failure). What it
    /// doesn't catch: producer dead — that's `handle_agent_exit`'s
    /// `.error` fallback. What it correctly *doesn't* false-trigger
    /// on (regression from the v1 design): healthy chained spells
    /// where the producer simply hasn't reached its turn yet — those
    /// producers either don't exist in the registry yet or have a
    /// `last_append_ms` of 0 (never streamed) and the helper skips
    /// them on purpose.
    async fn scan_ttl(&self) {
        let now = now_ms();
        let subs_snap = self.subs.read().await.clone();
        if subs_snap.is_empty() {
            return;
        }
        let exit_keys_snap = self.exit_keys.read().await.clone();
        if exit_keys_snap.is_empty() {
            return;
        }
        let nudged_snap = self.wake_nudged.read().await.clone();
        // Collect last_append_ms for every producer we might nudge.
        // The lookup is cheap (Arc + atomic) so we capture all
        // candidate producers — not all of them will be selected
        // by the helper, but the upper bound is tiny (≤ N agents
        // per spell).
        let mut producer_streams: HashMap<String, i64> = HashMap::new();
        for aid in exit_keys_snap.keys() {
            if let Some(slot) = self.registry.get(aid) {
                let stream = slot.lock().stream.clone();
                producer_streams.insert(aid.clone(), stream.last_append_ms());
            }
        }
        // M6d-5c: snapshot the FS-present blackboard keys. Used by
        // `select_ttl_targets` to enforce gates 4 (key still missing)
        // and 5 (producer's own deps satisfied). One read_dir per
        // tick keeps the lock-amortised cost bounded.
        let bb_root = self.swarm.blackboard_root().to_path_buf();
        let keys_on_blackboard: std::collections::HashSet<String> =
            tokio::task::spawn_blocking(move || {
                let mut out: std::collections::HashSet<String> = Default::default();
                if let Ok(rd) = std::fs::read_dir(&bb_root) {
                    for entry in rd.flatten() {
                        if entry
                            .file_type()
                            .ok()
                            .is_some_and(|t| t.is_file())
                        {
                            if let Ok(name) = entry.file_name().into_string() {
                                out.insert(name);
                            }
                        }
                    }
                }
                out
            })
            .await
            .unwrap_or_default();
        let targets = select_ttl_targets(
            &subs_snap,
            &exit_keys_snap,
            &producer_streams,
            &nudged_snap,
            &keys_on_blackboard,
            now,
            TTL_THRESHOLD_MS,
        );
        if targets.is_empty() {
            return;
        }

        for target in targets {
            let quiet_for_min =
                (now.saturating_sub(target.producer_last_append_ms) / 60_000) as i64;
            // Friendly waiter name for the message body. Falls back
            // to the agent_id if the waiter isn't in exit_keys
            // (planner / inline-only roles); the agent_id is still
            // human-actionable through the swarm panel.
            let waiter_label = exit_keys_snap
                .get(&target.waiter)
                .map(|ek| ek.role.clone())
                .unwrap_or_else(|| target.waiter.clone());
            let body = format!(
                "TTL nudge: your PTY has been quiet for {quiet_for_min} min while \
                 `{waiter_label}` waits for your handoff signal `{}`. \
                 If you're stuck, please progress; if you're done, please write \
                 `{}` (success) or `{}.error` (failure) to the blackboard so the \
                 spell can advance.",
                target.key, target.key, target.producer_role,
            );
            let msg = NewMessage {
                from_agent: "system".into(),
                to_agent: target.producer_aid.clone(),
                kind: "wake".into(),
                body,
                sent_at: now,
                in_reply_to: None,
            };
            match self.swarm.send_message(msg).await {
                Ok(_) => {
                    tracing::info!(
                        producer_aid = target.producer_aid,
                        producer_role = target.producer_role,
                        waiter_label,
                        key = target.key,
                        quiet_for_min,
                        "TTL nudge sent"
                    );
                    // Custom PTY-kick text — the standard "blackboard
                    // <K> updated; please check" wording is wrong here
                    // because <K> is the recipient's own handoff
                    // signal, not something they should read. Tell
                    // them what they actually need to do.
                    let kick_text = format!(
                        "you are blocking `{}` on `{}`; please progress (or write `{}.error`)",
                        waiter_label, target.key, target.producer_role,
                    );
                    if let Err(err) = inject_with_kick_text(
                        &self.registry,
                        &target.producer_aid,
                        &kick_text,
                        &target.key,
                    )
                    .await
                    {
                        tracing::debug!(
                            ?err,
                            producer_aid = target.producer_aid,
                            "TTL nudge PTY inject failed (mailbox delivered, will catch on next Stop)"
                        );
                    }
                    // Mark this pair as nudged so we don't re-fire
                    // every tick. Will be cleared by BlackboardChanged
                    // (key landed) or waiter Exited (cleanup), or by
                    // the rate-limit window elapsing naturally.
                    let mut w = self.wake_nudged.write().await;
                    w.insert((target.waiter, target.key), now);
                }
                Err(err) => {
                    tracing::warn!(
                        ?err,
                        producer_aid = target.producer_aid,
                        key = target.key,
                        "TTL nudge send_message failed; will retry next tick"
                    );
                    // Don't record this as nudged — we want to retry.
                }
            }
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

        // Naming: `<role>.error` matches the convention agents already
        // self-write when they detect their own failure (see
        // `roles/frontend.md` Upstream-failed branch). Using the same
        // key for the auto-synthesised failure means downstream role
        // prompts only need to check ONE key — they get the same value
        // whether the producer aborted gracefully or crashed.
        let error_key = format!("{}.error", ek.role);
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

    // ── M6d-5b: TTL based on producer PTY quiet time ────────────────────

    fn mk_exit_keys(entries: &[(&str, &str, &str)]) -> HashMap<String, ExitKey> {
        // (agent_id, role, handoff_signal) tuples.
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

    fn mk_streams(entries: &[(&str, i64)]) -> HashMap<String, i64> {
        entries
            .iter()
            .map(|(aid, t)| (aid.to_string(), *t))
            .collect()
    }

    const TTL: i64 = 5 * 60 * 1000;
    const NOW: i64 = 1_700_000_000_000;

    fn no_keys() -> std::collections::HashSet<String> {
        Default::default()
    }
    fn keys(items: &[&str]) -> std::collections::HashSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn select_ttl_fires_when_producer_quiet_beyond_threshold() {
        // critic waits on fixer.done; fixer is registered with
        // depends_on=[review.completed], and review.completed IS on
        // the blackboard — so fixer is "able to act" but hasn't.
        // fixer's PTY last appended 10 min ago. select should fire.
        let subs = build_subs(&[
            ("critic-1", &["fixer.done"]),
            ("fixer-1", &["review.completed"]),
        ]);
        let ek = mk_exit_keys(&[
            ("fixer-1", "fixer", "fixer.done"),
            ("critic-1", "critic", "review.completed"),
        ]);
        let streams = mk_streams(&[("fixer-1", NOW - 10 * 60 * 1000)]);
        let nudged = HashMap::new();
        let bb = keys(&["review.completed"]); // fixer's input met; fixer.done absent
        let out = select_ttl_targets(&subs, &ek, &streams, &nudged, &bb, NOW, TTL);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].waiter, "critic-1");
        assert_eq!(out[0].key, "fixer.done");
        assert_eq!(out[0].producer_aid, "fixer-1");
        assert_eq!(out[0].producer_role, "fixer");
    }

    #[test]
    fn select_ttl_skips_when_producer_active() {
        let subs = build_subs(&[("critic-1", &["fixer.done"])]);
        let ek = mk_exit_keys(&[("fixer-1", "fixer", "fixer.done")]);
        let streams = mk_streams(&[("fixer-1", NOW - 1_000)]);
        let nudged = HashMap::new();
        let out = select_ttl_targets(&subs, &ek, &streams, &nudged, &no_keys(), NOW, TTL);
        assert!(out.is_empty());
    }

    #[test]
    fn select_ttl_skips_when_producer_never_started() {
        let subs = build_subs(&[("critic-1", &["fixer.done"])]);
        let ek = mk_exit_keys(&[("fixer-1", "fixer", "fixer.done")]);
        let streams = mk_streams(&[("fixer-1", 0)]);
        let nudged = HashMap::new();
        let out = select_ttl_targets(&subs, &ek, &streams, &nudged, &no_keys(), NOW, TTL);
        assert!(out.is_empty());
    }

    #[test]
    fn select_ttl_skips_external_keys() {
        let subs = build_subs(&[("fe-1", &["design.approved"])]);
        let ek = mk_exit_keys(&[("fe-1", "frontend", "frontend.done")]);
        let streams = mk_streams(&[("fe-1", NOW - 10 * 60 * 1000)]);
        let nudged = HashMap::new();
        let out = select_ttl_targets(&subs, &ek, &streams, &nudged, &no_keys(), NOW, TTL);
        assert!(out.is_empty());
    }

    #[test]
    fn select_ttl_rate_limits_repeat_nudges() {
        let subs = build_subs(&[("critic-1", &["fixer.done"])]);
        let ek = mk_exit_keys(&[("fixer-1", "fixer", "fixer.done")]);
        let streams = mk_streams(&[("fixer-1", NOW - 10 * 60 * 1000)]);
        let mut nudged = HashMap::new();
        nudged.insert(
            ("critic-1".to_string(), "fixer.done".to_string()),
            NOW - 2 * 60 * 1000,
        );
        let out = select_ttl_targets(&subs, &ek, &streams, &nudged, &no_keys(), NOW, TTL);
        assert!(out.is_empty());
    }

    #[test]
    fn select_ttl_re_fires_after_rate_limit_window() {
        let subs = build_subs(&[("critic-1", &["fixer.done"])]);
        let ek = mk_exit_keys(&[("fixer-1", "fixer", "fixer.done")]);
        let streams = mk_streams(&[("fixer-1", NOW - 10 * 60 * 1000)]);
        let mut nudged = HashMap::new();
        nudged.insert(
            ("critic-1".to_string(), "fixer.done".to_string()),
            NOW - 6 * 60 * 1000,
        );
        let out = select_ttl_targets(&subs, &ek, &streams, &nudged, &no_keys(), NOW, TTL);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn select_ttl_excludes_self_subscribe_edge_case() {
        let subs = build_subs(&[("solo-1", &["solo.done"])]);
        let ek = mk_exit_keys(&[("solo-1", "solo", "solo.done")]);
        let streams = mk_streams(&[("solo-1", NOW - 10 * 60 * 1000)]);
        let nudged = HashMap::new();
        let out = select_ttl_targets(&subs, &ek, &streams, &nudged, &no_keys(), NOW, TTL);
        assert!(out.is_empty());
    }

    // ── M6d-5c: the two new gates ──────────────────────────────────────

    #[test]
    fn select_ttl_skips_when_key_already_on_blackboard() {
        // Regression test for 2026-05-23 e2e #2: FE wrote frontend.done
        // 5+ minutes ago; the TTL scanner saw critic still subscribed
        // (wake_subs doesn't shrink on key landing) and FE's PTY quiet
        // (FE finished its job → no more output) — under M6d-5b that
        // fired a nudge to FE saying "you owe critic frontend.done",
        // and FE wasted a turn looking at the already-landed key.
        // Now we check the blackboard up-front and skip.
        let subs = build_subs(&[("critic-1", &["frontend.done"])]);
        let ek = mk_exit_keys(&[("fe-1", "frontend", "frontend.done")]);
        let streams = mk_streams(&[("fe-1", NOW - 10 * 60 * 1000)]);
        let nudged = HashMap::new();
        let bb = keys(&["frontend.done"]); // ← already on disk
        let out = select_ttl_targets(&subs, &ek, &streams, &nudged, &bb, NOW, TTL);
        assert!(
            out.is_empty(),
            "key already on blackboard → consumer not waiting, must not nudge"
        );
    }

    #[test]
    fn select_ttl_skips_when_producer_deps_not_satisfied() {
        // Regression test for the OTHER 2026-05-23 e2e #2 failure:
        // fixer subscribed to review.completed; review.completed
        // hadn't landed yet (critic was still finishing round 1).
        // Under M6d-5b, fixer's PTY was quiet since bootstrap →
        // nudge fired → fixer fabricated a fake fixer.done to placate
        // the message. New gate: producer's own deps must all be met
        // before we count it as "stuck".
        let subs = build_subs(&[
            ("critic-1", &["fixer.done"]),
            ("fixer-1", &["review.completed"]),
        ]);
        let ek = mk_exit_keys(&[
            ("fixer-1", "fixer", "fixer.done"),
            ("critic-1", "critic", "review.completed"),
        ]);
        let streams = mk_streams(&[("fixer-1", NOW - 10 * 60 * 1000)]);
        let nudged = HashMap::new();
        // review.completed NOT on blackboard → fixer can't act yet.
        let bb = no_keys();
        let out = select_ttl_targets(&subs, &ek, &streams, &nudged, &bb, NOW, TTL);
        assert!(
            out.is_empty(),
            "fixer's own dep review.completed is missing → fixer is idle by design, must not nudge"
        );
    }

    #[test]
    fn select_ttl_fires_when_deps_met_and_key_missing() {
        // Positive case for both new gates: critic waits on fixer.done,
        // fixer's input review.completed IS on the blackboard, but
        // fixer hasn't produced fixer.done and has been quiet 10 min.
        // This is the case where the TTL nudge IS legitimate.
        let subs = build_subs(&[
            ("critic-1", &["fixer.done"]),
            ("fixer-1", &["review.completed"]),
        ]);
        let ek = mk_exit_keys(&[
            ("fixer-1", "fixer", "fixer.done"),
            ("critic-1", "critic", "review.completed"),
        ]);
        let streams = mk_streams(&[("fixer-1", NOW - 10 * 60 * 1000)]);
        let nudged = HashMap::new();
        let bb = keys(&["review.completed"]); // fixer's dep met
        let out = select_ttl_targets(&subs, &ek, &streams, &nudged, &bb, NOW, TTL);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].producer_role, "fixer");
    }

    #[test]
    fn select_ttl_strict_spell_at_5min_no_false_positive() {
        // Full snapshot of fullstack-feature-strict at T+5min, modeled
        // on the actual 2026-05-23 e2e #2 trace. Every producer is
        // either finished-and-quiet (FE/BE) or correctly waiting
        // (critic, fixer, test). Expected: no nudges.
        let subs = build_subs(&[
            ("test-1", &["review.completed"]),
            ("fixer-1", &["review.completed"]),
            ("critic-1", &["frontend.done", "backend.done", "fixer.done"]),
            // FE waits on api.spec; BE has no deps (depends_on=[])
            ("fe-1", &["api.spec"]),
        ]);
        let ek = mk_exit_keys(&[
            ("fe-1", "frontend", "frontend.done"),
            ("be-1", "backend", "backend.done"),
            ("critic-1", "critic", "review.completed"),
            ("fixer-1", "fixer", "fixer.done"),
            ("test-1", "test", "test.passed"),
        ]);
        // FE and BE both finished long ago and went quiet.
        // critic is mid-streaming. fixer hasn't started.
        // test hasn't started.
        let streams = mk_streams(&[
            ("fe-1", NOW - 8 * 60 * 1000),
            ("be-1", NOW - 7 * 60 * 1000),
            ("critic-1", NOW - 200), // mid-review
            ("fixer-1", 0),
            ("test-1", 0),
        ]);
        let nudged = HashMap::new();
        // Blackboard has FE/BE's done + reviews but not review.completed yet.
        let bb = keys(&[
            "api.spec",
            "frontend.done",
            "backend.done",
            "frontend.review",
            // backend.review still being written by critic
        ]);
        let out = select_ttl_targets(&subs, &ek, &streams, &nudged, &bb, NOW, TTL);
        assert!(
            out.is_empty(),
            "every producer in this snapshot is either done (key on bb) or correctly idle \
             (deps not met); got false positives: {out:?}"
        );
    }

    #[tokio::test]
    async fn wake_nudged_unregister_drops_only_targeted_waiter() {
        let nudged: WakeNudged = Arc::new(RwLock::new(HashMap::new()));
        nudged.write().await.insert(("test-a".into(), "x.done".into()), 1);
        nudged.write().await.insert(("test-b".into(), "x.done".into()), 1);
        unregister_wake_nudged(&nudged, "test-a").await;
        let snap = nudged.read().await;
        assert!(snap.get(&("test-a".into(), "x.done".into())).is_none());
        assert!(snap.get(&("test-b".into(), "x.done".into())).is_some());
    }

    #[tokio::test]
    async fn wake_nudged_prune_drops_all_waiters_for_key() {
        let nudged: WakeNudged = Arc::new(RwLock::new(HashMap::new()));
        nudged.write().await.insert(("a".into(), "x.done".into()), 1);
        nudged.write().await.insert(("b".into(), "x.done".into()), 1);
        nudged.write().await.insert(("a".into(), "y.done".into()), 1);
        prune_wake_nudged_by_key(&nudged, "x.done").await;
        let snap = nudged.read().await;
        assert!(snap.get(&("a".into(), "x.done".into())).is_none());
        assert!(snap.get(&("b".into(), "x.done".into())).is_none());
        assert!(snap.get(&("a".into(), "y.done".into())).is_some());
    }

    // ── M6d-6: PTY activity-based inject gate ───────────────────────────

    #[test]
    fn pty_quiet_enough_when_never_appended() {
        // last_append_ms == 0 is the sentinel for "stream has never
        // seen output". A wake-inject here cannot pollute anything.
        assert!(pty_quiet_enough_to_inject(0, 1_700_000_000_000));
    }

    #[test]
    fn pty_not_quiet_when_recent_output() {
        // 500 ms ago — well inside the 2 s threshold; mid-stream.
        let now = 1_700_000_000_000_i64;
        let last = now - 500;
        assert!(!pty_quiet_enough_to_inject(last, now));
    }

    #[test]
    fn pty_quiet_enough_when_output_old_enough() {
        // 3 s ago — past the 2 s quiet bar; safe to inject.
        let now = 1_700_000_000_000_i64;
        let last = now - 3_000;
        assert!(pty_quiet_enough_to_inject(last, now));
    }

    #[test]
    fn pty_quiet_at_exact_threshold_allows_inject() {
        // Boundary case: gap == threshold counts as "quiet enough".
        // Inclusive on the safe side because the threshold is already
        // generous — being strict at the edge would just add flake.
        let now = 1_700_000_000_000_i64;
        let last = now - PTY_QUIET_MS_FOR_INJECT;
        assert!(pty_quiet_enough_to_inject(last, now));
    }

    #[test]
    fn pty_quiet_handles_clock_skew_gracefully() {
        // `now < last_append_ms` should not panic and should NOT count
        // as quiet — the safer default is to skip injection until time
        // catches up. saturating_sub ensures no underflow.
        let now = 1_700_000_000_000_i64;
        let last = now + 1_000;
        assert!(!pty_quiet_enough_to_inject(last, now));
    }
}
