//! ACP (Agent Client Protocol) engine: spawn a CLI that speaks ACP over stdio
//! (today: `opencode acp`) as a PIPED child — NOT a PTY — and drive it as the
//! ACP *client* with flockmux owning the turn loop.
//!
//! This is the live counterpart to [`crate::acp`] (the codec + `Connection` +
//! `AcpSession` session layer, wire-verified against opencode 1.17.7). Here we:
//!   1. spawn the child with piped stdin/stdout (the SAME env allowlist the PTY
//!      path builds — HOME/PATH/OPENCODE_CONFIG/FLOCKMUX_* …),
//!   2. run a long-lived driver task: `initialize` → `session/new` (→ Ready via
//!      the shared lifecycle channel), then one `session/prompt` per delivered
//!      prompt (the bootstrap first turn, then wakes), mapping streamed
//!      `session/update`s onto `AgentActivity`,
//!   3. hand back an [`AgentChannel::Acp`] so the registry/reaper/wake treat it
//!      uniformly with PTY agents (liveness via the child, wakes via the prompt
//!      channel).
//!
//! AgentState is published the same way as PTY: `ShimReady`/`HealthFail` ride
//! the slot's `lifecycle_tx` (the existing subscriber maps them to Ready/Error),
//! the reaper synthesizes `ShimExit` when the child dies (its `is_alive` now
//! works for ACP), and Thinking/Idle are published directly per turn.

use crate::acp::{AcpSession, AcpUpdate, ConnError, Connection};
use crate::plugins::CliPlugin;
use crate::registry::{AgentChannel, LifecycleEvent};
use crate::transcript::emit_activity;
use anyhow::{Context, Result};
use flockmux_protocol::ws_swarm::{AgentState, SwarmEvent};
use flockmux_swarm::Swarm;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use tokio::process::Command;
use tokio::sync::{broadcast, mpsc};

fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Resolve a bare binary name against the augmented runtime PATH so the spawn
/// works even when launched from a Finder/launchd short PATH (the same reason
/// the PTY path augments PATH). Falls back to the bare name (OS PATH lookup) if
/// not found on disk.
fn resolve_binary(name: &str) -> PathBuf {
    if name.contains('/') {
        return PathBuf::from(name);
    }
    let path = crate::runtime_path::augmented_path();
    for dir in std::env::split_paths(&path) {
        let cand = dir.join(name);
        if cand.is_file() {
            return cand;
        }
    }
    PathBuf::from(name)
}

