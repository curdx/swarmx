//! REST endpoints. Loopback-only; no auth (per user decision — local single-user).

use crate::plugins::CliPlugin;
use crate::registry::LifecycleEvent;
use crate::spawn::{spawn_agent, WorkspaceLayout};
use crate::spells;
use crate::AppState;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use flockmux_protocol::rest::{
    AgentInfo, CliPluginInfo, CreateWorkspaceRequest, RunSpellAgent, RunSpellRequest,
    RunSpellResponse, SpawnAgentRequest, SpawnAgentResponse, SpawnWorkerRequest,
    SpawnWorkerResponse, SpellAgentInfo, SpellInfo, Workspace,
};
use flockmux_protocol::ws_swarm::{AgentState, SwarmEvent};
use flockmux_recorder::{Recorder, RecorderConfig};
use flockmux_storage::{NewAgent, NewRecording, NewSpellRun, NewWorker, NewWorkspace};
use serde_json::json;
use std::collections::HashMap;
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
    // Step 3: workspace_id is now mandatory. The frontend always passes
    // the active workspace's id; orphan `+ Claude` clicks must route
    // through CreateWizard first.
    let workspace_id = req.workspace_id.ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "workspace_id required (create or pick a workspace first)"})),
        )
    })?;
    let ws = state
        .store
        .get_workspace_by_id(workspace_id.clone())
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
        })?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(json!({"error": format!("workspace {workspace_id} not found")})),
            )
        })?;
    // PerAgent root defaults to the workspace's cwd so the per-agent
    // subdir lands inside the user's chosen project tree. Callers can
    // still override with `req.workspace` (e.g. tests pinning to /tmp).
    let workspace_root = req
        .workspace
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(&ws.cwd));
    // Single-agent spawn always uses per-agent subdir layout. Spells
    // are the only path that can ask for a shared workspace.
    let layout = WorkspaceLayout::PerAgent {
        root: workspace_root,
    };
    let outcome = spawn_with_bookkeeping(&state, &req.cli, req.role, layout, ws.id, None)
        .await
        .map_err(|(status, msg)| (status, Json(json!({"error": msg}))))?;
    Ok(Json(SpawnAgentResponse {
        agent_id: outcome.agent_id,
        cli: outcome.cli,
        role: outcome.role,
        workspace: outcome.workspace,
    }))
}

/// Outcome of [`spawn_with_bookkeeping`]. Carries the identity bits the
/// HTTP handler needs to build a response **and** a fresh lifecycle
/// subscription so longer-running orchestrators (spells) can await
/// `ShimReady` before injecting bootstrap input.
pub(crate) struct SpawnOutcome {
    pub agent_id: String,
    pub cli: String,
    pub role: String,
    pub workspace: String,
    pub lifecycle_rx: tokio::sync::broadcast::Receiver<LifecycleEvent>,
}

/// Shared "spawn + register + wire bookkeeping" pipeline used by both
/// POST /api/agent and the spell runner. Identical end state to the
/// previous monolithic handler — only the return path differs.
///
/// `layout` decides where on disk the agent's cwd lands:
/// - `PerAgent { root }` — historic default, agent gets its own
///   `<root>/<agent_id>/` subdir.
/// - `Shared { dir }` — every caller routed through this layout shares
///   the same cwd; used by M6a fullstack-feature spells so FE / BE /
///   Test peers see each other's commits in a single monorepo.
///
/// On success the agent is fully live: PTY pumping, registry insert
/// done, swarm inbox registered, SQLite + ws/swarm fan-out task spawned,
/// recording file open and finalize-watcher scheduled.
pub(crate) async fn spawn_with_bookkeeping(
    state: &AppState,
    cli: &str,
    role: Option<String>,
    layout: WorkspaceLayout,
    workspace_id: String,
    spell_run_id: Option<String>,
) -> Result<SpawnOutcome, (StatusCode, String)> {
    let plugin: CliPlugin = state
        .plugins
        .get(cli)
        .cloned()
        .ok_or_else(|| (StatusCode::NOT_FOUND, format!("unknown cli plugin: {cli}")))?;

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
        role,
        &layout,
        &state.shim_path,
        &state.mcp_bin,
        &state.server_url,
        recorder_handle,
    )
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

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
            workspace_id: Some(workspace_id.clone()),
            spell_run_id: spell_run_id.clone(),
        })
        .await
    {
        tracing::warn!(?e, agent = %agent_id, "record_agent_spawn failed");
    }

    drop(state.swarm.register_agent(agent_id.clone()));

    state.swarm.publish_event(SwarmEvent::AgentState {
        agent_id: agent_id.clone(),
        state: AgentState::Spawning,
    });

    // Subscribe twice: once for our own internal SQLite+swarm fan-out
    // task, and a fresh receiver for the caller (so e.g. the spell runner
    // can `await` ShimReady without racing the bookkeeping task).
    let lifecycle_rx_for_caller = result.slot.lifecycle_tx.subscribe();
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

    let outcome = SpawnOutcome {
        agent_id: agent_id.clone(),
        cli: result.slot.cli.clone(),
        role: result.slot.role.clone(),
        workspace: result.slot.workspace.clone(),
        lifecycle_rx: lifecycle_rx_for_caller,
    };
    state.registry.insert(agent_id, result.slot);
    Ok(outcome)
}

