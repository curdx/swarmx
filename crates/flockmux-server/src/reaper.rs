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

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use flockmux_protocol::ws_swarm::{AgentState, SwarmEvent};
use flockmux_storage::Store;
use flockmux_swarm::Swarm;
use parking_lot::Mutex;
use tokio::time::MissedTickBehavior;

use crate::registry::{AgentSlot, LifecycleEvent, Registry};

/// How often the reaper sweeps the registry. 5s keeps the worst-case lag
/// between "process actually died" and "UI stops lying" well under the 10s the
/// honesty work targets, without meaningfully waking the runtime.
const SWEEP_INTERVAL: Duration = Duration::from_secs(5);

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
        loop {
            tick.tick().await;
            sweep_once(&registry, &store, &swarm).await;
        }
    });
}

/// One pass over the registry. The exit-code persist/publish is done OUTSIDE
/// the slot lock (`detect_exit` releases it before returning) so we never hold
/// a `parking_lot` mutex across the `.await`.
async fn sweep_once(registry: &Registry, store: &Store, swarm: &Swarm) {
    for (agent_id, slot_arc) in registry.list() {
        let Some(code) = detect_exit(slot_arc.as_ref()) else {
            continue;
        };

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
    }
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
    if slot.bridge.is_alive() {
        return None;
    }
    let code = slot.bridge.try_exit_code().unwrap_or(-1);
    slot.lifecycle.lock().shim_exit = Some(code);
    // Relay to any live PTY subscriber so it updates immediately; its consumer
    // is idempotent with the direct persist/publish the caller does.
    let _ = slot.lifecycle_tx.send(LifecycleEvent::ShimExit(code));
    Some(code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pty_stream::PtyStream;
    use crate::registry::Lifecycle;
    use flockmux_pty::{PtyBridge, PtyHandles, SpawnOpts};
    use std::collections::HashMap;
    use std::sync::atomic::AtomicBool;

    fn slot_for(cmd: &str) -> Arc<Mutex<AgentSlot>> {
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
        let slot = AgentSlot {
            bridge: Arc::new(bridge),
            stream: Arc::new(PtyStream::new()),
            lifecycle: Arc::new(Mutex::new(Lifecycle::default())),
            lifecycle_tx,
            input_tx,
            cli: "test".into(),
            role: "test".into(),
            workspace: "/tmp".into(),
            paused: Arc::new(AtomicBool::new(false)),
            mcp_ready: tokio::sync::watch::channel(false).0,
        };
        Arc::new(Mutex::new(slot))
    }

    async fn wait_until_dead(slot: &Mutex<AgentSlot>) {
        for _ in 0..200 {
            if !slot.lock().bridge.is_alive() {
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
}
