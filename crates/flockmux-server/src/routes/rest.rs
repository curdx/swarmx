//! REST endpoints. Loopback-only; no auth (per user decision — local single-user).

use crate::plugins::CliPlugin;
use crate::registry::LifecycleEvent;
use crate::spawn::spawn_agent;
use crate::AppState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use flockmux_protocol::rest::{AgentInfo, CliPluginInfo, SpawnAgentRequest, SpawnAgentResponse};
use flockmux_protocol::ws_swarm::{AgentState, SwarmEvent};
use flockmux_recorder::{Recorder, RecorderConfig};
use flockmux_storage::{NewAgent, NewRecording};
use serde_json::json;
use std::path::PathBuf;
use uuid::Uuid;

fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub async fn list_plugins(State(state): State<AppState>) -> impl IntoResponse {
    let plugins: Vec<CliPluginInfo> = state
        .plugins
        .list()
        .into_iter()
        .map(|p| CliPluginInfo {
            id: p.id.clone(),
            display_name: p.display_name.clone(),
            binary: p.binary.clone(),
        })
        .collect();
    Json(plugins)
}

pub async fn spawn(
    State(state): State<AppState>,
    Json(req): Json<SpawnAgentRequest>,
) -> Result<Json<SpawnAgentResponse>, (StatusCode, Json<serde_json::Value>)> {
    let plugin: CliPlugin = state
        .plugins
        .get(&req.cli)
        .cloned()
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(json!({"error": format!("unknown cli plugin: {}", req.cli)})),
            )
        })?;

    let workspace_root = req
        .workspace
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or_else(|| state.workspaces_root.clone());

    let spawned_at = now_ms();

    // Mint the recording id + path up front so spawn_agent can hand the
    // pump a writer handle. If the recorder fails to open, we still spawn
    // the agent (recording is best-effort, not load-bearing for M3).
    let recording_id = format!("rec-{}", &Uuid::new_v4().to_string()[..12]);
    let recording_path = state
        .recordings_root
        .join(format!("{}.cast", recording_id));
    let recorder = match Recorder::start(RecorderConfig {
        agent_id: String::new(), // filled in by the writer config; informational only
        cols: 120,
        rows: 32,
        started_at_ms: spawned_at,
        file_path: recording_path.clone(),
    })
    .await
    {
        Ok(r) => Some(r),
        Err(e) => {
            tracing::warn!(?e, "recorder open failed; spawning agent without recording");
            None
        }
    };
    let recorder_handle = recorder.as_ref().map(|r| r.handle());

    let result = spawn_agent(
        &plugin,
        req.role,
        &workspace_root,
        &state.shim_path,
        &state.mcp_bin,
        &state.server_url,
        recorder_handle,
    )
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
    })?;

    let agent_id = result.agent_id.clone();

    // Persist the spawn. PTY is already live; on store failure we log and
    // keep the in-memory agent — the user can still attach and use it.
    if let Err(e) = state
        .store
        .record_agent_spawn(NewAgent {
            id: agent_id.clone(),
            cli: result.slot.cli.clone(),
            role: result.slot.role.clone(),
            workspace: result.slot.workspace.clone(),
            spawned_at,
        })
        .await
    {
        tracing::warn!(?e, agent = %agent_id, "record_agent_spawn failed");
    }

    // Reserve an inbox slot and immediately drop the receiver: M3 doesn't
    // route swarm messages into PTY stdin (claude/codex are fullscreen
    // alt-screen TUIs). Delivery is SQLite + ws/swarm only; M4 MCP will
    // plug a real consumer in here.
    drop(state.swarm.register_agent(agent_id.clone()));

    state.swarm.publish_event(SwarmEvent::AgentState {
        agent_id: agent_id.clone(),
        state: AgentState::Spawning,
    });

    // Fan ShimReady / ShimExit into SQLite + ws/swarm. The task exits when
    // every sender on `lifecycle_tx` is dropped (i.e. after the slot is
    // removed and the PTY pump finishes).
    {
        let mut lifecycle_rx = result.slot.lifecycle_tx.subscribe();
        let store = state.store.clone();
        let swarm = state.swarm.clone();
        let agent_for_task = agent_id.clone();
        tokio::spawn(async move {
            loop {
                match lifecycle_rx.recv().await {
                    Ok(LifecycleEvent::ShimReady) => {
                        let at = now_ms();
                        if let Err(e) =
                            store.record_shim_ready(agent_for_task.clone(), at).await
                        {
                            tracing::warn!(?e, agent = %agent_for_task, "record_shim_ready failed");
                        }
                        swarm.publish_event(SwarmEvent::AgentState {
                            agent_id: agent_for_task.clone(),
                            state: AgentState::Ready,
                        });
                    }
                    Ok(LifecycleEvent::ShimExit(code)) => {
                        let at = now_ms();
                        if let Err(e) = store
                            .record_shim_exit(agent_for_task.clone(), code, at)
                            .await
                        {
                            tracing::warn!(?e, agent = %agent_for_task, "record_shim_exit failed");
                        }
                        swarm.publish_event(SwarmEvent::AgentState {
                            agent_id: agent_for_task.clone(),
                            state: AgentState::Exited,
                        });
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(agent = %agent_for_task, lagged = n, "lifecycle subscriber lagged");
                        continue;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
            tracing::debug!(agent = %agent_for_task, "lifecycle subscriber closed");
        });
    }

    // Persist the recording-start + spawn a background task that awaits
    // EOF and persists the finalize. Recording is best-effort: if the
    // recorder failed to open earlier, both halves are skipped.
    if let Some(rec) = recorder {
        let new_rec = NewRecording {
            id: recording_id.clone(),
            agent_id: agent_id.clone(),
            path: recording_path.to_string_lossy().into_owned(),
            started_at: spawned_at,
            cols: 120,
            rows: 32,
        };
        if let Err(e) = state.store.record_recording_start(new_rec).await {
            tracing::warn!(?e, agent = %agent_id, "record_recording_start failed");
        }
        let store = state.store.clone();
        let rec_id_for_task = recording_id.clone();
        let agent_for_task = agent_id.clone();
        tokio::spawn(async move {
            match rec.wait_finalize().await {
                Ok(fin) => {
                    if let Err(e) = store
                        .record_recording_finalize(
                            rec_id_for_task,
                            fin.finalized_at_ms,
                            fin.duration_ms,
                            fin.last_seq,
                        )
                        .await
                    {
                        tracing::warn!(?e, agent = %agent_for_task, "record_recording_finalize failed");
                    }
                }
                Err(e) => {
                    tracing::warn!(?e, agent = %agent_for_task, "recorder wait_finalize failed");
                }
            }
        });
    }

    let resp = SpawnAgentResponse {
        agent_id: agent_id.clone(),
        cli: result.slot.cli.clone(),
        role: result.slot.role.clone(),
        workspace: result.slot.workspace.clone(),
    };
    state.registry.insert(agent_id, result.slot);
    Ok(Json(resp))
}

pub async fn list_agents(State(state): State<AppState>) -> impl IntoResponse {
    // Build a snapshot of the live in-memory registry first — for live
    // agents the in-memory `Lifecycle` is the source of truth (it tracks
    // OSC markers that may not yet be persisted to SQLite).
    let mut live: std::collections::HashMap<String, AgentInfo> =
        std::collections::HashMap::new();
    for (id, slot) in state.registry.list() {
        let slot = slot.lock();
        let lc = *slot.lifecycle.lock();
        live.insert(
            id.clone(),
            AgentInfo {
                agent_id: id,
                cli: slot.cli.clone(),
                role: slot.role.clone(),
                workspace: slot.workspace.clone(),
                shim_ready: lc.shim_ready,
                shim_exit: lc.shim_exit,
                killed_at: None,
                spawned_at: None,
            },
        );
    }

    // Union with SQLite history so a freshly-started server can still tell
    // the UI about agents from prior runs. Live entries win — they keep
    // their `shim_ready`/`shim_exit` derived from the in-memory lifecycle.
    let mut items: Vec<AgentInfo> = Vec::new();
    match state.store.list_agents().await {
        Ok(rows) => {
            for row in rows {
                if let Some(mut info) = live.remove(&row.id) {
                    // Backfill the timestamps from SQLite but keep the
                    // live lifecycle snapshot.
                    info.spawned_at = Some(row.spawned_at);
                    items.push(info);
                } else {
                    items.push(AgentInfo {
                        agent_id: row.id,
                        cli: row.cli,
                        role: row.role,
                        workspace: row.workspace,
                        shim_ready: row.shim_ready_at.is_some(),
                        shim_exit: row.shim_exit_code,
                        killed_at: row.killed_at,
                        spawned_at: Some(row.spawned_at),
                    });
                }
            }
        }
        Err(e) => {
            tracing::warn!(?e, "list_agents: store.list_agents failed; live-only view");
        }
    }
    // Any live entries that weren't in the store (shouldn't happen, but
    // be defensive) get appended at the end.
    items.extend(live.into_values());
    Json(items)
}

pub async fn kill(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
) -> impl IntoResponse {
    match state.registry.remove(&agent_id) {
        Some(slot) => {
            {
                let slot = slot.lock();
                slot.bridge.kill();
            }
            // Drop the in-memory inbox before persisting the kill so any
            // in-flight send_message sees "no inbox" rather than racing
            // against a half-torn-down agent.
            state.swarm.unregister_agent(&agent_id);
            if let Err(e) = state
                .store
                .record_agent_kill(agent_id.clone(), now_ms())
                .await
            {
                tracing::warn!(?e, agent = %agent_id, "record_agent_kill failed");
            }
            state.swarm.publish_event(SwarmEvent::AgentState {
                agent_id: agent_id.clone(),
                state: AgentState::Exited,
            });
            (StatusCode::NO_CONTENT, Json(json!({"ok": true})))
        }
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("agent {agent_id} not found")})),
        ),
    }
}