pub async fn list_agents(State(state): State<AppState>) -> impl IntoResponse {
    // Role-registry handoff lookup. Empty handoff_signal means the role
    // doesn't produce a key (orchestrator, etc.). Cloned locally so we
    // don't hold the registry borrow across async boundaries.
    let role_handoff: std::collections::HashMap<String, String> = state
        .roles
        .list()
        .into_iter()
        .map(|r| (r.manifest.id.clone(), r.manifest.handoff_signal.clone()))
        .collect();

    let handoff_for = |role: &str| -> String {
        role_handoff.get(role).cloned().unwrap_or_default()
    };
    // depends_on used to come from `wake_subs`, but that table is the
    // INTERNAL "wake me when this key lands" registration — Magentic-One's
    // append_wake_sub bug-#48 fix made the orchestrator subscribe to every
    // worker's handoff_signal, which then leaked into the DAG as a fake
    // "orchestrator depends on worker" edge. Two different concepts had
    // ended up sharing one field. Task-graph depends_on is now read
    // strictly from the `workers` row backfill below; orchestrator (not
    // in workers table) gets empty deps and renders as a DAG root, which
    // matches the user's mental model.
    let depends_for = |_agent_id: &str| -> Vec<String> { Vec::new() };

    // Build a snapshot of the live in-memory registry first — for live
    // agents the in-memory `Lifecycle` is the source of truth (it tracks
    // OSC markers that may not yet be persisted to SQLite).
    let mut live: std::collections::HashMap<String, AgentInfo> =
        std::collections::HashMap::new();
    for (id, slot) in state.registry.list() {
        let slot = slot.lock();
        let lc = *slot.lifecycle.lock();
        let depends_on = depends_for(&id);
        let handoff_signal = handoff_for(&slot.role);
        let paused = slot.paused.load(std::sync::atomic::Ordering::Relaxed);
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
                depends_on,
                handoff_signal,
                // Step 1: AgentSlot doesn't carry workspace_id yet (Step 3
                // wires that in). For live entries the SQLite row is the
                // authoritative source and we backfill from it below.
                workspace_id: None,
                spell_run_id: None,
                // parent_agent_id is derived from the spell_runs table after
                // the SQLite union below — fills in once spell_run_id is set.
                parent_agent_id: None,
                paused,
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
                    // Backfill the timestamps + workspace lineage from
                    // SQLite but keep the live lifecycle snapshot.
                    info.spawned_at = Some(row.spawned_at);
                    info.workspace_id = row.workspace_id;
                    info.spell_run_id = row.spell_run_id;
                    items.push(info);
                } else {
                    // Historical row: depends_on is empty (subscription
                    // was unregistered at kill); handoff_signal still
                    // computed from the saved role so the graph can
                    // place the node even when its wake-state is gone.
                    let handoff_signal = handoff_for(&row.role);
                    items.push(AgentInfo {
                        agent_id: row.id,
                        cli: row.cli,
                        role: row.role,
                        workspace: row.workspace,
                        shim_ready: row.shim_ready_at.is_some(),
                        shim_exit: row.shim_exit_code,
                        killed_at: row.killed_at,
                        spawned_at: Some(row.spawned_at),
                        depends_on: Vec::new(),
                        handoff_signal,
                        workspace_id: row.workspace_id,
                        spell_run_id: row.spell_run_id,
                        parent_agent_id: None,
                        paused: false,
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

    // Derive parent_agent_id from workers.parent_agent_id (Magentic-One
    // 重构后业务 agent 全部走 workers 表)。orchestrator 本身不在 workers
    // 表里 — 它是 init spell 拉的,parent 为 None(树根),符合"用户的
    // 助手"语义。一次批量 IN 查询,N+1 不允许。
    let worker_ids: Vec<String> = items.iter().map(|it| it.agent_id.clone()).collect();
    if !worker_ids.is_empty() {
        match state.store.list_workers_by_ids(worker_ids).await {
            Ok(by_id) => {
                for it in items.iter_mut() {
                    if let Some(w) = by_id.get(&it.agent_id) {
                        it.parent_agent_id = Some(w.parent_agent_id.clone());
                        // Magentic-One workers carry their handoff_signal +
                        // depends_on on the `workers` row, NOT on a role
                        // manifest (their role_label is ad-hoc, picked by
                        // the orchestrator). Backfill both so the DAG view
                        // can render the dashed waiting edges — without
                        // this, AgentInfo.handoff_signal stays empty (the
                        // role lookup above misses), depends_on stays
                        // empty (only live registry knew it), and
                        // deriveEdges falls back to "no producer" → no
                        // dashed line ever drawn.
                        if it.handoff_signal.is_empty() && !w.handoff_signal.is_empty() {
                            it.handoff_signal = w.handoff_signal.clone();
                        }
                        if it.depends_on.is_empty() && !w.depends_on_json.is_empty() {
                            if let Ok(parsed) =
                                serde_json::from_str::<Vec<String>>(&w.depends_on_json)
                            {
                                it.depends_on = parsed;
                            }
                        }
                    }
                }
            }
            Err(e) => {
                tracing::warn!(?e, "list_agents: list_workers_by_ids failed; parent edges omitted");
            }
        }
    }

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
            // M6b: tear down the wake subscription too so we don't try
            // to inject into a registry slot that's about to be dropped.
            crate::wake::unregister_wake_subs(&state.wake_subs, &agent_id).await;
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

/// M6e: operator-triggered manual wake. The UI's ⚡ button posts here
/// when the operator believes an agent has missed its natural wake or
/// is stuck. Delivery is the same mailbox + PTY-kick pair that the
/// event-driven wake uses, with a body that says "manual wake from
/// operator" so the recipient understands the context. Returns 404 if
/// the agent isn't in the registry (already exited, never spawned).
pub async fn wake_agent(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
) -> impl IntoResponse {
    if state.registry.get(&agent_id).is_none() {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("agent {agent_id} not found")})),
        );
    }
    match crate::wake::deliver_manual_wake(&state.swarm, &state.registry, &agent_id).await {
        Ok(_) => (StatusCode::NO_CONTENT, Json(json!({"ok": true}))),
        Err(e) => {
            tracing::warn!(?e, agent = %agent_id, "manual wake failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
        }
    }
}

// ────────────────────────────────────────────────────────────────────────
// Spawn ad-hoc worker (Magentic-One 重构): orchestrator 通过 MCP
// swarm_spawn_worker 拉一个临时 worker。绕过 spell + role,worker 的
// prompt / handoff_signal / depends_on 全部来自请求体,server 复用
// spawn_with_bookkeeping 完整拉 PTY,然后:
//   1. workers 表写一行(留档 + DAG parent 派生)
//   2. WakeCoordinator 注册 wake_subs + exit_keys
//   3. 后台等 ShimReady 后注入 system_prompt(跟 run_spell 同款 paste+\r)
// 跟 run_spell 的区别只是"不解析 spell / 不查 role registry / 不挂 spell_run"。
// ────────────────────────────────────────────────────────────────────────

pub async fn spawn_worker(
    State(state): State<AppState>,
    Json(req): Json<SpawnWorkerRequest>,
) -> Result<Json<SpawnWorkerResponse>, (StatusCode, Json<serde_json::Value>)> {
    if req.cli.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "missing cli"})),
        ));
    }
    if req.role_label.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "missing role_label"})),
        ));
    }
    if req.system_prompt.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "missing system_prompt"})),
        ));
    }
    if req.workspace_id.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "missing workspace_id"})),
        ));
    }
    if req.caller_agent_id.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "missing caller_agent_id"})),
        ));
    }

    // Resolve workspace cwd. spawn_worker only supports Shared layout
    // because the orchestrator-and-workers model assumes everyone works
    // in the same monorepo / project dir (跟 fullstack-feature 一致)。
    let ws = state
        .store
        .get_workspace_by_id(req.workspace_id.clone())
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("workspace lookup failed: {e}")})),
            )
        })?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(json!({"error": format!("unknown workspace_id: {}", req.workspace_id)})),
            )
        })?;
    let layout = WorkspaceLayout::Shared {
        dir: PathBuf::from(&ws.cwd),
    };

    let out = spawn_with_bookkeeping(
        &state,
        &req.cli,
        Some(req.role_label.clone()),
        layout,
        req.workspace_id.clone(),
        None, // ad-hoc workers don't belong to a spell run
    )
    .await
    .map_err(|(status, msg)| (status, Json(json!({"error": msg}))))?;

    // M6b: register wake subscription + exit-key BEFORE deferred bootstrap
    // inject (same ordering as run_spell — guards against fast producers).
    crate::wake::register_wake_subs(
        &state.wake_subs,
        out.agent_id.clone(),
        req.depends_on.clone(),
    )
    .await;
    crate::wake::register_exit_key(
        &state.exit_keys,
        out.agent_id.clone(),
        req.role_label.clone(),
        req.handoff_signal.clone(),
        now_ms(),
    )
    .await;

    // Magentic-One closes the loop here: the orchestrator (the spawning
    // agent) needs to be woken when this worker writes its handoff_signal,
    // so it can read the artifact, update Progress Ledger, and decide
    // what's next. `register_wake_subs` above already covers what the
    // *worker* waits on, but not what the *spawner* waits on. Without
    // this append the worker writes ui.done → blackboard event fires →
    // no subscriber → orchestrator sleeps forever. Append-not-overwrite
    // so the orchestrator can have many workers in flight at once.
    if !req.handoff_signal.is_empty() && !req.caller_agent_id.is_empty() {
        crate::wake::append_wake_sub(
            &state.wake_subs,
            req.caller_agent_id.clone(),
            req.handoff_signal.clone(),
        )
        .await;
    }

    // Persist worker metadata. Failure is non-fatal (PTY is already live),
    // but the DAG view will miss the parent edge until next listAgents
    // refresh after a successful retry.
    let depends_on_json =
        serde_json::to_string(&req.depends_on).unwrap_or_else(|_| "[]".to_string());
    if let Err(e) = state
        .store
        .record_worker(NewWorker {
            agent_id: out.agent_id.clone(),
            parent_agent_id: req.caller_agent_id.clone(),
            role_label: req.role_label.clone(),
            system_prompt: req.system_prompt.clone(),
            handoff_signal: req.handoff_signal.clone(),
            depends_on_json,
            spawned_at: now_ms(),
        })
        .await
    {
        tracing::warn!(?e, agent = %out.agent_id, "record_worker failed");
    }

    // Bootstrap inject — copied near-verbatim from run_spell. Same race
    // semantics (ShimReady or already_ready short-circuit), same 2500ms
    // MCP-settle window, same paste + 150ms + \r submit pattern.
    let prompt = req.system_prompt.clone();
    let agent_id = out.agent_id.clone();
    let mut rx = out.lifecycle_rx.resubscribe();
    let registry = state.registry.clone();
    let agent_for_log = agent_id.clone();
    tokio::spawn(async move {
        let already_ready = registry
            .get(&agent_id)
            .map(|s| s.lock().lifecycle.lock().shim_ready)
            .unwrap_or(false);
        if !already_ready {
            let bootstrap = async {
                loop {
                    match rx.recv().await {
                        Ok(LifecycleEvent::ShimReady) => return Ok(()),
                        Ok(LifecycleEvent::ShimExit(code)) => {
                            return Err(format!("worker exited before ShimReady (code={code})"));
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            return Err("lifecycle channel closed".into());
                        }
                    }
                }
            };
            match tokio::time::timeout(std::time::Duration::from_secs(30), bootstrap).await {
                Ok(Ok(())) => {}
                Ok(Err(msg)) => {
                    tracing::warn!(agent = %agent_for_log, msg = %msg, "worker bootstrap aborted");
                    return;
                }
                Err(_) => {
                    tracing::warn!(agent = %agent_for_log, "worker bootstrap timed out waiting for ShimReady");
                    return;
                }
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(2500)).await;
        let slot_lock = match registry.get(&agent_for_log) {
            Some(s) => s,
            None => {
                tracing::warn!(agent = %agent_for_log, "worker slot vanished before bootstrap");
                return;
            }
        };
        let input_tx = slot_lock.lock().input_tx.clone();
        let body = prompt.into_bytes();
        let body_len = body.len();
        if let Err(err) = input_tx.send(bytes::Bytes::from(body)).await {
            tracing::warn!(agent = %agent_for_log, ?err, "worker PTY paste send failed");
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        if let Err(err) = input_tx.send(bytes::Bytes::from_static(b"\r")).await {
            tracing::warn!(agent = %agent_for_log, ?err, "worker PTY submit send failed");
            return;
        }
        tracing::info!(
            agent = %agent_for_log,
            bytes = body_len,
            "worker bootstrap prompt injected"
        );
    });

    Ok(Json(SpawnWorkerResponse {
        agent_id: out.agent_id,
        cli: out.cli,
        role_label: req.role_label,
        workspace: out.workspace,
    }))
}

// ────────────────────────────────────────────────────────────────────────
// Interrupt / resume (M-pause): operator-controlled pause without tearing
// down the PTY. Cancels the in-flight turn via Ctrl-C (\x03) and flips a
// pause flag that gates the WakeCoordinator's auto-wake path. The PTY,
// blackboard subscription, mailbox, and recording all stay live. Resume
// flips the flag back and delivers one manual wake so any backlog of
// blackboard writes from the paused window gets a fresh look.
// ────────────────────────────────────────────────────────────────────────

async fn interrupt_one_inner(state: &AppState, agent_id: &str) -> Result<(), String> {
    let slot = state
        .registry
        .get(agent_id)
        .ok_or_else(|| format!("agent {agent_id} not found"))?;
    let input_tx = {
        let guard = slot.lock();
        // Set paused FIRST so any in-flight BlackboardChanged event the
        // wake coordinator is currently processing for this agent will
        // see paused=true before we even send the Ctrl-C. The Ordering
        // is Relaxed both here and at the load site — we don't need
        // cross-thread sync beyond visibility.
        guard.paused.store(true, std::sync::atomic::Ordering::Relaxed);
        guard.input_tx.clone()
    };
    // Best-effort Ctrl-C. If the PTY is already dead (shim_exit fired
    // but registry slot hasn't been removed yet) the send returns Err —
    // we keep paused=true anyway so a re-spawn-into-same-slot scenario
    // can't accidentally start auto-waking again.
    if let Err(e) = input_tx.send(bytes::Bytes::from_static(b"\x03")).await {
        tracing::warn!(?e, agent = %agent_id, "interrupt Ctrl-C send failed (PTY may be dead); paused flag still set");
    }
    Ok(())
}

/// `POST /api/agent/:id/interrupt` — cancel current turn + pause auto-wake.
pub async fn interrupt(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
) -> impl IntoResponse {
    match interrupt_one_inner(&state, &agent_id).await {
        Ok(_) => (
            StatusCode::OK,
            Json(json!({"ok": true, "agent_id": agent_id, "paused": true})),
        ),
        Err(msg) => (StatusCode::NOT_FOUND, Json(json!({"error": msg}))),
    }
}

/// `POST /api/agent/:id/resume` — clear paused flag + deliver one manual
/// wake to consume any backlog the agent missed while paused. We always
/// deliver the wake (even if the mailbox was empty) so the agent's next
/// Stop hook fire reliably triggers wake-check; this is symmetric with the
/// ⚡ button behavior in `wake_agent`.
pub async fn resume(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
) -> impl IntoResponse {
    let slot = match state.registry.get(&agent_id) {
        Some(s) => s,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": format!("agent {agent_id} not found")})),
            )
        }
    };
    slot.lock()
        .paused
        .store(false, std::sync::atomic::Ordering::Relaxed);
    if let Err(e) = crate::wake::deliver_manual_wake(&state.swarm, &state.registry, &agent_id).await
    {
        tracing::warn!(?e, agent = %agent_id, "resume: manual wake failed (paused already cleared)");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string(), "paused": false})),
        );
    }
    (
        StatusCode::OK,
        Json(json!({"ok": true, "agent_id": agent_id, "paused": false})),
    )
}

