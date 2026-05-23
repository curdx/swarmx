//! REST endpoints. Loopback-only; no auth (per user decision — local single-user).

use crate::plugins::CliPlugin;
use crate::registry::LifecycleEvent;
use crate::spawn::{spawn_agent, WorkspaceLayout};
use crate::spells;
use crate::AppState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use flockmux_protocol::rest::{
    AgentInfo, CliPluginInfo, RunSpellAgent, RunSpellRequest, RunSpellResponse, SpawnAgentRequest,
    SpawnAgentResponse, SpellAgentInfo, SpellInfo,
};
use flockmux_protocol::ws_swarm::{AgentState, SwarmEvent};
use flockmux_recorder::{Recorder, RecorderConfig};
use flockmux_storage::{NewAgent, NewRecording};
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
    let workspace_root = req
        .workspace
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or_else(|| state.workspaces_root.clone());
    // Single-agent spawn always uses per-agent subdir layout. Spells
    // are the only path that can ask for a shared workspace.
    let layout = WorkspaceLayout::PerAgent {
        root: workspace_root,
    };
    let outcome = spawn_with_bookkeeping(&state, &req.cli, req.role, layout)
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
    // Snapshot the wake-sub map ONCE before iterating agents so a churn
    // of register/unregister calls mid-listing doesn't make some agents
    // show their deps while others don't. The read lock is released
    // before we await on the store.
    let depends_snapshot: std::collections::HashMap<String, Vec<String>> = {
        let map = state.wake_subs.read().await;
        map.clone()
    };
    // Same idea for the role registry: clone the (role → handoff_signal)
    // mapping locally so we don't keep the registry borrow open through
    // async boundaries. Empty handoff_signal means the role doesn't
    // produce a key (planner, writer/critic/editor in critic-loop).
    let role_handoff: std::collections::HashMap<String, String> = state
        .roles
        .list()
        .into_iter()
        .map(|r| (r.manifest.id.clone(), r.manifest.handoff_signal.clone()))
        .collect();

    let handoff_for = |role: &str| -> String {
        role_handoff.get(role).cloned().unwrap_or_default()
    };
    let depends_for = |agent_id: &str| -> Vec<String> {
        depends_snapshot.get(agent_id).cloned().unwrap_or_default()
    };

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
            // M6b: tear down the wake subscription too so we don't try
            // to inject into a registry slot that's about to be dropped.
            crate::wake::unregister_wake_subs(&state.wake_subs, &agent_id).await;
            // M6d-5: matching TTL row cleanup — the waiter is gone, no
            // point ageing a subscription against a dead agent. The
            // run-loop also handles this on AgentState::Exited, but
            // doing it here means the kill route's response is fully
            // consistent before the broadcast fans out.
            crate::wake::unregister_wake_started_at(&state.wake_started_at, &agent_id).await;
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

    // Pick the workspace layout. For shared_workspace spells we use the
    // explicit `workspace_dir` if the client sent one (M6a UX: the
    // SpellsLauncher exposes a text input); otherwise we mint a fresh
    // `<workspaces_root>/spell-<uuid>/` so the launch never silently
    // falls back to per-agent isolation (which would defeat the spell's
    // entire premise — see fullstack-feature.md, FE/BE need to see each
    // other's commits).
    let layout: WorkspaceLayout = if spell.manifest.shared_workspace {
        let dir = req
            .workspace_dir
            .as_deref()
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                state
                    .workspaces_root
                    .join(format!("spell-{}", &Uuid::new_v4().to_string()[..8]))
            });
        WorkspaceLayout::Shared { dir }
    } else {
        WorkspaceLayout::PerAgent {
            root: state.workspaces_root.clone(),
        }
    };

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
        // M6d-5: matching TTL bookkeeping. Stamps each (waiter, key)
        // pair with the moment of registration so the WakeCoordinator's
        // periodic scanner can age them out and nudge stuck producers.
        // Single now_ms() snapshot so all keys in one registration share
        // a deadline — keeps the eventual alert messages consistent.
        let registered_at = now_ms();
        crate::wake::register_wake_started_at(
            &state.wake_started_at,
            &out.agent_id,
            &resolved.depends_on,
            registered_at,
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
        let prompt = spells::render_prompt(raw_prompt, &req.task, &role_to_id);
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

