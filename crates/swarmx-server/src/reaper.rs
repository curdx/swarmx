//! Liveness reaper — defense-in-depth against agents stuck forever "alive".
//!
//! Two paths already retire an agent: `scan_osc` emits a `ShimExit` when the
//! shim prints its OSC exit marker, and the PTY pump synthesizes one on EOF
//! (spawn.rs). But a shim killed by SIGKILL / OOM — or a pump task that died
//! before it could 补发 — can leave the registry believing a child is alive,
//! showing a green dot + "正在响应" indefinitely. That is exactly the fake
//! state the honesty work is removing.
//!
//! This periodic sweep is the backstop. Every few seconds it asks each live
//! `PtyBridge` `is_alive()` — a deterministic `waitpid`, never a false positive
//! — and for any child that has actually exited without a recorded `ShimExit`,
//! it synthesizes one: latching `lifecycle.shim_exit`, relaying to live PTY
//! subscribers, persisting via `record_shim_exit`, and publishing the
//! `AgentState` change — the same effects the WS lifecycle consumer produces,
//! but without depending on a WS client being attached.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use swarmx_protocol::ws_swarm::{AgentState, SwarmEvent};
use swarmx_storage::Store;
use swarmx_swarm::Swarm;
use parking_lot::Mutex;
use tokio::time::MissedTickBehavior;

use crate::registry::{AgentSlot, LifecycleEvent, Registry};

/// How often the reaper sweeps the registry. 5s keeps the worst-case lag
/// between "process actually died" and "UI stops lying" well under the 10s the
/// honesty work targets, without meaningfully waking the runtime.
const SWEEP_INTERVAL: Duration = Duration::from_secs(5);

/// Grace between a slot's exit being recorded and its eviction from the
/// registry. Long enough that a just-detached UI can reattach for the final
/// bytes; short enough that dead slots don't accumulate. Eviction reclaims the
/// three resources a self-exited agent otherwise leaks forever: the parked PTY
/// writer thread, its master fd, and a slot against the live-agent cap (which is
/// literally `registry.list().len()`). The persisted SQLite row stays the source
/// of truth for the exited-agent list the UI renders — the registry holds only
/// live-or-recently-exited agents.
const REAP_GRACE: Duration = Duration::from_secs(60);

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Spawn the liveness reaper for the whole process. The task exits only at
/// shutdown; callers drop the handle (same pattern as the wake coordinator).
pub fn spawn(registry: Registry, store: Arc<Store>, swarm: Arc<Swarm>) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(SWEEP_INTERVAL);
        tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
        // Per-agent clock started when a slot's exit is first observed; drives
        // the REAP_GRACE eviction below.
        let mut exited_at: HashMap<String, Instant> = HashMap::new();
        loop {
            tick.tick().await;
            sweep_once(&registry, &store, &swarm, &mut exited_at, REAP_GRACE).await;
        }
    });
}

/// One pass over the registry. The exit-code persist/publish is done OUTSIDE
/// the slot lock (`detect_exit` releases it before returning) so we never hold
/// a `parking_lot` mutex across the `.await`.
async fn sweep_once(
    registry: &Registry,
    store: &Store,
    swarm: &Swarm,
    exited_at: &mut HashMap<String, Instant>,
    grace: Duration,
) {
    let mut present: HashSet<String> = HashSet::new();
    for (agent_id, slot_arc) in registry.list() {
        present.insert(agent_id.clone());

        if let Some(code) = detect_exit(slot_arc.as_ref()) {
            let at = now_ms();
            if let Err(e) = store.record_shim_exit(agent_id.clone(), code, at).await {
                tracing::warn!(?e, agent = %agent_id, "reaper: record_shim_exit failed");
            }
            // Non-zero exit = abnormal death → Error (red, sorted to top); clean
            // exit → Exited. Intentional kills also exit non-zero, but those rows
            // carry `killed_at`, which the UI prioritizes over this.
            let next = if code == 0 {
                AgentState::Exited
            } else {
                AgentState::Error
            };
            swarm.publish_event(SwarmEvent::AgentState {
                agent_id: agent_id.clone(),
                state: next,
            });
            tracing::info!(
                agent = %agent_id,
                code,
                "reaper: child exited without a ShimExit marker; synthesized one"
            );
            // Start the eviction clock now that the exit is recorded.
            exited_at.insert(agent_id.clone(), Instant::now());
            continue;
        }

        // Already-accounted exit (recorded elsewhere too — the WS lifecycle
        // consumer or the pump's EOF). Evict after the grace so the parked writer
        // thread, master fd, and live-agent-cap slot are reclaimed instead of
        // leaking until process exit. Dropping the registry's `Arc` at end of
        // this iteration is what actually reaps them.
        if is_reapable(slot_arc.as_ref()) {
            let since = *exited_at.entry(agent_id.clone()).or_insert_with(Instant::now);
            if since.elapsed() >= grace {
                registry.remove(&agent_id);
                exited_at.remove(&agent_id);
                tracing::info!(
                    agent = %agent_id,
                    "reaper: evicted exited slot (reclaimed writer thread + master fd + live-agent slot)"
                );
            }
        }
    }
    // Forget clocks for agents already gone from the registry (teardown / auto-kill).
    exited_at.retain(|id, _| present.contains(id));
}