/// `POST /api/agent/interrupt-all?workspace_id=<id>` — interrupt every live
/// agent in a workspace. Live = present in the in-memory registry; killed
/// agents (SQLite-only) are ignored. `workspace_id` is matched against the
/// SQLite-stored value, which means agents spawned via legacy paths
/// without a workspace_id (pre-Step-3) won't be affected — they'd need
/// to be interrupted individually. If `workspace_id` is omitted, errors
/// (we never want to mass-interrupt the entire process).
pub async fn interrupt_all(
    State(state): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    let workspace_id = match q.get("workspace_id").map(|s| s.trim()).filter(|s| !s.is_empty()) {
        Some(w) => w.to_string(),
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "missing required query param 'workspace_id'"})),
            )
        }
    };

    // Resolve which live agents belong to this workspace. The registry
    // slot doesn't carry workspace_id yet (Step 3 of the workspace
    // rollout); we cross-reference SQLite to find matching agent ids.
    let rows = match state.store.list_agents().await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(?e, "interrupt_all: list_agents store call failed");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            );
        }
    };
    let target_ids: Vec<String> = rows
        .into_iter()
        .filter(|row| row.killed_at.is_none())
        .filter(|row| row.workspace_id.as_deref() == Some(workspace_id.as_str()))
        .map(|row| row.id)
        .collect();

    let mut interrupted: Vec<String> = Vec::new();
    let mut failed: Vec<serde_json::Value> = Vec::new();
    for id in target_ids {
        match interrupt_one_inner(&state, &id).await {
            Ok(_) => interrupted.push(id),
            Err(msg) => {
                // Agent may have exited between list_agents and now —
                // skip and report, don't abort the whole batch.
                failed.push(json!({"agent_id": id, "error": msg}));
            }
        }
    }
    (
        StatusCode::OK,
        Json(json!({
            "ok": true,
            "interrupted": interrupted.len(),
            "agent_ids": interrupted,
            "failed": failed,
        })),
    )
}