/// Spawn an ACP-transport agent and its driver task. Returns the
/// [`AgentChannel::Acp`] for the caller to seat in an `AgentSlot`. `env` is the
/// fully-built spawn environment (moved in); `lifecycle_tx` is the slot's shared
/// lifecycle channel the driver emits readiness/health onto; `swarm` is used to
/// publish per-turn activity + Thinking/Idle states.
pub fn spawn_acp(
    plugin: &CliPlugin,
    workspace: &Path,
    env: HashMap<String, String>,
    agent_id: String,
    swarm: Arc<Swarm>,
    lifecycle_tx: broadcast::Sender<LifecycleEvent>,
) -> Result<AgentChannel> {
    // `opencode acp` speaks newline-delimited JSON-RPC over its own stdio — no
    // PTY, no shim, no model_args (the model comes from its config; flockmux
    // configures the default). cwd is set both ways: process cwd + session/new.
    let mut cmd = Command::new(resolve_binary(&plugin.binary));
    cmd.arg("acp");
    cmd.env_clear();
    cmd.envs(&env);
    cmd.current_dir(workspace);
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    // Belt-and-suspenders against the orphan-CLI hole: dropping the Child kills
    // the process. Teardown also calls `kill()` explicitly via the slot.
    cmd.kill_on_drop(true);

    let mut child = cmd
        .spawn()
        .with_context(|| format!("spawn `{} acp`", plugin.binary))?;
    let stdout = child
        .stdout
        .take()
        .context("acp child has no stdout pipe")?;
    let stdin = child.stdin.take().context("acp child has no stdin pipe")?;

    // Drain stderr to the log so an opencode crash / auth error is visible
    // rather than silently swallowed.
    if let Some(stderr) = child.stderr.take() {
        let aid = agent_id.clone();
        tokio::spawn(async move {
            use tokio::io::{AsyncBufReadExt, BufReader};
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::debug!(agent = %aid, "opencode acp stderr: {line}");
            }
        });
    }

    let handles = Connection::spawn(stdout, stdin);
    let mut session = AcpSession::from_handles(handles);
    let (prompt_tx, mut prompt_rx) = mpsc::unbounded_channel::<String>();
    let child = Arc::new(Mutex::new(child));

    let cwd = workspace.to_string_lossy().into_owned();
    let driver_agent = agent_id.clone();
    let driver_swarm = swarm;
    let driver_lifecycle = lifecycle_tx;

    tokio::spawn(async move {
        // ── Handshake ──────────────────────────────────────────────────────
        if let Err(e) = session
            .initialize("flockmux", env!("CARGO_PKG_VERSION"))
            .await
        {
            tracing::warn!(agent = %driver_agent, error = %e, "acp initialize failed");
            let _ = driver_lifecycle.send(LifecycleEvent::HealthFail {
                reason: format!("ACP 初始化失败: {e}"),
                kind: "acp".into(),
            });
            return;
        }
        let session_id = match session.new_session(&cwd).await {
            Ok(id) => id,
            Err(e) => {
                tracing::warn!(agent = %driver_agent, error = %e, "acp session/new failed");
                let _ = driver_lifecycle.send(LifecycleEvent::HealthFail {
                    reason: format!("ACP 建会话失败: {e}"),
                    kind: "acp".into(),
                });
                return;
            }
        };
        tracing::info!(agent = %driver_agent, session = %session_id, "acp session ready");
        // Ready: the existing lifecycle subscriber records shim_ready + flips
        // AgentState to Ready + arms the first-response watchdog.
        let _ = driver_lifecycle.send(LifecycleEvent::ShimReady);

        // ── Turn loop ──────────────────────────────────────────────────────
        // One `session/prompt` per delivered text: the bootstrap first turn,
        // then each wake. `prompt_rx` closes when the slot (and its prompt_tx)
        // is dropped → the loop ends.
        while let Some(text) = prompt_rx.recv().await {
            driver_swarm.publish_event(SwarmEvent::AgentState {
                agent_id: driver_agent.clone(),
                state: AgentState::Thinking,
            });
            let mut seq: u32 = 0;
            // Turn-start beat: makes the agent non-silent immediately (the
            // first-response watchdog) and shows the turn began.
            emit_activity(
                &driver_swarm,
                &driver_agent,
                "running",
                "ACP turn".into(),
                seq,
                None,
                now_ms(),
            );
            let mut reply = String::new();
            let result = session
                .run_turn(&text, |u| match u {
                    AcpUpdate::ToolCall {
                        tool_call_id,
                        title,
                        ..
                    } => {
                        seq += 1;
                        let label = if title.is_empty() {
                            format!("tool {tool_call_id}")
                        } else {
                            title
                        };
                        emit_activity(
                            &driver_swarm,
                            &driver_agent,
                            "running",
                            label,
                            seq,
                            None,
                            now_ms(),
                        );
                    }
                    AcpUpdate::ToolCallUpdate {
                        tool_call_id,
                        status,
                    } => {
                        seq += 1;
                        let phase = if status == "failed" || status == "error" {
                            "error"
                        } else {
                            "ok"
                        };
                        emit_activity(
                            &driver_swarm,
                            &driver_agent,
                            phase,
                            format!("tool {tool_call_id}"),
                            seq,
                            None,
                            now_ms(),
                        );
                    }
                    AcpUpdate::AgentMessageChunk { text } => reply.push_str(&text),
                    _ => {}
                })
                .await;
            seq += 1;
            match result {
                Ok(stop) => {
                    let snippet: String = reply.trim().chars().take(80).collect();
                    let label = if snippet.is_empty() {
                        format!("turn done ({stop:?})")
                    } else {
                        snippet
                    };
                    emit_activity(
                        &driver_swarm,
                        &driver_agent,
                        "ok",
                        label,
                        seq,
                        None,
                        now_ms(),
                    );
                    driver_swarm.publish_event(SwarmEvent::AgentState {
                        agent_id: driver_agent.clone(),
                        state: AgentState::Idle,
                    });
                }
                // Peer gone: the child died / closed stdio. Stop driving; the
                // reaper will observe the dead child and publish Exited.
                Err(ConnError::Closed) => {
                    tracing::info!(agent = %driver_agent, "acp connection closed; driver exiting");
                    break;
                }
                Err(e) => {
                    tracing::warn!(agent = %driver_agent, error = %e, "acp turn failed");
                    emit_activity(
                        &driver_swarm,
                        &driver_agent,
                        "error",
                        format!("turn failed: {e}"),
                        seq,
                        None,
                        now_ms(),
                    );
                    driver_swarm.publish_event(SwarmEvent::AgentState {
                        agent_id: driver_agent.clone(),
                        state: AgentState::Idle,
                    });
                }
            }
        }
        tracing::info!(agent = %driver_agent, "acp driver loop ended");
    });

    Ok(AgentChannel::Acp { child, prompt_tx })
}