/// Inspect one slot. If its child has exited but no `ShimExit` was ever
/// recorded, latch `lifecycle.shim_exit`, relay to live subscribers, and
/// return the exit code to persist. Returns `None` when the agent is still
/// alive or its exit was already accounted for (so a later sweep or a racing
/// WS consumer never double-counts). Synchronous — the slot lock is dropped
/// when this returns, before the caller's DB write.
fn detect_exit(slot: &Mutex<AgentSlot>) -> Option<i32> {
    let slot = slot.lock();
    if slot.lifecycle.lock().shim_exit.is_some() {
        return None;
    }
    if slot.is_alive() {
        return None;
    }
    let code = slot.try_exit_code().unwrap_or(-1);
    slot.lifecycle.lock().shim_exit = Some(code);
    // Relay to any live PTY subscriber so it updates immediately; its consumer
    // is idempotent with the direct persist/publish the caller does.
    let _ = slot.lifecycle_tx.send(LifecycleEvent::ShimExit(code));
    Some(code)
}

/// True when a slot's exit is already recorded (`shim_exit` latched) and its
/// child is confirmed dead — i.e. a spent slot safe to evict from the registry
/// after the grace. Distinct from `detect_exit`: that one *accounts* a new death
/// (and skips already-latched slots); this one *identifies* an already-accounted
/// death to reclaim, so the two compose across sweeps without double-counting.
fn is_reapable(slot: &Mutex<AgentSlot>) -> bool {
    let slot = slot.lock();
    slot.lifecycle.lock().shim_exit.is_some() && !slot.is_alive()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pty_stream::PtyStream;
    use crate::registry::{AgentChannel, Lifecycle};
    use swarmx_pty::{PtyBridge, PtyHandles, SpawnOpts};
    use std::collections::HashMap;
    use std::sync::atomic::AtomicBool;

    fn slot_for(cmd: &str) -> Arc<Mutex<AgentSlot>> {
        Arc::new(Mutex::new(agent_slot_for(cmd)))
    }

    fn agent_slot_for(cmd: &str) -> AgentSlot {
        let PtyHandles {
            bridge,
            output_rx: _output_rx,
        } = PtyBridge::spawn(SpawnOpts {
            argv: &["/bin/sh".into(), "-c".into(), cmd.into()],
            cwd: None,
            env: HashMap::new(),
            cols: 80,
            rows: 24,
        })
        .expect("spawn test child");
        let input_tx = bridge.input_sender();
        let (lifecycle_tx, _rx) = tokio::sync::broadcast::channel(16);
        AgentSlot {
            channel: AgentChannel::Pty {
                bridge: Arc::new(bridge),
                stream: Arc::new(PtyStream::new()),
                input_tx,
            },
            lifecycle: Arc::new(Mutex::new(Lifecycle::default())),
            lifecycle_tx,
            cli: "test".into(),
            role: "test".into(),
            workspace: "/tmp".into(),
            paused: Arc::new(AtomicBool::new(false)),
            mcp_ready: tokio::sync::watch::channel(false).0,
            tui_http_port: None,
            serve_http_port: None,
            zulu: None,
        }
    }

    async fn wait_until_dead(slot: &Mutex<AgentSlot>) {
        for _ in 0..200 {
            if !slot.lock().is_alive() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("child did not exit in time");
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn detects_dead_child_and_latches_once() {
        let slot = slot_for("exit 0");
        wait_until_dead(&slot).await;

        // First sweep synthesizes the exit (code 0) and latches lifecycle.
        assert_eq!(detect_exit(&slot), Some(0));
        assert_eq!(slot.lock().lifecycle.lock().shim_exit, Some(0));
        // Second sweep is a no-op — already accounted for.
        assert_eq!(detect_exit(&slot), None);
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn nonzero_exit_code_is_surfaced() {
        let slot = slot_for("exit 7");
        wait_until_dead(&slot).await;
        assert_eq!(detect_exit(&slot), Some(7));
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn live_child_is_left_alone() {
        // `sleep 10` stays alive across the check.
        let slot = slot_for("sleep 10");
        assert_eq!(detect_exit(&slot), None);
        assert_eq!(slot.lock().lifecycle.lock().shim_exit, None);
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn reapable_only_after_exit_is_accounted() {
        let dead = slot_for("exit 0");
        wait_until_dead(&dead).await;
        // Dead but not yet accounted (no shim_exit) → not reapable.
        assert!(!is_reapable(&dead));
        // After detect_exit latches shim_exit → reapable.
        assert_eq!(detect_exit(&dead), Some(0));
        assert!(is_reapable(&dead));
        // A live child is never reapable.
        let live = slot_for("sleep 10");
        assert!(!is_reapable(&live));
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn sweep_evicts_exited_slot_after_grace() {
        use swarmx_storage::Store;
        use swarmx_swarm::Swarm;
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&dir.path().join("db.sqlite")).await.unwrap());
        let swarm = Swarm::new(store.clone(), dir.path().join("bb"));
        let registry = Registry::new();
        registry.insert("a1".into(), agent_slot_for("exit 0"));
        // Wait for the child to actually die.
        if let Some(s) = registry.get("a1") {
            wait_until_dead(&s).await;
        }

        let mut exited_at = HashMap::new();
        // Sweep 1 (long grace): accounts the exit, starts the clock, keeps the slot.
        sweep_once(&registry, &store, &swarm, &mut exited_at, Duration::from_secs(60)).await;
        assert!(registry.get("a1").is_some(), "slot kept during grace");
        // Sweep 2 (zero grace): the accounted exit is now evicted → slot + writer
        // thread + fd + live-agent-cap slot reclaimed.
        sweep_once(&registry, &store, &swarm, &mut exited_at, Duration::ZERO).await;
        assert!(registry.get("a1").is_none(), "exited slot evicted after grace");
        assert!(registry.list().is_empty(), "live-agent cap reclaimed");
    }
}