// ────────────────────────────────────────────────────────────────────────
// Workspace endpoints (Step 2 of workspace-as-first-class rollout)
// ────────────────────────────────────────────────────────────────────────

/// `POST /api/workspaces` — create a new workspace and return the
/// persisted row. CreateWizard calls this *before* launching the `init`
/// spell so the spell's spawned scout already carries `workspace_id`.
pub async fn create_workspace_handler(
    State(state): State<AppState>,
    Json(req): Json<CreateWorkspaceRequest>,
) -> Result<Json<Workspace>, (StatusCode, Json<serde_json::Value>)> {
    if req.name.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "name must be non-empty"})),
        ));
    }
    if req.cwd.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "cwd must be non-empty"})),
        ));
    }
    // Validate the cwd BEFORE persisting the row. Otherwise we'd create the
    // workspace, then the `init` spell's "create shared workspace" step fails
    // because the directory can't be entered — leaving the user with a dead,
    // 0-member ghost workspace pointing at a bad path. A 4xx here keeps the DB
    // clean and surfaces a plain "doesn't exist" message instead of a 500.
    {
        let path = std::path::Path::new(req.cwd.trim());
        if !path.exists() {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({"error": format!("directory does not exist: {}", req.cwd.trim())})),
            ));
        }
        if !path.is_dir() {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({"error": format!("not a directory: {}", req.cwd.trim())})),
            ));
        }
    }
    let rec = state
        .store
        .create_workspace(
            NewWorkspace {
                name: req.name,
                cwd: req.cwd,
                accent: req.accent,
            },
            now_ms(),
        )
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
        })?;
    Ok(Json(Workspace {
        id: rec.id,
        slug: rec.slug,
        name: rec.name,
        cwd: rec.cwd,
        accent: rec.accent,
        created_at: rec.created_at,
        member_count: 0,
    }))
}

/// `GET /api/workspaces` — list alive workspaces with their live member
/// counts (alive agents whose `workspace_id` points here). Soft-deleted
/// rows are excluded.
pub async fn list_workspaces_handler(State(state): State<AppState>) -> impl IntoResponse {
    let rows = match state.store.list_workspaces(false).await {
        Ok(rs) => rs,
        Err(e) => {
            tracing::warn!(?e, "list_workspaces failed");
            return Json(Vec::<Workspace>::new());
        }
    };
    // Compute member_count from list_agents instead of per-workspace SQL
    // queries — there are typically <100 agents total, so a single pass
    // beats N+1 SELECTs and keeps the store API smaller.
    let agents = state.store.list_agents().await.unwrap_or_default();
    let mut counts: HashMap<String, i64> = HashMap::new();
    for a in agents {
        if a.killed_at.is_some() {
            continue;
        }
        if let Some(ws_id) = a.workspace_id {
            *counts.entry(ws_id).or_insert(0) += 1;
        }
    }
    let items: Vec<Workspace> = rows
        .into_iter()
        .map(|r| Workspace {
            member_count: counts.get(&r.id).copied().unwrap_or(0),
            id: r.id,
            slug: r.slug,
            name: r.name,
            cwd: r.cwd,
            accent: r.accent,
            created_at: r.created_at,
        })
        .collect();
    Json(items)
}

/// `DELETE /api/workspaces/:id` — soft-delete a workspace. Live agents
/// in the workspace are intentionally NOT killed; the row just stops
/// showing up in `GET /api/workspaces` so the left nav loses it. Anyone
/// still attached via the WS keeps their PTY alive, by design.
pub async fn delete_workspace_handler(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.store.soft_delete_workspace(id.clone(), now_ms()).await {
        Ok(0) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("workspace {id} not found or already deleted")})),
        ),
        Ok(_) => (StatusCode::NO_CONTENT, Json(json!({"ok": true}))),
        Err(e) => {
            tracing::warn!(?e, ws_id = %id, "soft_delete_workspace failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
        }
    }
}

// ────────────────────────────────────────────────────────────────────────
// Spell endpoints
// ────────────────────────────────────────────────────────────────────────

pub async fn list_spells(State(state): State<AppState>) -> impl IntoResponse {
    // Each [[agents]] entry might be inline (role+cli given) or use
    // role_ref to defer to the RoleRegistry. We resolve here so the UI
    // dropdown shows the actually-effective role/cli rather than the
    // raw "role_ref=frontend / cli=None" the manifest carries.
    // Unresolvable refs (typo, missing role file) are returned as
    // "<role_ref>:?" so the user notices in the dropdown rather than
    // hitting a 500 at run time.
    let items: Vec<SpellInfo> = state
        .spells
        .list()
        .into_iter()
        .map(|s| SpellInfo {
            name: s.manifest.name.clone(),
            description: s.manifest.description.clone(),
            agents: s
                .manifest
                .agents
                .iter()
                .map(|a| match spells::resolve_agent(a, &state.roles) {
                    Ok(r) => SpellAgentInfo {
                        role: r.role,
                        cli: r.cli,
                    },
                    Err(_) => SpellAgentInfo {
                        role: a.effective_role().unwrap_or("?").to_string(),
                        cli: a.cli.clone().unwrap_or_else(|| "?".to_string()),
                    },
                })
                .collect(),
        })
        .collect();
    Json(items)
}

/// Run a spell: spawn all declared agents, wait for each to become ready,
/// then PTY-inject its rendered system_prompt to bootstrap the turn.
///
/// We deliberately fail-soft on per-agent bootstrap injection failures —
/// the spawn already succeeded by that point, and the user can still
/// interact with the agent manually. Returning 500 would mislead them
/// into thinking nothing was spawned, when in fact N agents are now live
/// in the registry.
pub async fn run_spell(
    State(state): State<AppState>,
    Json(req): Json<RunSpellRequest>,
) -> Result<Json<RunSpellResponse>, (StatusCode, Json<serde_json::Value>)> {
    let spell = state
        .spells
        .get(&req.name)
        .cloned()
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(json!({"error": format!("unknown spell: {}", req.name)})),
            )
        })?;

    // Resolve every `[[agents]]` entry against the role registry up-
    // front. Failing here is much friendlier than half-spawning agents
    // and then erroring out — partial spawns are visible PTYs the user
    // would have to kill manually.
    let resolved_agents: Vec<spells::ResolvedAgent> = spell
        .manifest
        .agents
        .iter()
        .map(|a| spells::resolve_agent(a, &state.roles))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": format!("spell `{}` resolve failed: {err:#}", req.name)
                })),
            )
        })?;

    // M6b: detect depends_on cycles before any spawn happens. We build
    // the cycle-detection inputs from the resolved agents themselves
    // (role → depends_on) joined with each role's `handoff_signal` (role
    // → key it produces). Roles without a handoff_signal are treated as
    // terminal — they can't be cycled back to.
    {
        let mut role_handoff: HashMap<String, String> = HashMap::new();
        let mut role_deps: HashMap<String, Vec<String>> = HashMap::new();
        for resolved in &resolved_agents {
            role_deps.insert(resolved.role.clone(), resolved.depends_on.clone());
            // The role-registry holds the canonical handoff_signal; for
            // inline-only agents (no role_ref) we leave it blank.
            if let Some(r) = state.roles.get(&resolved.role) {
                role_handoff.insert(resolved.role.clone(), r.manifest.handoff_signal.clone());
            }
        }
        // M6d-3: a spell can opt out of cycle detection if its prompts
        // explicitly bound the loop (e.g. critic↔fixer in
        // fullstack-feature-strict, capped at 3 rounds by the fixer's
        // round counter). Default behaviour stays: reject cycles.
        let skip_cycle_check = spell.manifest.allow_cycles;
        let cycle_result = if skip_cycle_check {
            tracing::info!(
                spell = %req.name,
                "skipping depends_on cycle check; spell declared allow_cycles=true"
            );
            Ok(())
        } else {
            crate::wake::detect_depends_on_cycles(&role_handoff, &role_deps)
        };
        if let Err(err) = cycle_result {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": format!("spell `{}` has depends_on cycle: {err:#}", req.name)
                })),
            ));
        }
    }

    // Step 3: resolve the effective workspace_id BEFORE picking the
    // layout. Order of precedence:
    //   1. `caller_agent_id` — set when MCP `swarm_run_spell` fires from
    //      inside an existing agent (planner / scout). Reverse-resolves
    //      to the caller's workspace_id so spell agents inherit it.
    //   2. `workspace_id` — set when the UI's launcher / CreateWizard
    //      sends a spell directly.
    //   3. else → 400. This is the core fix for the "orphan workspace
    //      tab" bug: a spell launch with no workspace context is now an
    //      error instead of silently creating an unowned spawn.
    let workspace_id: String = if let Some(caller) = req.caller_agent_id.as_ref() {
        match state
            .store
            .get_workspace_id_for_agent(caller.clone())
            .await
        {
            Ok(Some(ws_id)) => ws_id,
            Ok(None) => {
                return Err((
                    StatusCode::BAD_REQUEST,
                    Json(json!({
                        "error": format!(
                            "caller agent `{caller}` has no workspace_id; pass workspace_id explicitly"
                        )
                    })),
                ));
            }
            Err(e) => {
                return Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error": e.to_string()})),
                ));
            }
        }
    } else if let Some(ws_id) = req.workspace_id.clone() {
        ws_id
    } else {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": "spell requires workspace context: pass workspace_id or caller_agent_id"
            })),
        ));
    };
    // Look up the workspace row so we can default Shared layout cwd to
    // its `cwd` when the client didn't override.
    let workspace = state
        .store
        .get_workspace_by_id(workspace_id.clone())
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
        })?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(json!({"error": format!("workspace {workspace_id} not found")})),
            )
        })?;

    // Pick the workspace layout. For shared_workspace spells we use the
    // explicit `workspace_dir` if the client sent one (M6a UX: the
    // SpellsLauncher exposes a text input); otherwise default to the
    // workspace's `cwd` so the spell runs in the project the user picked
    // in CreateWizard. PerAgent spells get per-agent subdirs under
    // `workspaces_root` as before — cwd and workspace_id are orthogonal
    // (filesystem layer vs. UI grouping layer).
    let layout: WorkspaceLayout = if spell.manifest.shared_workspace {
        let dir = req
            .workspace_dir
            .as_deref()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(&workspace.cwd));
        WorkspaceLayout::Shared { dir }
    } else {
        WorkspaceLayout::PerAgent {
            root: state.workspaces_root.clone(),
        }
    };

    // Record the spell-run lineage row so future UI can group agents by
    // "this is the third critic-loop run in critic-demo". Lifetime is
    // tied to the workspace via FK; soft-deleting the workspace doesn't
    // touch this row.
    let spell_run = state
        .store
        .create_spell_run(NewSpellRun {
            workspace_id: workspace.id.clone(),
            spell_name: spell.manifest.name.clone(),
            task: req.task.clone(),
            caller_agent_id: req.caller_agent_id.clone(),
            started_at: now_ms(),
        })
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
        })?;
    let spell_run_id = spell_run.id.clone();

    // Phase 1: spawn all agents up-front (no PTY input yet) so each one's
    // agent_id is known before we render any prompt. Otherwise the writer's
    // prompt couldn't reference critic's id.
    let mut outcomes: Vec<(SpawnOutcome, String)> =
        Vec::with_capacity(resolved_agents.len());
    for resolved in &resolved_agents {
        let out = spawn_with_bookkeeping(
            &state,
            &resolved.cli,
            Some(resolved.role.clone()),
            layout.clone(),
            workspace.id.clone(),
            Some(spell_run_id.clone()),
        )
        .await
        .map_err(|(status, msg)| {
            (
                status,
                Json(json!({
                    "error": format!("spell `{}` failed at agent `{}`: {}", req.name, resolved.role, msg)
                })),
            )
        })?;
        // M6b: register the wake subscription IMMEDIATELY after spawn,
        // before Phase 2 kicks off the deferred bootstrap-inject task.
        // This guards against the race where a producer agent (e.g. BE)
        // is unbelievably fast and writes its handoff key before we get
        // around to subscribing this agent's deps.
        crate::wake::register_wake_subs(
            &state.wake_subs,
            out.agent_id.clone(),
            resolved.depends_on.clone(),
        )
        .await;
        // M6c step 5: also remember which signal THIS agent is supposed
        // to produce + the moment we registered it. If the agent exits
        // without writing the signal, the wake coordinator turns that
        // exit into a `<signal>.error` so the downstream dependents
        // stop hanging. The spawn time is used to disambiguate "fresh
        // write from this run's agent" vs "stale leftover from a
        // previous run on the same blackboard". Empty signal (inline
        // role, planner) → register_exit_key is a no-op.
        let handoff_signal = state
            .roles
            .get(&resolved.role)
            .map(|r| r.manifest.handoff_signal.clone())
            .unwrap_or_default();
        crate::wake::register_exit_key(
            &state.exit_keys,
            out.agent_id.clone(),
            resolved.role.clone(),
            handoff_signal,
            now_ms(),
        )
        .await;
        outcomes.push((out, resolved.system_prompt.clone()));
    }

    // Build role → agent_id map for {<role>_id} substitution.
    let role_to_id: HashMap<String, String> = outcomes
        .iter()
        .map(|(o, _)| (o.role.clone(), o.agent_id.clone()))
        .collect();

    let spell_name = spell.manifest.name.clone();
    // Phase 2: for each agent, wait until shim_ready then inject the
    // rendered prompt. Spawn this off into background tasks so the HTTP
    // response returns promptly — the user wants to see the agents pop
    // up in the UI, not wait 5+ seconds for all bootstraps to land.
    for (out, raw_prompt) in outcomes.iter() {
        if raw_prompt.trim().is_empty() {
            continue;
        }
        let prompt = spells::render_prompt(raw_prompt, &req.task, &workspace_id, &role_to_id);
        let agent_id = out.agent_id.clone();
        let mut rx = out.lifecycle_rx.resubscribe();
        let registry = state.registry.clone();
        let spell_name_for_task = spell_name.clone();
        let role_to_id_for_task = role_to_id.clone();
        tokio::spawn(async move {
            // Check first whether ShimReady already fired in the gap
            // between spawn_agent returning and our subscribe — the PTY
            // pump task is concurrent with the spawn caller, so for fast
            // CLIs (or warm filesystem caches) OSC_READY can arrive
            // BEFORE we have a receiver hooked up. Reading the mutex
            // covers that race; if shim_ready is already true we bypass
            // the broadcast wait entirely.
            let already_ready = registry
                .get(&agent_id)
                .map(|s| s.lock().lifecycle.lock().shim_ready)
                .unwrap_or(false);
            if !already_ready {
                let bootstrap = async {
                    loop {
                        match rx.recv().await {
                            Ok(LifecycleEvent::ShimReady) => return Ok(()),
                            Ok(LifecycleEvent::ShimExit(code)) => {
                                return Err(format!(
                                    "agent exited before ShimReady (code={code})"
                                ));
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                                return Err("lifecycle channel closed".into());
                            }
                        }
                    }
                };
                match tokio::time::timeout(std::time::Duration::from_secs(30), bootstrap).await {
                    Ok(Ok(())) => {}
                    Ok(Err(msg)) => {
                        tracing::warn!(spell = %spell_name_for_task, agent = %agent_id, msg = %msg, "spell bootstrap aborted");
                        return;
                    }
                    Err(_) => {
                        tracing::warn!(spell = %spell_name_for_task, agent = %agent_id, "spell bootstrap timed out waiting for ShimReady");
                        return;
                    }
                }
            }
            // Grace period before injecting the bootstrap prompt. Two
            // distinct waits are stacked here:
            //
            // - Input-stack settle (XtermPane has the same logic): codex's
            //   ratatui crossterm poll attaches just after OSC_READY and
            //   ~300ms covers the race.
            // - MCP server connect: claude/codex spawn MCP subprocesses
            //   AFTER their UI shows ready, taking 1–3 s. If we fire the
            //   prompt before flockmux-swarm finishes the handshake, the
            //   agent reads its toolset, sees no swarm tools, and hand-
            //   waves with "I don't have a swarm_send_message tool" —
            //   exactly what we observed in practice. Empirically the
            //   "MCP Wait for Servers" banner in claude clears at ~2 s
            //   on this machine; 2500 ms gives breathing room while still
            //   keeping bootstrap under 3 s end-to-end.
            tokio::time::sleep(std::time::Duration::from_millis(2500)).await;
            let slot_lock = match registry.get(&agent_id) {
                Some(s) => s,
                None => {
                    tracing::warn!(spell = %spell_name_for_task, agent = %agent_id, "agent slot vanished before bootstrap");
                    return;
                }
            };
            let input_tx = slot_lock.lock().input_tx.clone();
            // Submission strategy: send the prompt body and the Enter
            // keystroke as TWO separate frames with a delay between.
            //
            // Why: claude/codex TUIs heuristically classify a burst of
            // bytes containing newlines as a *paste* (claude renders
            // "[Pasted text #N +M lines]" placeholder). If \r is part of
            // the same burst, it becomes a paste-newline (a literal line
            // break within the message body) rather than a submit. By
            // splitting and letting the TUI settle the paste first, the
            // standalone \r reads as the Enter key the way the user
            // would have pressed it.
            //
            // The 150ms gap is empirical: shorter (~50ms) sometimes
            // missed the boundary on cold-start codex; longer hurts
            // user-perceived bootstrap time without measurable benefit.
            let body = prompt.into_bytes();
            let body_len = body.len();
            let lossy = String::from_utf8_lossy(&body);
            // Diagnostic: log when a `{task}` or `{<role>_id}` slot
            // survived rendering. Dynamic over the spell's declared
            // roles so this fires for any spell, not just critic-loop.
            let has_unsubst = lossy.contains("{task}")
                || role_to_id_for_task
                    .keys()
                    .any(|r| lossy.contains(&format!("{{{r}_id}}")));
            if let Err(err) = input_tx.send(bytes::Bytes::from(body)).await {
                tracing::warn!(spell = %spell_name_for_task, agent = %agent_id, ?err, "PTY paste send failed during spell bootstrap");
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            if let Err(err) = input_tx.send(bytes::Bytes::from_static(b"\r")).await {
                tracing::warn!(spell = %spell_name_for_task, agent = %agent_id, ?err, "PTY submit send failed during spell bootstrap");
                return;
            }
            tracing::info!(
                spell = %spell_name_for_task,
                agent = %agent_id,
                bytes = body_len,
                has_unsubstituted_placeholders = has_unsubst,
                "spell bootstrap prompt injected"
            );
        });
    }

    let resp = RunSpellResponse {
        spell: req.name,
        agents: outcomes
            .into_iter()
            .map(|(o, _)| RunSpellAgent {
                role: o.role,
                cli: o.cli,
                agent_id: o.agent_id,
            })
            .collect(),
    };
    Ok(Json(resp))
}

