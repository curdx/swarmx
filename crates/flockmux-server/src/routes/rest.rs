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
    AgentInfo, CliPluginInfo, CreateThreadRequest, CreateWorkspaceRequest, RunSpellAgent,
    RunSpellRequest, RunSpellResponse, SpawnAgentRequest, SpawnAgentResponse, SpawnWorkerRequest,
    SpawnWorkerResponse, SpellAgentInfo, SpellInfo, ThreadInfo, UpdateThreadRequest, Workspace,
    WorkspaceRoot,
};
use flockmux_protocol::ws_swarm::{AgentState, SwarmEvent};
use flockmux_recorder::{Recorder, RecorderConfig};
use flockmux_storage::{
    NewAgent, NewRecording, NewSpellRun, NewThread, NewWorker, NewWorkspace, NewWorkspaceRoot,
    ThreadRecord,
};
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

/// Closest candidate to `target` within edit distance 3 — drives the
/// "did you mean 'frontend'?" hint when an unknown role slug is spawned.
fn closest_match(target: &str, candidates: &[String]) -> Option<String> {
    candidates
        .iter()
        .map(|c| (levenshtein(target, c), c))
        .filter(|(d, _)| *d <= 3)
        .min_by_key(|(d, _)| *d)
        .map(|(_, c)| c.clone())
}

fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = if ca == cb { 0 } else { 1 };
            cur[j + 1] = (prev[j + 1] + 1).min(cur[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// First dependency not yet satisfied — neither the key itself NOR its
/// `.error`/`.failed` failure alias is present on the blackboard — or `None` if
/// all are satisfied. Pure (unit-tested); drives the P1-D readiness gate's
/// "are this worker's inputs ready?" decision. A `.error`/`.failed` alias counts
/// as satisfied so a downstream worker wakes to handle an upstream FAILURE
/// rather than waiting forever for a key the dead producer will never write.
fn first_unsatisfied_dep(
    deps: &[String],
    present: &std::collections::HashSet<String>,
) -> Option<String> {
    deps.iter()
        .find(|k| {
            !present.contains(k.as_str())
                && !present.contains(format!("{k}.error").as_str())
                && !present.contains(format!("{k}.failed").as_str())
        })
        .cloned()
}

/// Spawn-time dependency-graph validation + key minting (P0-A), pure so it can
/// be unit-tested without an HTTP server. Resolves each typed `consumes` ref to
/// the producer's minted blackboard key, after verifying the producer role
/// exists and declares the requested output kind. Returns the minted
/// `depends_on` keys, or a human error (mapped to 400 by the caller).
fn resolve_consumes_to_deps(
    registry: &crate::roles::RoleRegistry,
    role_slug: &str,
    consumes: &[flockmux_protocol::rest::ConsumeRef],
    workspace_id: &str,
    thread_slug: &str,
) -> Result<Vec<String>, String> {
    let mut depends_on = Vec::with_capacity(consumes.len());
    for c in consumes {
        let from = c.from_role.trim();
        let kind = c.kind.trim();
        if from.is_empty() {
            return Err("consumes entry has empty from_role".to_string());
        }
        if from == role_slug {
            return Err(format!(
                "role '{role_slug}' cannot consume its own output (self-dependency)"
            ));
        }
        let producer = registry.get(from).ok_or_else(|| {
            let valid = registry.ids();
            let mut msg = format!("consumes references unknown role '{from}'");
            if let Some(s) = closest_match(from, &valid) {
                msg.push_str(&format!(" — did you mean '{s}'?"));
            }
            msg.push_str(&format!(" valid roles: {valid:?}"));
            msg
        })?;
        let producer_kinds: Vec<String> = if producer.manifest.produces.is_empty() {
            vec!["done".to_string()]
        } else {
            producer.manifest.produces.clone()
        };
        if !producer_kinds.iter().any(|k| k == kind) {
            return Err(format!(
                "role '{from}' does not produce kind '{kind}' — it produces {producer_kinds:?}"
            ));
        }
        depends_on.push(crate::roles::mint_handoff_key(
            workspace_id,
            thread_slug,
            from,
            kind,
        ));
    }
    Ok(depends_on)
}

/// Append the server-minted handoff key(s) to the orchestrator-authored worker
/// prompt, so the worker writes the canonical key verbatim instead of inventing
/// one — the F3 drift class is designed away (P0-A).
fn build_worker_prompt(
    base: &str,
    success_keys: &[String],
    error_key: &str,
    dep_keys: &[String],
) -> String {
    let mut out = base.to_string();

    // INPUTS / wait-gate: a worker is bootstrapped immediately on spawn, but its
    // typed dependencies may not be on the blackboard yet. Without this block it
    // would act prematurely (and, worse, write its handoff key → auto-killed
    // before the real work). Tell it to check its inputs first and STOP (without
    // writing handoff) if any are missing; the WakeCoordinator re-wakes it when
    // they land. A `<key>.error` means the upstream failed — handle, don't hang.
    if !dep_keys.is_empty() {
        let deps = dep_keys
            .iter()
            .map(|k| format!("  - {k}"))
            .collect::<Vec<_>>()
            .join("\n");
        out.push_str(&format!(
            "\n\n──────────────────────────────────────────────────────────────\n\
             INPUTS — this task depends on other agents' output. BEFORE doing \
             anything, use swarm_list_blackboard / swarm_read_blackboard to check \
             for ALL of these keys:\n{deps}\n\
             If ANY key is missing: do NOT start the task and do NOT write your \
             handoff key. Reply in one line that you are waiting for inputs, then \
             STOP — flockmux re-wakes you automatically the moment they appear. \
             A `<key>.error` (instead of the key) means that upstream FAILED: \
             handle the failure path, do not wait forever. Only proceed with the \
             task once EVERY input key is present.\n"
        ));
    }

    // HANDOFF: the server-minted keys this worker writes. Copy verbatim.
    if !success_keys.is_empty() {
        let keys = success_keys
            .iter()
            .map(|k| format!("  - {k}"))
            .collect::<Vec<_>>()
            .join("\n");
        out.push_str(&format!(
            "\n──────────────────────────────────────────────────────────────\n\
             HANDOFF (managed by flockmux — copy these keys VERBATIM via \
             swarm_write_blackboard; do NOT invent or alter them):\n\
             • On SUCCESS (only when the task is actually done), write your result to:\n{keys}\n\
             • On FAILURE/abort, write to `{error_key}` instead, so dependents \
             fail loudly rather than hang forever.\n"
        ));
    }

    out
}

/// Map a storage `ThreadRecord` onto the wire `ThreadInfo` (drops the internal
/// `deleted_at`; the API only ever surfaces alive threads).
fn thread_record_to_info(t: ThreadRecord) -> ThreadInfo {
    ThreadInfo {
        id: t.id,
        workspace_id: t.workspace_id,
        slug: t.slug,
        name: t.name,
        isolation: t.isolation,
        branch: t.branch,
        cwd: t.cwd,
        state: t.state,
        created_at: t.created_at,
    }
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
            input_settle_ms: p.input_settle_ms,
            default_model: p.default_model.clone(),
        })
        .collect();
    Json(plugins)
}

/// `POST /api/agent/:id/mcp-ready` — called by the agent's own `flockmux-mcp`
/// subprocess once the CLI has fetched its tool list (per the MCP lifecycle,
/// that's the moment the `swarm_*` tools become visible to the model). Flips
/// the slot's `mcp_ready` gate so the deferred bootstrap can inject the prompt
/// immediately instead of waiting out a fixed timeout — the readiness-probe
/// pattern, replacing the old fixed 2500ms MCP-settle sleep. Idempotent;
/// 404 if the agent isn't (or is no longer) live.
pub async fn mcp_ready(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
) -> StatusCode {
    match state.registry.get(&agent_id) {
        Some(slot) => {
            slot.lock().mcp_ready.send_replace(true);
            tracing::debug!(agent = %agent_id, "mcp-ready signalled by flockmux-mcp");
            StatusCode::NO_CONTENT
        }
        None => StatusCode::NOT_FOUND,
    }
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
    // Trailing args: spell_run_id=None (standalone), thread_id=None (a single
    // ad-hoc agent lands on the workspace's main thread).
    let outcome =
        spawn_with_bookkeeping(&state, &req.cli, req.role, req.model, layout, ws.id, None, None)
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
/// Hard ceiling on concurrent **live** agents — the fork-bomb guard (F4).
/// Counts the in-memory registry (killed agents are removed from it), so it's
/// a true concurrency bound. Env override `FLOCKMUX_MAX_LIVE_AGENTS` for
/// bigger hosts; 0 / unparseable falls back to the default.
fn max_live_agents() -> usize {
    std::env::var("FLOCKMUX_MAX_LIVE_AGENTS")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(256)
}

/// Max delegation depth (orchestrator → worker → worker → …). Env override
/// `FLOCKMUX_MAX_SPAWN_DEPTH`; 0 / unparseable falls back to the default.
fn max_spawn_depth() -> usize {
    std::env::var("FLOCKMUX_MAX_SPAWN_DEPTH")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(6)
}

pub(crate) async fn spawn_with_bookkeeping(
    state: &AppState,
    cli: &str,
    role: Option<String>,
    model: Option<String>,
    layout: WorkspaceLayout,
    workspace_id: String,
    spell_run_id: Option<String>,
    thread_id: Option<String>,
) -> Result<SpawnOutcome, (StatusCode, String)> {
    // Fork-bomb guard (F4): a runaway/looping orchestrator — or a worker it
    // spawned — calling swarm_spawn_worker can otherwise fork unbounded real
    // CLI processes (each launched with --dangerously-skip-permissions),
    // exhausting PTYs / RAM / file descriptors and burning API budget. Cap the
    // TOTAL live agents here, the single chokepoint shared by /api/agent,
    // /api/worker and run_spell, so every spawn path is bounded. The auto-kill
    // reaper keeps well-behaved swarms far below this.
    let live = state.registry.list().len();
    let cap = max_live_agents();
    if live >= cap {
        return Err((
            StatusCode::TOO_MANY_REQUESTS,
            format!(
                "live-agent cap reached ({live}/{cap}); refusing to spawn. \
                 Finish or kill an agent first, or raise FLOCKMUX_MAX_LIVE_AGENTS."
            ),
        ));
    }

    let plugin: CliPlugin = state
        .plugins
        .get(cli)
        .cloned()
        .ok_or_else(|| (StatusCode::NOT_FOUND, format!("unknown cli plugin: {cli}")))?;

    // F1: resolve the requested abstract tier into the concrete model for THIS
    // cli, using the per-CLI model config. claude keeps its alias ("sonnet" →
    // "sonnet"); codex gets the user's mapped model or — if unmapped — its own
    // default (no bare "sonnet" forwarded to a custom provider → no 503). A raw
    // model id passes through verbatim. This is the single chokepoint for all
    // three spawn paths (/api/agent, /api/worker, run_spell).
    let model = {
        let resolved = state.models.read().await.resolve(cli, model.as_deref());
        if resolved != model {
            tracing::info!(cli = %cli, from = ?model, to = ?resolved, "model tier resolved per-CLI");
        }
        resolved
    };

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
        model,
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
            // Direction this agent belongs to. `None` = the workspace's main
            // thread (resolved by callers; legacy/pre-thread spawns also None).
            thread_id: thread_id.clone(),
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
                        // Non-zero exit = abnormal death → Error (the UI
                        // surfaces it red, sorted to top). Clean exit → Exited.
                        // Intentional kills also exit non-zero, but those rows
                        // carry `killed_at`, which the UI prioritizes over this.
                        let next = if code == 0 {
                            AgentState::Exited
                        } else {
                            AgentState::Error
                        };
                        swarm.publish_event(SwarmEvent::AgentState {
                            agent_id: agent_for_task.clone(),
                            state: next,
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

    // Tail this worker's CLI session transcript to surface tool-level activity
    // in the UI — zero cost to the worker (we read the JSONL it already writes,
    // never hook or slow it). No-op for a CLI with no known transcript format.
    crate::transcript::spawn_tailer(
        state.swarm.clone(),
        agent_id.clone(),
        result.slot.cli.clone(),
        std::path::PathBuf::from(&result.slot.workspace),
        result.transcript_session_id.clone(),
    );

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
                thread_id: None, // backfilled from SQLite below
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
                    info.thread_id = row.thread_id;
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
                        thread_id: row.thread_id,
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

/// Full agent teardown, shared by REST `DELETE /api/agent/:id` and the WS
/// `ClientControl::Kill` path so the two can't diverge (F1): kill the PTY, drop
/// the in-memory inbox, unregister the wake subscription, persist the kill, and
/// broadcast `Exited`. Returns `true` if the agent existed (caller maps to
/// 204 vs 404). NOTE: this does NOT post a farewell message or clear exit_keys
/// — those are specific to the WakeCoordinator's auto-kill-on-handoff path.
pub(crate) async fn teardown_agent(state: &AppState, agent_id: &str) -> bool {
    match state.registry.remove(agent_id) {
        Some(slot) => {
            {
                let slot = slot.lock();
                slot.bridge.kill();
            }
            // Drop the in-memory inbox before persisting the kill so any
            // in-flight send_message sees "no inbox" rather than racing
            // against a half-torn-down agent.
            state.swarm.unregister_agent(agent_id);
            // M6b: tear down the wake subscription too so we don't try
            // to inject into a registry slot that's about to be dropped.
            crate::wake::unregister_wake_subs(&state.wake_subs, agent_id).await;
            if let Err(e) = state
                .store
                .record_agent_kill(agent_id.to_string(), now_ms())
                .await
            {
                tracing::warn!(?e, agent = %agent_id, "record_agent_kill failed");
            }
            state.swarm.publish_event(SwarmEvent::AgentState {
                agent_id: agent_id.to_string(),
                state: AgentState::Exited,
            });
            true
        }
        None => false,
    }
}

pub async fn kill(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
) -> impl IntoResponse {
    if teardown_agent(&state, &agent_id).await {
        (StatusCode::NO_CONTENT, Json(json!({"ok": true})))
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("agent {agent_id} not found")})),
        )
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

/// Per-agent bootstrap-injection context — the only things that differ
/// between the `spawn_worker` (ad-hoc) and `run_spell` launch paths.
struct BootstrapCtx {
    /// "worker" or "spell" — surfaced in log lines.
    source: &'static str,
    /// Spell name for spell-launched agents; empty for ad-hoc workers.
    spell: String,
    /// Declared role-id keys; used to flag a surviving `{<role>_id}` / `{task}`
    /// placeholder in the rendered prompt (empty for raw worker prompts).
    role_keys: Vec<String>,
}

/// Background task: wait for `ShimReady` (short-circuit if it already fired),
/// let the agent's MCP servers settle, then paste `prompt` + Enter into its
/// PTY. Fail-soft — every error path `warn!`s and returns.
///
/// This is the SINGLE home of the timing-sensitive bootstrap sequence. It was
/// previously copy-pasted between `spawn_worker` and `run_spell` (the F22
/// finding); extracting it means the 2500ms MCP-settle window, the
/// paste→150ms→`\r` submit split, and the ShimReady race handling can never
/// drift between the two paths.
fn spawn_bootstrap_inject(
    registry: crate::registry::Registry,
    mut rx: tokio::sync::broadcast::Receiver<LifecycleEvent>,
    agent_id: String,
    prompt: String,
    ctx: BootstrapCtx,
    // P1-D readiness gate: blackboard keys this agent depends on. The first
    // prompt is NOT injected until all are present (or their `.error`/`.failed`
    // alias). Empty ⇒ inject immediately (orchestrators / dep-less workers).
    deps: Vec<String>,
    swarm: std::sync::Arc<flockmux_swarm::Swarm>,
    // This worker's spawn time (unix-ms). A dep only satisfies the gate if its
    // latest blackboard write is at/after this — so a STALE key left on disk by
    // a PRIOR run on the same thread can't bypass the gate.
    spawned_at: i64,
) {
    tokio::spawn(async move {
        // Short-circuit if ShimReady already fired in the gap between
        // spawn_agent returning and our resubscribe — the PTY pump runs
        // concurrently with the spawn caller, so for fast CLIs OSC_READY can
        // arrive before a receiver is hooked up. Reading the mutex covers it.
        let already_ready = registry
            .get(&agent_id)
            .map(|s| s.lock().lifecycle.lock().shim_ready)
            .unwrap_or(false);
        if !already_ready {
            let wait_ready = async {
                loop {
                    match rx.recv().await {
                        Ok(LifecycleEvent::ShimReady) => return Ok(()),
                        Ok(LifecycleEvent::ShimExit(code)) => {
                            return Err(format!("agent exited before ShimReady (code={code})"));
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            return Err("lifecycle channel closed".into());
                        }
                    }
                }
            };
            match tokio::time::timeout(std::time::Duration::from_secs(30), wait_ready).await {
                Ok(Ok(())) => {}
                Ok(Err(msg)) => {
                    tracing::warn!(source = ctx.source, spell = %ctx.spell, agent = %agent_id, msg = %msg, "bootstrap aborted");
                    return;
                }
                Err(_) => {
                    tracing::warn!(source = ctx.source, spell = %ctx.spell, agent = %agent_id, "bootstrap timed out waiting for ShimReady");
                    return;
                }
            }
        }
        // Wait until the agent's MCP tools are actually visible to the model
        // before injecting — otherwise the model reads an empty toolset and
        // hand-waves "I don't have a swarm_send_message tool". The agent's own
        // flockmux-mcp pings /api/agent/:id/mcp-ready when the CLI fetches its
        // tool list (MCP lifecycle), flipping the slot's `mcp_ready` watch. We
        // wait for that real signal (readiness-probe pattern) with a bounded
        // fallback for any CLI/case that never pings. This replaces a fixed
        // 2500ms sleep: claude/codex emit no stable "MCP ready" banner to
        // scrape (verified empirically), and a fixed sleep both over-waits on
        // fast starts and under-waits on slow ones (a known anti-pattern).
        let slot_lock = match registry.get(&agent_id) {
            Some(s) => s,
            None => {
                tracing::warn!(source = ctx.source, spell = %ctx.spell, agent = %agent_id, "slot vanished before bootstrap");
                return;
            }
        };
        // Subscribe without holding the parking_lot guard across the await.
        let mut mcp_rx = slot_lock.lock().mcp_ready.subscribe();
        if !*mcp_rx.borrow() {
            // Generous cap: only applies when the ping never arrives (e.g. a
            // future CLI without MCP, or a lost ping). On the happy path the
            // watch fires in ~1-2s and we proceed immediately.
            const MCP_READY_FALLBACK: std::time::Duration = std::time::Duration::from_secs(6);
            tokio::select! {
                _ = mcp_rx.changed() => {
                    tracing::debug!(source = ctx.source, spell = %ctx.spell, agent = %agent_id, "mcp ready; injecting bootstrap");
                }
                _ = tokio::time::sleep(MCP_READY_FALLBACK) => {
                    tracing::warn!(source = ctx.source, spell = %ctx.spell, agent = %agent_id, "mcp-ready not signalled within fallback; injecting anyway");
                }
            }
        }

        // ── P1-D readiness gate ───────────────────────────────────────────
        // Do NOT inject the worker's first prompt until every declared
        // dependency (or its `.error`/`.failed` failure alias) is on the
        // blackboard. A dependent worker therefore CANNOT run its first turn on
        // inputs that don't exist yet — the premature-execution bug (observed:
        // a reviewer judged FAIL before its producer wrote the file) is made
        // structurally impossible at the mechanism level; the prompt INPUTS
        // block becomes a secondary catch. The PTY sits idle (no tokens) while
        // waiting; the producer's write lands the key and the next poll
        // proceeds. A producer that DIES writes `<key>.error` (M6c), accepted by
        // the alias check so the worker wakes to handle the failure rather than
        // hang. Aborts if the agent is killed meanwhile.
        if !deps.is_empty() {
            const POLL: std::time::Duration = std::time::Duration::from_millis(750);
            const LOG_EVERY: std::time::Duration = std::time::Duration::from_secs(30);
            // Bound: if a declared producer is NEVER spawned (and so never writes
            // a key OR a `.error`), don't poll forever as a phantom-alive agent.
            // On timeout, inject anyway — the prompt INPUTS block then catches the
            // missing input and the worker fails LOUD (surfacing the mistake to
            // the orchestrator) instead of hanging invisibly.
            const MAX_WAIT: std::time::Duration = std::time::Duration::from_secs(300);
            let start = std::time::Instant::now();
            let mut since_log = LOG_EVERY; // log once immediately on first wait
            loop {
                if registry.get(&agent_id).is_none() {
                    tracing::info!(agent = %agent_id, "readiness gate: agent gone before deps satisfied; aborting bootstrap");
                    return;
                }
                // A dep counts as present only if its latest blackboard write is
                // FRESH (`at >= spawned_at`). A stale key left by a prior run on
                // the same thread must NOT satisfy the gate — else the premature-
                // execution bug silently returns against stale inputs. `.error`/
                // `.failed` aliases count (fail-loud on producer death).
                let mut present = std::collections::HashSet::new();
                for key in &deps {
                    for probe in [key.clone(), format!("{key}.error"), format!("{key}.failed")] {
                        let fresh = swarm
                            .store()
                            .list_blackboard_ops(Some(probe.clone()))
                            .await
                            .ok()
                            .and_then(|ops| ops.first().map(|r| r.at))
                            .is_some_and(|at| at >= spawned_at);
                        if fresh {
                            present.insert(probe);
                        }
                    }
                }
                let missing = first_unsatisfied_dep(&deps, &present);
                if missing.is_none() {
                    tracing::info!(agent = %agent_id, deps = ?deps, "readiness gate: deps satisfied; injecting first turn");
                    break;
                }
                if start.elapsed() >= MAX_WAIT {
                    tracing::warn!(agent = %agent_id, waiting_for = ?missing, max_wait_s = MAX_WAIT.as_secs(), "readiness gate: timed out; injecting anyway (producer may never have spawned) — worker's INPUTS block will fail loud");
                    break;
                }
                if since_log >= LOG_EVERY {
                    tracing::info!(agent = %agent_id, waiting_for = ?missing, deps = ?deps, elapsed_s = start.elapsed().as_secs(), "readiness gate: holding first turn until deps land");
                    since_log = std::time::Duration::ZERO;
                }
                tokio::time::sleep(POLL).await;
                since_log += POLL;
            }
        }

        let input_tx = slot_lock.lock().input_tx.clone();
        // Diagnostic: flag a surviving `{task}` / `{<role>_id}` placeholder
        // (computed before `prompt` is consumed by `into_bytes`).
        let has_unsubst = prompt.contains("{task}")
            || ctx.role_keys.iter().any(|r| prompt.contains(&format!("{{{r}_id}}")));
        let body = prompt.into_bytes();
        let body_len = body.len();
        // Submit as TWO frames (paste body, settle 150ms, then \r): claude/
        // codex TUIs classify a burst containing newlines as a *paste*, so a
        // \r in the same burst becomes a literal newline rather than a submit.
        // Splitting lets the TUI settle the paste, then the standalone \r reads
        // as Enter. 150ms is empirical (50ms sometimes missed cold-start codex).
        if let Err(err) = input_tx.send(bytes::Bytes::from(body)).await {
            tracing::warn!(source = ctx.source, spell = %ctx.spell, agent = %agent_id, ?err, "PTY paste send failed during bootstrap");
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        if let Err(err) = input_tx.send(bytes::Bytes::from_static(b"\r")).await {
            tracing::warn!(source = ctx.source, spell = %ctx.spell, agent = %agent_id, ?err, "PTY submit send failed during bootstrap");
            return;
        }
        tracing::info!(
            source = ctx.source,
            spell = %ctx.spell,
            agent = %agent_id,
            bytes = body_len,
            has_unsubstituted_placeholders = has_unsubst,
            "bootstrap prompt injected"
        );
    });
}

pub async fn spawn_worker(
    State(state): State<AppState>,
    Json(req): Json<SpawnWorkerRequest>,
) -> Result<Json<SpawnWorkerResponse>, (StatusCode, Json<serde_json::Value>)> {
    if req.role.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "missing role (pass a registry slug; see swarm_list_roles)"})),
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

    // Inherit the caller's direction (thread): a worker runs in the same
    // thread — and thus the same worktree cwd — as the orchestrator/worker
    // that delegated it, so siblings on one direction don't clobber another
    // direction's working tree. A genuine `None` (caller has no thread) = the
    // workspace's main thread, whose cwd is the workspace cwd.
    // A hard lookup error must NOT silently fall back to the workspace cwd:
    // for an isolated direction that would run file work in the WRONG (shared)
    // tree. So a DB error fails the spawn; only a genuine `None` (caller has no
    // thread) maps to the main thread / workspace cwd.
    let thread_id = state
        .store
        .get_thread_id_for_agent(req.caller_agent_id.clone())
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("thread lookup failed: {e}")})),
            )
        })?;
    // Both the cwd AND the slug: the slug is the blackboard namespace segment
    // (`<workspace_id>/<thread_slug>/…`) that minted handoff keys are scoped by,
    // so producer + consumer keys match within a direction and never collide
    // across directions. Main thread (no row) → slug "main".
    let (thread_cwd, thread_slug) = match thread_id.as_ref() {
        Some(tid) => match state.store.get_thread(tid.clone()).await {
            Ok(Some(t)) => (t.cwd, t.slug),
            // Thread row gone (deleted) → fall back to the main/project cwd.
            Ok(None) => (ws.cwd.clone(), "main".to_string()),
            // Hard error: don't guess a directory; fail loudly.
            Err(e) => {
                return Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error": format!("thread cwd lookup failed: {e}")})),
                ))
            }
        },
        None => (ws.cwd.clone(), "main".to_string()),
    };
    let layout = WorkspaceLayout::Shared {
        dir: PathBuf::from(&thread_cwd),
    };

    // ── P0: role registry resolution + typed handoff minting ─────────────
    // Effective registry = global (builtin + repo) overlaid by this
    // workspace/direction's project `.flockmux/roles/` (override by slug).
    let mut registry = (*state.roles).clone();
    let project_roles_dir = PathBuf::from(&thread_cwd).join(".flockmux").join("roles");
    if project_roles_dir.is_dir() {
        match crate::roles::RoleRegistry::load_dir(&project_roles_dir) {
            Ok(proj) => registry.overlay(proj),
            Err(e) => {
                tracing::warn!(?e, dir = %project_roles_dir.display(), "project roles overlay failed")
            }
        }
    }

    // Validate the role slug. Unknown → 400 with valid options + did-you-mean.
    let role_slug = req.role.trim().to_string();
    let role = registry.get(&role_slug).cloned().ok_or_else(|| {
        let valid = registry.ids();
        let mut msg = format!("unknown role '{role_slug}'");
        if let Some(s) = closest_match(&role_slug, &valid) {
            msg.push_str(&format!(" — did you mean '{s}'?"));
        }
        msg.push_str(&format!(" valid roles: {valid:?}"));
        (StatusCode::BAD_REQUEST, Json(json!({ "error": msg })))
    })?;
    let manifest = &role.manifest;

    // Resolve cli/model from the role's defaults unless explicitly overridden.
    let resolved_cli = req
        .cli
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(manifest.default_cli.as_str())
        .to_string();
    if resolved_cli.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": format!("role '{role_slug}' has no default_cli and no cli override was given")})),
        ));
    }
    let resolved_model = req
        .model
        .clone()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            let t = manifest.default_model_tier.trim();
            (!t.is_empty()).then(|| t.to_string())
        });
    let role_label = if manifest.name.trim().is_empty() {
        role_slug.clone()
    } else {
        manifest.name.clone()
    };

    // Effective produces: spawn override → role.produces → ["done"].
    let produces: Vec<String> = if !req.produces.is_empty() {
        req.produces
            .iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    } else if !manifest.produces.is_empty() {
        manifest.produces.clone()
    } else {
        vec!["done".to_string()]
    };

    // Mint the canonical handoff key(s) this worker writes (one per kind), plus
    // the single primary signal (the "done" kind if present, else the first).
    let minted_produces: Vec<String> = produces
        .iter()
        .map(|k| crate::roles::mint_handoff_key(&req.workspace_id, &thread_slug, &role_slug, k))
        .collect();
    let primary_kind = if produces.iter().any(|k| k == "done") {
        "done"
    } else {
        produces[0].as_str()
    };
    let handoff_signal =
        crate::roles::mint_handoff_key(&req.workspace_id, &thread_slug, &role_slug, primary_kind);
    // The failure key is the primary handoff key + `.error`, so the existing
    // `base_key_aliases` fan-out (`<key>.error` → `<key>`) wakes exactly the
    // consumers that wait on the success key — no separate wiring needed.
    let error_key = format!("{handoff_signal}.error");

    // ── Spawn-time dependency-graph validation (fail LOUD, not silent) ───
    // Resolve each typed `consumes` ref to the producer's minted key, after
    // verifying the producer role exists AND declares that output kind. This
    // is the structural fix for the F3 drift class: a typo/unknown dep is
    // rejected here with valid options, never a silent never-wake. Pure logic
    // lives in `resolve_consumes_to_deps` (unit-tested).
    let depends_on = resolve_consumes_to_deps(
        &registry,
        &role_slug,
        &req.consumes,
        &req.workspace_id,
        &thread_slug,
    )
    .map_err(|msg| (StatusCode::BAD_REQUEST, Json(json!({ "error": msg }))))?;

    // The orchestrator's prompt + an INPUTS wait-gate (if it has deps) + an
    // explicit copy-verbatim handoff block.
    let system_prompt =
        build_worker_prompt(&req.system_prompt, &minted_produces, &error_key, &depends_on);
    let produces_json = serde_json::to_string(&produces).unwrap_or_else(|_| "[]".to_string());
    let consumes_json = serde_json::to_string(&req.consumes).unwrap_or_else(|_| "[]".to_string());

    // Fork-bomb guard (F4), recursion arm: bound the delegation depth so a
    // worker that spawns a worker that spawns a worker… can't recurse without
    // limit. Walk the parent chain in the workers table from the caller up;
    // the orchestrator is a spell agent (not a worker), so it isn't in the
    // table and the walk terminates there. The global live-agent cap bounds
    // total blast radius; this gives a faster, clearer rejection for deep
    // chains and is loop-bounded so a corrupt/cyclic link can't hang the walk.
    {
        let cap = max_spawn_depth();
        let mut depth = 1usize; // the worker we're about to spawn
        let mut cur = req.caller_agent_id.clone();
        for _ in 0..cap + 2 {
            let rows = state
                .store
                .list_workers_by_ids(vec![cur.clone()])
                .await
                .map_err(|e| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({"error": format!("worker depth lookup failed: {e}")})),
                    )
                })?;
            match rows.get(&cur) {
                Some(w) if !w.parent_agent_id.is_empty() => {
                    depth += 1;
                    cur = w.parent_agent_id.clone();
                }
                _ => break,
            }
        }
        if depth > cap {
            return Err((
                StatusCode::TOO_MANY_REQUESTS,
                Json(json!({"error": format!(
                    "spawn-depth cap reached (depth {depth} > {cap}); refusing to delegate deeper. \
                     Flatten the work or raise FLOCKMUX_MAX_SPAWN_DEPTH."
                )})),
            ));
        }
    }

    // Readiness-gate baseline: any dep written at/after this counts as a
    // CURRENT-run input; earlier writes are stale (prior run) and ignored.
    let worker_spawn_ms = now_ms();
    let out = spawn_with_bookkeeping(
        &state,
        &resolved_cli,
        Some(role_label.clone()),
        resolved_model.clone(),
        layout,
        req.workspace_id.clone(),
        None,             // ad-hoc workers don't belong to a spell run
        thread_id.clone(), // P3③: inherit the caller's direction (None = main)
    )
    .await
    .map_err(|(status, msg)| (status, Json(json!({"error": msg}))))?;

    // P1-D: the worker's dependencies are enforced by the readiness GATE inside
    // spawn_bootstrap_inject (it won't inject the first prompt until every dep —
    // or its `.error` alias — is on the blackboard). So we deliberately do NOT
    // register the worker in wake_subs for its OWN deps: that old re-wake path
    // would fire a PTY kick at the still-un-prompted worker the instant a dep
    // landed, racing the gate's inject and risking a spurious empty turn. The
    // gate polls the blackboard itself and delivers the single first turn.
    // (register_exit_key + the orchestrator's append_wake_sub below stay — they
    // fire when THIS worker writes its handoff, unrelated to its inputs.)
    crate::wake::register_exit_key(
        &state.exit_keys,
        out.agent_id.clone(),
        role_slug.clone(),
        handoff_signal.clone(),
        now_ms(),
    )
    .await;

    // Magentic-One closes the loop here: the orchestrator (the spawning agent)
    // is woken when this worker writes its minted handoff key, so it can read
    // the artifact, update the Progress Ledger, and decide what's next.
    // Append-not-overwrite so it can have many workers in flight at once.
    if !handoff_signal.is_empty() && !req.caller_agent_id.is_empty() {
        crate::wake::append_wake_sub(
            &state.wake_subs,
            req.caller_agent_id.clone(),
            handoff_signal.clone(),
        )
        .await;
    }

    // Persist worker metadata. Failure is non-fatal (PTY is already live),
    // but the DAG view will miss the parent edge until next listAgents
    // refresh after a successful retry. We store the AUGMENTED prompt (what
    // was actually injected) for faithful replay.
    let depends_on_json = serde_json::to_string(&depends_on).unwrap_or_else(|_| "[]".to_string());
    if let Err(e) = state
        .store
        .record_worker(NewWorker {
            agent_id: out.agent_id.clone(),
            parent_agent_id: req.caller_agent_id.clone(),
            role_label: role_label.clone(),
            system_prompt: system_prompt.clone(),
            handoff_signal: handoff_signal.clone(),
            depends_on_json,
            spawned_at: now_ms(),
            role_slug: role_slug.clone(),
            produces_json,
            consumes_json,
        })
        .await
    {
        tracing::warn!(?e, agent = %out.agent_id, "record_worker failed");
    }

    // Bootstrap inject — shared with run_spell (see spawn_bootstrap_inject).
    // We inject the AUGMENTED prompt (orchestrator's text + the minted handoff
    // block) so the worker is told the exact canonical key to write.
    spawn_bootstrap_inject(
        state.registry.clone(),
        out.lifecycle_rx.resubscribe(),
        out.agent_id.clone(),
        system_prompt.clone(),
        BootstrapCtx {
            source: "worker",
            spell: String::new(),
            role_keys: Vec::new(),
        },
        depends_on.clone(), // P1-D: gate the first turn on these minted keys
        state.swarm.clone(),
        worker_spawn_ms,
    );

    Ok(Json(SpawnWorkerResponse {
        agent_id: out.agent_id,
        cli: out.cli,
        role_label,
        workspace: out.workspace,
        handoff_signal,
        depends_on,
    }))
}

/// `GET /api/roles` — the role registry catalog for `swarm_list_roles` and the
/// UI. Returns the global (builtin + repo) registry; per-workspace
/// `.flockmux/roles/` overrides are applied at spawn time, not here.
pub async fn list_roles(State(state): State<AppState>) -> Json<serde_json::Value> {
    let rows: Vec<serde_json::Value> = state
        .roles
        .list()
        .iter()
        .map(|r| {
            let m = &r.manifest;
            json!({
                "id": m.id,
                "name": m.name,
                "when_to_use": m.when_to_use,
                "default_cli": m.default_cli,
                "default_model_tier": m.default_model_tier,
                "produces": if m.produces.is_empty() { vec!["done".to_string()] } else { m.produces.clone() },
                "modality": m.modality,
                "risk": m.risk,
            })
        })
        .collect();
    Json(json!(rows))
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

    // Auto-create the workspace's `main` direction so every workspace owns at
    // least one thread from birth. Zero-friction: shared isolation, cwd = the
    // workspace cwd, ready immediately. A store failure here is non-fatal —
    // agents simply fall back to the legacy `thread_id = None` (= main) path —
    // so we log and return an empty thread list rather than 500 the creation.
    let threads: Vec<ThreadInfo> = match state
        .store
        .create_thread(
            NewThread {
                workspace_id: rec.id.clone(),
                slug: "main".to_string(),
                name: Some("main".to_string()),
                isolation: "shared".to_string(),
                branch: None,
                cwd: rec.cwd.clone(),
                state: "ready".to_string(),
            },
            now_ms(),
        )
        .await
    {
        Ok(t) => vec![thread_record_to_info(t)],
        Err(e) => {
            tracing::warn!(?e, workspace = %rec.id, "create main thread failed; agents fall back to main = None");
            Vec::new()
        }
    };

    // Attach any dependency-source roots the wizard sent. Each is validated
    // the same way as the primary cwd above (exists + is a dir → 4xx), then
    // persisted. The workspace row already exists at this point; a failed root
    // insert returns 500 without rolling back the workspace (acceptable — the
    // user can re-attach the root). Empty/whitespace paths are skipped.
    let mut roots: Vec<WorkspaceRoot> = Vec::new();
    for root in req.roots {
        let p = root.path.trim();
        if p.is_empty() {
            continue;
        }
        let path = std::path::Path::new(p);
        if !path.exists() {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({"error": format!("dependency directory does not exist: {}", p)})),
            ));
        }
        if !path.is_dir() {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({"error": format!("dependency path is not a directory: {}", p)})),
            ));
        }
        // The wizard only ever creates the primary + peers + under-primary
        // deps, so every root it sends is a top-level node (parent_id=None).
        // Any client-supplied id is ignored — the server mints it.
        let saved = state
            .store
            .add_workspace_root(
                NewWorkspaceRoot {
                    workspace_id: rec.id.clone(),
                    path: p.to_string(),
                    role: root.role,
                    label: root.label,
                    parent_id: None,
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
        roots.push(WorkspaceRoot {
            id: saved.id,
            path: saved.path,
            role: saved.role,
            label: saved.label,
            parent_id: saved.parent_id,
            branch: None, // filled at list time
        });
    }

    // If any tree nodes were attached, write a flockmux-managed context block
    // into the primary project dir so the spawned orchestrator (claude →
    // CLAUDE.md, codex → AGENTS.md) reads the attached source directly instead
    // of decompiling/guessing. Best-effort; never fatal.
    if !roots.is_empty() {
        write_workspace_deps_context(rec.cwd.trim(), &rec.name, &roots);
    }

    Ok(Json(Workspace {
        id: rec.id,
        slug: rec.slug,
        name: rec.name,
        cwd: rec.cwd,
        cwd_branch: None, // filled at list time
        accent: rec.accent,
        created_at: rec.created_at,
        member_count: 0,
        roots,
        threads,
    }))
}

/// Write/refresh a flockmux-managed "workspace structure" block into the
/// workspace's CLAUDE.md and AGENTS.md so the orchestrator reads the attached
/// source directly (best practice: a per-project context file) instead of
/// decompiling/guessing. The block renders the workspace's user-defined
/// LOGICAL tree: the primary project (`cwd` + `name`) plus every attached
/// node, nested by `parent_id`. Idempotent: the block is delimited by
/// HTML-comment markers and replaced in place on re-write; any user content
/// outside the markers is preserved. When `roots` is empty the managed block
/// is STRIPPED instead of written (the inverse path — used when the last
/// attached node is removed), leaving any surrounding user content intact.
/// Best-effort — failures are logged, never fatal.
fn write_workspace_deps_context(cwd: &str, name: &str, roots: &[WorkspaceRoot]) {
    use std::fmt::Write as _;
    const START: &str = "<!-- flockmux:deps:start -->";
    const END: &str = "<!-- flockmux:deps:end -->";

    // No roots left → strip the managed block (and trailing blank lines)
    // from each context file if present. We never create a file here; only
    // existing files with a managed block are rewritten.
    if roots.is_empty() {
        for fname in ["CLAUDE.md", "AGENTS.md"] {
            let path = std::path::Path::new(cwd).join(fname);
            let existing = match std::fs::read_to_string(&path) {
                Ok(s) => s,
                Err(_) => continue, // file doesn't exist / unreadable — nothing to strip
            };
            if let (Some(s), Some(e)) = (existing.find(START), existing.find(END)) {
                let end_full = e + END.len();
                // Drop the block plus any trailing newlines that followed it
                // so we don't leave a dangling blank gap behind.
                let after = existing[end_full..].trim_start_matches(['\n', '\r']);
                let before = &existing[..s];
                // If the block was the only content, `before` is empty / blank
                // and the file becomes empty — that's fine per spec.
                let stripped = if after.is_empty() {
                    before.trim_end().to_string()
                } else {
                    format!("{}{}", before, after)
                };
                if stripped.trim().is_empty() {
                    // The managed block was the file's only content — delete the
                    // now-empty file instead of leaving a dangling 0-byte
                    // CLAUDE.md/AGENTS.md behind (flockmux created it, flockmux
                    // cleans it up). If the user had their own content around the
                    // block, `stripped` is non-empty and we keep+rewrite it.
                    match std::fs::remove_file(&path) {
                        Ok(()) => tracing::info!(file = %path.display(), "removed empty workspace deps context file (no roots left)"),
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                        Err(e) => tracing::warn!(?e, file = %path.display(), "failed removing empty workspace deps context file"),
                    }
                } else if let Err(e) = std::fs::write(&path, stripped) {
                    tracing::warn!(?e, file = %path.display(), "failed stripping workspace deps context");
                } else {
                    tracing::info!(file = %path.display(), "stripped workspace deps context (no roots left)");
                }
            }
        }
        return;
    }

    // Render the prefix label for one tree node by role.
    fn node_label(role: &str) -> &'static str {
        match role {
            "project" => "项目",
            "tool" => "[工具]",
            _ => "[依赖]",
        }
    }
    // Emit `node` (and recurse into its children) at the given depth. Children
    // are the roots whose parent_id == node.id, in slice order (already sorted
    // by created_at by the caller). `depth` controls the 2-space indent.
    fn emit_node(block: &mut String, node: &WorkspaceRoot, roots: &[WorkspaceRoot], depth: usize) {
        let indent = "  ".repeat(depth);
        let label = node_label(&node.role);
        let name = node.label.as_deref().unwrap_or("");
        let _ = if name.is_empty() {
            writeln!(block, "{indent}- {label} `{}`", node.path)
        } else {
            writeln!(block, "{indent}- {label} {name} `{}`", node.path)
        };
        for child in roots.iter().filter(|r| r.parent_id.as_deref() == Some(node.id.as_str())) {
            emit_node(block, child, roots, depth + 1);
        }
    }

    let mut block = String::new();
    let _ = writeln!(block, "{START}");
    let _ = writeln!(block, "## 工作空间结构 (flockmux managed)");
    let _ = writeln!(block);
    let _ = writeln!(
        block,
        "下面是本工作空间的项目与它们挂载的依赖源码（树中父子表示\"依赖/归属\"，物理\
         路径见每行）。开发时直接阅读/按需修改这些源码——不要反编译 jar/包、不要凭\
         猜测。改动跨项目的共享库时注意它可能被多处使用。"
    );
    let _ = writeln!(block);

    // The PRIMARY project = (cwd, name, role="project"), implicit root.
    // Its children = roots with parent_id=None && role!="project".
    // Top-level peer projects = roots with parent_id=None && role=="project".
    let _ = writeln!(block, "- 项目 {name} `{cwd}`   (primary)");
    for r in roots
        .iter()
        .filter(|r| r.parent_id.is_none() && r.role != "project")
    {
        emit_node(&mut block, r, roots, 1);
    }
    for r in roots
        .iter()
        .filter(|r| r.parent_id.is_none() && r.role == "project")
    {
        emit_node(&mut block, r, roots, 0);
    }
    let _ = write!(block, "{END}");

    for fname in ["CLAUDE.md", "AGENTS.md"] {
        let path = std::path::Path::new(cwd).join(fname);
        let existing = std::fs::read_to_string(&path).unwrap_or_default();
        let next = if let (Some(s), Some(e)) = (existing.find(START), existing.find(END)) {
            // replace existing managed block in place
            let end_full = e + END.len();
            format!("{}{}{}", &existing[..s], block, &existing[end_full..])
        } else if existing.trim().is_empty() {
            block.clone()
        } else {
            format!("{}\n\n{}\n", existing.trim_end(), block)
        };
        if let Err(e) = std::fs::write(&path, next) {
            tracing::warn!(?e, file = %path.display(), "failed writing workspace deps context");
        } else {
            tracing::info!(file = %path.display(), roots = roots.len(), "wrote workspace deps context");
        }
    }
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
    // Fetch every attached root in one shot and group by workspace_id (rows
    // come back ordered by created_at ASC, so each group preserves attach
    // order). Same single-pass rationale as member_count above — avoids N+1.
    let mut roots_by_ws: HashMap<String, Vec<WorkspaceRoot>> = HashMap::new();
    for r in state.store.list_all_workspace_roots().await.unwrap_or_default() {
        roots_by_ws.entry(r.workspace_id).or_default().push(WorkspaceRoot {
            id: r.id,
            path: r.path,
            role: r.role,
            label: r.label,
            parent_id: r.parent_id,
            branch: None, // filled below from branch_map
        });
    }
    // Threads (directions) per workspace. Unlike roots there's no "list all"
    // query; with a handful of workspaces locally, one list_threads each is
    // cheap and keeps the store API minimal. Oldest-first (main leads).
    let mut threads_by_ws: HashMap<String, Vec<ThreadInfo>> = HashMap::new();
    for r in &rows {
        let list = state
            .store
            .list_threads(r.id.clone())
            .await
            .unwrap_or_default();
        threads_by_ws.insert(
            r.id.clone(),
            list.into_iter().map(thread_record_to_info).collect(),
        );
    }
    // Live git branch per path (workspace cwds + every attached root), for the
    // sidebar's branch chips. Batched off the async runtime (git shells out and
    // blocks) and memoized with a short TTL so the frequent workspaces refetch
    // doesn't re-run git every time. Best-effort: on a join error every chip is
    // simply absent.
    let mut paths: Vec<PathBuf> = rows.iter().map(|r| PathBuf::from(&r.cwd)).collect();
    for roots in roots_by_ws.values() {
        paths.extend(roots.iter().map(|rt| PathBuf::from(&rt.path)));
    }
    let branch_map = tokio::task::spawn_blocking(move || branches_for_paths(&paths))
        .await
        .unwrap_or_default();
    // Fold the computed branches into the attached roots.
    for roots in roots_by_ws.values_mut() {
        for rt in roots.iter_mut() {
            rt.branch = branch_map
                .get(std::path::Path::new(&rt.path))
                .cloned()
                .flatten();
        }
    }
    let items: Vec<Workspace> = rows
        .into_iter()
        .map(|r| Workspace {
            member_count: counts.get(&r.id).copied().unwrap_or(0),
            roots: roots_by_ws.remove(&r.id).unwrap_or_default(),
            threads: threads_by_ws.remove(&r.id).unwrap_or_default(),
            cwd_branch: branch_map
                .get(std::path::Path::new(&r.cwd))
                .cloned()
                .flatten(),
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

/// Live git branch for each of `paths`, memoized with a short TTL. Shelling out
/// to git is blocking, so this runs under `spawn_blocking`; the TTL keeps the
/// hot workspaces-list refetch (fired on every thread/agent event) from
/// re-invoking git each time. A path maps to `Some(branch)` only when it's a
/// git work tree on a named branch — `None`/absent renders no chip.
fn branches_for_paths(paths: &[PathBuf]) -> HashMap<PathBuf, Option<String>> {
    use parking_lot::Mutex;
    use std::sync::OnceLock;
    use std::time::{Duration, Instant};
    const TTL: Duration = Duration::from_secs(3);
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, (Option<String>, Instant)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));

    let mut out: HashMap<PathBuf, Option<String>> = HashMap::new();
    for p in paths {
        if out.contains_key(p) {
            continue; // de-dup: a path can appear as both a cwd and a root
        }
        if let Some((b, at)) = cache.lock().get(p) {
            if at.elapsed() < TTL {
                out.insert(p.clone(), b.clone());
                continue;
            }
        }
        let b = crate::worktree::current_branch(p);
        cache.lock().insert(p.clone(), (b.clone(), Instant::now()));
        out.insert(p.clone(), b);
    }
    out
}

// ── threads (directions) ─────────────────────────────────────────────────

/// Ensure `base` is unique among a workspace's ALIVE thread slugs, appending
/// `-2`, `-3`, … on collision. Best-effort: on a list error we return `base`
/// and let the DB's unique index reject a genuine dup.
async fn unique_thread_slug(state: &AppState, workspace_id: &str, base: &str) -> String {
    let existing: std::collections::HashSet<String> = state
        .store
        .list_threads(workspace_id.to_string())
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|t| t.slug)
        .collect();
    if !existing.contains(base) {
        return base.to_string();
    }
    let mut n = 2;
    loop {
        let cand = format!("{base}-{n}");
        if !existing.contains(&cand) {
            return cand;
        }
        n += 1;
    }
}

/// `GET /api/workspaces/:id/threads` — list a workspace's directions
/// (oldest-first; the first entry is the auto-created `main`).
pub async fn list_threads_handler(
    State(state): State<AppState>,
    Path(workspace_id): Path<String>,
) -> impl IntoResponse {
    let list = state
        .store
        .list_threads(workspace_id)
        .await
        .unwrap_or_default();
    Json(
        list.into_iter()
            .map(thread_record_to_info)
            .collect::<Vec<_>>(),
    )
}

/// `POST /api/workspaces/:id/threads` — open a new direction. Zero-friction:
/// `name` optional; created `shared` + `ready` in the workspace's own cwd. Git
/// isolation is DEFERRED until the direction is named (see
/// `update_thread_handler`) so clicking "+ new direction" never blocks on git.
/// The slug is FIXED at creation and never changes — renaming only moves the
/// display name + git branch, so already-spawned agents' blackboard keys
/// (`{workspace_id}/{slug}/…`) stay valid.
pub async fn create_thread_handler(
    State(state): State<AppState>,
    Path(workspace_id): Path<String>,
    Json(req): Json<CreateThreadRequest>,
) -> Result<Json<ThreadInfo>, (StatusCode, Json<serde_json::Value>)> {
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
                Json(json!({"error": format!("unknown workspace_id: {workspace_id}")})),
            )
        })?;
    let name = req.name.as_deref().map(str::trim).filter(|s| !s.is_empty());
    // A name (if any) yields a readable, stable slug; otherwise a short random
    // placeholder. Slug is fixed for the thread's life (see doc above).
    let base_slug = match name {
        Some(n) => crate::worktree::sanitize_suffix(n),
        None => format!("t-{}", &Uuid::new_v4().to_string()[..6]),
    };
    let slug = unique_thread_slug(&state, &workspace_id, &base_slug).await;
    // A direction named up-front (the "新方向" dialog's name field) must isolate
    // exactly like one the orchestrator names from the first message —
    // otherwise filling that field silently defeats worktree isolation (thread
    // stays `shared`, both directions edit one cwd and clobber each other).
    // Named, non-main → create `preparing` + kick off background `worktree add`;
    // the frontend waits for `ready` before spawning the orchestrator so it
    // lands in the worktree. Unnamed keeps the zero-friction shared/ready path
    // (orchestrator isolates on its first-message naming).
    let will_isolate = name.is_some();
    let branch = slug.clone();
    let rec = state
        .store
        .create_thread(
            NewThread {
                workspace_id: workspace_id.clone(),
                slug,
                name: name.map(|s| s.to_string()),
                isolation: "shared".to_string(),
                branch: None,
                cwd: ws.cwd.clone(),
                state: if will_isolate { "preparing" } else { "ready" }.to_string(),
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
    let info = thread_record_to_info(rec);
    if will_isolate {
        // Background git takeover + `worktree add` → flips isolation/cwd/state
        // to worktree/<dir>/ready on success, degrades to shared/ready on any
        // failure (so a non-git project or a git hiccup never blocks the
        // direction — it just stays unisolated, surfaced in the sidebar).
        spawn_thread_worktree(
            state.clone(),
            info.id.clone(),
            workspace_id.clone(),
            ws.cwd.clone(),
            branch,
        );
    }
    publish_thread_changed(&state, &workspace_id, &info.id, "created");
    Ok(Json(info))
}

/// Broadcast that a workspace's direction (thread) list changed so subscribers
/// (the sidebar) refetch `/api/workspaces`. The REST snapshot stays the source
/// of truth; this is just the "now" signal for a change the snapshot can't push
/// itself — notably `swarm_name_thread` → background worktree isolation, which
/// renames + flips the branch icon without any UI-initiated request.
fn publish_thread_changed(state: &AppState, workspace_id: &str, thread_id: &str, op: &str) {
    state.swarm.publish_event(SwarmEvent::ThreadChanged {
        workspace_id: workspace_id.to_string(),
        thread_id: thread_id.to_string(),
        op: op.to_string(),
    });
}

/// `PATCH /api/workspaces/:id/threads/:tid` — (re)name a direction. Naming a
/// previously-unnamed, non-`main` direction is ALSO the trigger for automatic
/// git isolation: we persist the name + branch, flip state to `preparing`,
/// return immediately, and a background task does git takeover + `worktree add`
/// + repoints the thread cwd, finally → `ready`. If git isolation fails (or the
/// project can't be a repo) the direction degrades gracefully to `shared` and
/// stays usable. The `main` direction and already-isolated threads are a pure
/// rename. The slug NEVER changes (keeps blackboard keys stable).
///
/// NOTE (P3 boundary): repointing the cwd does not migrate an ALREADY-running
/// agent's process cwd. The intended flow names the direction from the first
/// message (before file work) and the frontend gates orchestrator/worker spawns
/// on `state == "ready"`, restarting into the new cwd. Sequencing lives in P4/P5.
pub async fn update_thread_handler(
    State(state): State<AppState>,
    Path((workspace_id, thread_id)): Path<(String, String)>,
    Json(req): Json<UpdateThreadRequest>,
) -> Result<Json<ThreadInfo>, (StatusCode, Json<serde_json::Value>)> {
    let name = req.name.trim();
    if name.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "name must be non-empty"})),
        ));
    }
    let thread = state
        .store
        .get_thread(thread_id.clone())
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
        })?
        .filter(|t| t.workspace_id == workspace_id)
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(json!({"error": format!("unknown thread: {thread_id}")})),
            )
        })?;

    // Kick off git isolation only for a not-yet-isolated, non-main direction
    // that isn't already mid-isolation. The `state != "preparing"` guard makes
    // a second concurrent PATCH (double-click / retried swarm_name_thread) a
    // pure rename instead of racing a second `git worktree add` on the same
    // dest. A degraded thread is back at `ready`, so it can still retry.
    let should_isolate = thread.slug != "main"
        && thread.isolation != "worktree"
        && thread.state != "preparing";

    if should_isolate {
        // Branch (and thus the worktree dir `<project>-<branch>`) is derived
        // from the STABLE per-workspace-unique slug, NOT the raw name: two
        // directions sharing a display name still get distinct slugs, so they
        // can't collide on the same branch/worktree dir. `name` only updates
        // the display label.
        let branch = thread.slug.clone();
        // Phase 1 (sync, fast): name + `preparing`. branch/isolation/cwd are
        // persisted in phase 2 only once the worktree actually exists, so a
        // failed isolation never leaves a stale branch on a shared thread.
        state
            .store
            .update_thread(
                thread_id.clone(),
                Some(name.to_string()),
                None, // slug stays stable
                None, // isolation flips in phase 2 on success
                None, // branch persisted in phase 2 on success
                None, // cwd repoints in phase 2 on success
                Some("preparing".to_string()),
            )
            .await
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error": e.to_string()})),
                )
            })?;
        // Phase 2 (background): git takeover + worktree add → ready.
        spawn_thread_worktree(
            state.clone(),
            thread_id.clone(),
            workspace_id.clone(),
            thread.cwd.clone(),
            branch,
        );
    } else {
        state
            .store
            .update_thread(
                thread_id.clone(),
                Some(name.to_string()),
                None,
                None,
                None,
                None,
                None,
            )
            .await
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error": e.to_string()})),
                )
            })?;
    }

    // Phase-1 rename / `preparing` flip is persisted now — tell the sidebar.
    // (Isolation success/degrade fires a second `ThreadChanged` from phase 2.)
    publish_thread_changed(&state, &workspace_id, &thread_id, "updated");

    let updated = state
        .store
        .get_thread(thread_id.clone())
        .await
        .ok()
        .flatten()
        .map(thread_record_to_info)
        .ok_or_else(|| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "thread vanished after update"})),
            )
        })?;
    Ok(Json(updated))
}

/// Background git isolation for a freshly-named direction. The git calls block
/// (each internally time-boxed at 30s) so they run on a blocking thread off the
/// async runtime. On success the thread is repointed to the worktree dir and
/// marked `worktree`/`ready`; on ANY failure it degrades to `shared`/`ready`
/// (cwd unchanged) so the direction stays usable, just not isolated.
fn spawn_thread_worktree(
    state: AppState,
    thread_id: String,
    workspace_id: String,
    project_cwd: String,
    branch: String,
) {
    tokio::spawn(async move {
        let cwd_for_git = project_cwd.clone();
        let branch_for_git = branch.clone();
        let git_result = tokio::task::spawn_blocking(move || {
            let p = std::path::Path::new(&cwd_for_git);
            crate::worktree::git_init_with_commit(p)?;
            crate::worktree::worktree_add(p, &branch_for_git)
        })
        .await;
        match git_result {
            Ok(Ok(dest)) => {
                let dest_str = dest.to_string_lossy().into_owned();
                let update = state
                    .store
                    .update_thread(
                        thread_id.clone(),
                        None,
                        None,
                        Some("worktree".to_string()),
                        Some(branch.clone()),
                        Some(dest_str.clone()),
                        Some("ready".to_string()),
                    )
                    .await;
                // Was the direction soft-deleted while we were isolating?
                // `update_thread` guards on `deleted_at IS NULL`, so the write
                // above no-ops on a mid-flight delete — detect it by re-reading
                // and DON'T re-root into / leak a dead direction.
                let deleted = !matches!(
                    state.store.get_thread(thread_id.clone()).await,
                    Ok(Some(t)) if t.deleted_at.is_none()
                );
                if deleted {
                    tracing::info!(thread = %thread_id, "direction deleted during isolation; removing orphaned worktree");
                    let repo = project_cwd.clone();
                    let d = dest_str.clone();
                    let _ = tokio::task::spawn_blocking(move || {
                        crate::worktree::worktree_remove(
                            std::path::Path::new(&repo),
                            std::path::Path::new(&d),
                        )
                    })
                    .await;
                } else if let Err(e) = update {
                    // The worktree exists on disk but we couldn't record it.
                    // Degrade to shared/ready so the direction is never left
                    // stuck in `preparing` (the worktree dir is orphaned —
                    // acceptable; it's reusable on a later same-slug retry).
                    tracing::warn!(?e, thread = %thread_id, "worktree built but thread update failed; degrading to shared");
                    degrade_thread_to_shared(&state, &thread_id).await;
                    publish_thread_changed(&state, &workspace_id, &thread_id, "updated");
                } else {
                    tracing::info!(thread = %thread_id, dest = %dest_str, "direction isolated in git worktree");
                    // Re-emit the workspace deps-context into the worktree so the
                    // re-rooted orchestrator (whose new cwd is the worktree, a
                    // copy of the cwd repo only) can still see the peer/dependency
                    // projects at their real paths — otherwise it loses sight of
                    // repos the user may be asking it to work on.
                    write_deps_context_into_dir(&state, &workspace_id, &dest_str).await;
                    // P5-D: re-root the orchestrator into the fresh worktree.
                    // The orchestrator that named the direction is still
                    // running in the OLD (shared) cwd, so its own edits + any
                    // workers it dispatches would split-brain across two dirs.
                    reroot_thread_orchestrator(&state, &workspace_id, &thread_id, &dest_str)
                        .await;
                    // worktree/ready + cwd repointed → sidebar flips to the
                    // branch icon + worktree path live (no reload).
                    publish_thread_changed(&state, &workspace_id, &thread_id, "isolated");
                }
            }
            Ok(Err(e)) => {
                tracing::warn!(?e, thread = %thread_id, "git isolation failed; direction stays shared");
                degrade_thread_to_shared(&state, &thread_id).await;
                publish_thread_changed(&state, &workspace_id, &thread_id, "updated");
            }
            Err(e) => {
                tracing::warn!(?e, thread = %thread_id, "worktree task panicked; direction stays shared");
                degrade_thread_to_shared(&state, &thread_id).await;
                publish_thread_changed(&state, &workspace_id, &thread_id, "updated");
            }
        }
    });
}

/// Mark a direction `degraded`/`ready` after a failed isolation attempt. We use
/// a distinct `isolation = "degraded"` (not plain `shared`) so the sidebar can
/// SIGNAL that isolation was attempted and failed — otherwise it looks identical
/// to a not-yet-isolated direction and the user wrongly believes their work is
/// isolated when it's actually sharing the main cwd (two directions' agents then
/// clobber each other's files). `degraded != "worktree"`, so a later rename
/// still retries isolation, and the delete path still skips worktree removal.
async fn degrade_thread_to_shared(state: &AppState, thread_id: &str) {
    let _ = state
        .store
        .update_thread(
            thread_id.to_string(),
            None,
            None,
            Some("degraded".to_string()),
            None,
            None,
            Some("ready".to_string()),
        )
        .await;
}

/// P5-D: after a direction is isolated into a worktree, re-root its orchestrator
/// there. The orchestrator that named the direction is still running in the OLD
/// (shared) cwd; restarting it in the worktree keeps its self-edits + any
/// workers it dispatches from splitting across two directories. We kill the
/// direction's live agents (naming happens BEFORE any worker is dispatched, per
/// orchestrator.md, so this is usually just the orchestrator) and re-run `init`
/// in the new cwd — the fresh orchestrator reads the existing ledger (Phase A
/// short-circuit) and continues. Only ever fires for a git-isolated direction.
async fn reroot_thread_orchestrator(
    state: &AppState,
    workspace_id: &str,
    thread_id: &str,
    new_cwd: &str,
) {
    // Agents we tear down here — their as-yet-unread user messages get
    // re-addressed to the fresh orchestrator below so nothing is dropped.
    let mut killed_ids: Vec<String> = Vec::new();
    match state.store.list_agents().await {
        Ok(rows) => {
            for a in rows {
                if a.thread_id.as_deref() == Some(thread_id)
                    && a.killed_at.is_none()
                    && a.shim_exit_at.is_none()
                {
                    teardown_agent(state, &a.id).await;
                    killed_ids.push(a.id);
                }
            }
            // Naming is supposed to happen BEFORE any worker is dispatched
            // (orchestrator.md), so this should usually kill just the
            // orchestrator. If it killed more, a worker was torn down mid-task
            // (its old-cwd work is abandoned) — surface the invariant breach.
            if killed_ids.len() > 1 {
                tracing::warn!(
                    thread = %thread_id, killed = killed_ids.len(),
                    "re-root tore down >1 agent — a worker was dispatched before naming"
                );
            }
        }
        Err(e) => {
            tracing::warn!(?e, thread = %thread_id, "re-root: list_agents failed; old agents may linger in the shared cwd");
        }
    }
    // The first orchestrator read the user's opening request to NAME this
    // direction, then was torn down (above) BEFORE it wrote a ledger — so the
    // fresh orchestrator would otherwise have neither the ledger nor the
    // (already-read) message and would re-onboard from scratch. Seed its
    // `{task}` with that request so its first turn acts on it instead of asking
    // the user what they want all over again. Best-effort: empty seed on any
    // error just reverts to the old first-wake greeting.
    let seed_task = state
        .store
        .latest_user_message_for_agents(killed_ids.clone())
        .await
        .unwrap_or(None)
        .unwrap_or_default();
    let req = RunSpellRequest {
        name: "init".into(),
        task: seed_task,
        workspace_dir: Some(new_cwd.to_string()),
        workspace_id: Some(workspace_id.to_string()),
        caller_agent_id: None,
        thread_id: Some(thread_id.to_string()),
    };
    match run_spell(State(state.clone()), Json(req)).await {
        Ok(Json(resp)) => {
            // Hand the killed orchestrator's unanswered user messages to the
            // fresh one (new agent id) so the first message that triggered the
            // rename isn't stranded on a dead inbox.
            if let Some(orch) = resp.agents.iter().find(|a| a.role == "orchestrator") {
                match state
                    .store
                    .reassign_unread_user_messages(killed_ids.clone(), orch.agent_id.clone())
                    .await
                {
                    Ok(n) if n > 0 => tracing::info!(
                        thread = %thread_id, moved = n, new_orch = %orch.agent_id,
                        "re-root: moved unread user messages to the new orchestrator"
                    ),
                    Ok(_) => {}
                    Err(e) => tracing::warn!(
                        ?e, thread = %thread_id,
                        "re-root: reassigning unread user messages failed"
                    ),
                }
            }
        }
        Err((status, _)) => {
            tracing::warn!(
                %status, thread = %thread_id,
                "re-root orchestrator after isolation failed (revive on demand)"
            );
        }
    }
}

/// `DELETE /api/workspaces/:id/threads/:tid` — soft-delete a direction (its
/// slug becomes reusable). A git worktree, if any, is removed best-effort in
/// the background. The `main` direction cannot be deleted.
pub async fn delete_thread_handler(
    State(state): State<AppState>,
    Path((workspace_id, thread_id)): Path<(String, String)>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    let thread = state
        .store
        .get_thread(thread_id.clone())
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
        })?
        .filter(|t| t.workspace_id == workspace_id)
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(json!({"error": format!("unknown thread: {thread_id}")})),
            )
        })?;
    if thread.slug == "main" {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "the main direction cannot be deleted"})),
        ));
    }
    // Kill the direction's live agents FIRST. Otherwise, once the thread row is
    // gone, those agents are orphaned: P4's strict thread-scoping hides them
    // from chat/DAG/members so the user can't kill them, and `git worktree
    // remove --force` below would yank a still-running agent's cwd out from
    // under it. (Workspace delete deliberately does NOT kill its agents — a
    // direction is the opposite: its agents have nowhere left to live.)
    match state.store.list_agents().await {
        Ok(rows) => {
            for a in rows {
                if a.thread_id.as_deref() == Some(thread_id.as_str())
                    && a.killed_at.is_none()
                    && a.shim_exit_at.is_none()
                {
                    teardown_agent(&state, &a.id).await;
                }
            }
        }
        Err(e) => {
            // Don't soft-delete + force-remove the worktree while we may have
            // failed to kill live agents in it — fail loud instead of orphaning.
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("could not enumerate agents to stop before delete: {e}")})),
            ));
        }
    }
    state
        .store
        .soft_delete_thread(thread_id.clone(), now_ms())
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
        })?;
    publish_thread_changed(&state, &workspace_id, &thread_id, "deleted");

    // Drop the direction's blackboard ledgers (`<ws>/<slug>/…`) so they don't
    // orphan in the panel once the slug is gone. Applies to shared directions
    // too (they still wrote ledgers under the prefix). Best-effort: DB rows
    // first, then the on-disk dir — the notify watcher just sees a removal.
    let bb_prefix = format!("{}/{}", workspace_id, thread.slug);
    if let Err(e) = state
        .store
        .delete_blackboard_prefix(bb_prefix.clone())
        .await
    {
        tracing::warn!(?e, prefix = %bb_prefix, "failed to delete direction blackboard ops");
    }
    let bb_dir = state.blackboard_root.join(&workspace_id).join(&thread.slug);
    let _ = tokio::task::spawn_blocking(move || {
        let _ = std::fs::remove_dir_all(&bb_dir);
    })
    .await;

    // Best-effort worktree cleanup. repo = the workspace's primary cwd; dest =
    // the thread's worktree dir.
    if thread.isolation == "worktree" {
        if let Ok(Some(ws)) = state.store.get_workspace_by_id(workspace_id).await {
            let repo = ws.cwd.clone();
            let dest = thread.cwd.clone();
            tokio::spawn(async move {
                let _ = tokio::task::spawn_blocking(move || {
                    crate::worktree::worktree_remove(
                        std::path::Path::new(&repo),
                        std::path::Path::new(&dest),
                    )
                })
                .await;
            });
        }
    }
    Ok(StatusCode::NO_CONTENT)
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

/// Re-derive and rewrite the workspace's flockmux-managed deps context
/// block from the current set of attached roots. Call this after any
/// add/delete so CLAUDE.md / AGENTS.md stay in sync (and the block is
/// stripped once the last root is removed). Best-effort: store errors are
/// logged and swallowed — the membership change already committed and the
/// context file is advisory, never load-bearing.
async fn refresh_workspace_deps_context(state: &AppState, workspace_id: &str) {
    let ws = match state.store.get_workspace_by_id(workspace_id.to_string()).await {
        Ok(Some(ws)) => ws,
        Ok(None) => return,
        Err(e) => {
            tracing::warn!(?e, ws_id = %workspace_id, "refresh deps context: get_workspace_by_id failed");
            return;
        }
    };
    // Don't touch the context file of a soft-deleted workspace.
    if ws.deleted_at.is_some() {
        return;
    }
    let roots: Vec<WorkspaceRoot> = match state
        .store
        .list_workspace_roots(workspace_id.to_string())
        .await
    {
        Ok(rows) => rows
            .into_iter()
            .map(|r| WorkspaceRoot {
                id: r.id,
                path: r.path,
                role: r.role,
                label: r.label,
                parent_id: r.parent_id,
                branch: None, // deps-context writer ignores branch
            })
            .collect(),
        Err(e) => {
            tracing::warn!(?e, ws_id = %workspace_id, "refresh deps context: list_workspace_roots failed");
            return;
        }
    };
    write_workspace_deps_context(ws.cwd.trim(), &ws.name, &roots);
}

/// Write the managed deps-context block (CLAUDE.md / AGENTS.md) into an
/// ARBITRARY directory — used when a direction is isolated into a git worktree.
/// The worktree is a copy of the cwd repo ONLY; the peer/dependency roots live
/// at their original absolute paths and are NOT carried in. Without re-emitting
/// the context here, the re-rooted orchestrator's new cwd (the worktree) has no
/// record that the peer projects exist, so it loses sight of repos the user may
/// actually be asking it to work on. We list the worktree as the primary and the
/// peers at their real paths (still readable), restoring multi-root visibility.
async fn write_deps_context_into_dir(state: &AppState, workspace_id: &str, target_dir: &str) {
    let ws = match state.store.get_workspace_by_id(workspace_id.to_string()).await {
        Ok(Some(ws)) => ws,
        _ => return,
    };
    if ws.deleted_at.is_some() {
        return;
    }
    let roots: Vec<WorkspaceRoot> = match state
        .store
        .list_workspace_roots(workspace_id.to_string())
        .await
    {
        Ok(rows) => rows
            .into_iter()
            .map(|r| WorkspaceRoot {
                id: r.id,
                path: r.path,
                role: r.role,
                label: r.label,
                parent_id: r.parent_id,
                branch: None,
            })
            .collect(),
        Err(e) => {
            tracing::warn!(?e, ws_id = %workspace_id, "worktree deps context: list_workspace_roots failed");
            return;
        }
    };
    // No peer/dependency roots → nothing worth re-emitting in the worktree.
    if roots.is_empty() {
        return;
    }
    write_workspace_deps_context(target_dir, &ws.name, &roots);
}

/// `POST /api/workspaces/:id/roots` — attach a dependency-source root to an
/// existing workspace. Mirrors the per-root validation in
/// `create_workspace_handler` (exists + is a dir → 4xx) and rejects
/// duplicates already attached to this workspace. On success the managed
/// context block in CLAUDE.md / AGENTS.md is refreshed.
pub async fn add_workspace_root_handler(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<WorkspaceRoot>,
) -> Result<Json<WorkspaceRoot>, (StatusCode, Json<serde_json::Value>)> {
    // 404 if the workspace is missing or soft-deleted.
    let ws = state
        .store
        .get_workspace_by_id(id.clone())
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
        })?
        .filter(|ws| ws.deleted_at.is_none())
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(json!({"error": format!("workspace {id} not found")})),
            )
        })?;

    let path = req.path.trim();
    if path.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "dependency path must be non-empty"})),
        ));
    }
    {
        let p = std::path::Path::new(path);
        if !p.exists() {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({"error": format!("dependency directory does not exist: {}", path)})),
            ));
        }
        if !p.is_dir() {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({"error": format!("dependency path is not a directory: {}", path)})),
            ));
        }
    }

    // Reject a duplicate already attached to this workspace.
    let existing = state
        .store
        .list_workspace_roots(ws.id.clone())
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
        })?;
    if existing.iter().any(|r| r.path == path) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": format!("dependency already attached: {}", path)})),
        ));
    }

    // If a parent was supplied, it must be an existing node in THIS
    // workspace's tree. A parent in another workspace (or a stale id) is a
    // client bug — 400. A genuinely-missing id is a 404.
    let parent_id = match req.parent_id.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(pid) => {
            let parent = state
                .store
                .get_workspace_root(pid.to_string())
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
                        Json(json!({"error": format!("parent root {pid} not found")})),
                    )
                })?;
            if parent.workspace_id != ws.id {
                return Err((
                    StatusCode::BAD_REQUEST,
                    Json(json!({"error": format!(
                        "parent root {pid} belongs to a different workspace"
                    )})),
                ));
            }
            Some(pid.to_string())
        }
        None => None,
    };

    let saved = state
        .store
        .add_workspace_root(
            NewWorkspaceRoot {
                workspace_id: ws.id.clone(),
                path: path.to_string(),
                role: req.role,
                label: req.label,
                parent_id,
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

    refresh_workspace_deps_context(&state, &id).await;

    Ok(Json(WorkspaceRoot {
        id: saved.id,
        path: saved.path,
        role: saved.role,
        label: saved.label,
        parent_id: saved.parent_id,
        branch: None, // filled on the next workspaces list refetch
    }))
}

/// `DELETE /api/workspaces/:id/roots?id=<root_id>` — detach a node from the
/// workspace's logical tree, CASCADING to all of its descendants. The node id
/// rides in the query string (DELETE has no body in the frontend's fetch).
/// Refreshes the managed context block afterwards (stripping it if this
/// removed the last node).
pub async fn delete_workspace_root_handler(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let root_id = match params.get("id").map(|s| s.trim()).filter(|s| !s.is_empty()) {
        Some(p) => p.to_string(),
        None => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "missing required query param 'id'"})),
            ))
        }
    };

    let n = state
        .store
        .delete_workspace_root(id.clone(), root_id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
        })?;

    refresh_workspace_deps_context(&state, &id).await;

    Ok(Json(json!({"deleted": n})))
}

/// `GET /api/workspaces/:id/root-suggestions[?path=<dir>]` — scan a project
/// dir for manifest-declared LOCAL PATH dependencies (package.json file:/link:,
/// Cargo.toml path deps, go.mod replace directives, pyproject.toml uv sources)
/// and return them as attachable root suggestions. `?path=` selects which dir
/// to scan (e.g. a peer project's dir when adding a child under it); it
/// defaults to the workspace's primary `cwd`. Best-effort: parse errors and
/// missing files are swallowed — this only ever feeds an optional picker.
/// Excludes the scanned dir itself and any path already attached anywhere in
/// the workspace.
pub async fn suggest_workspace_roots_handler(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Json<Vec<WorkspaceRoot>> {
    let ws = match state.store.get_workspace_by_id(id.clone()).await {
        Ok(Some(ws)) => ws,
        _ => return Json(Vec::new()),
    };
    // Scan the dir named by ?path= (a specific node's project dir) or fall
    // back to the workspace's primary cwd.
    let scan_dir = params
        .get("path")
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| ws.cwd.trim())
        .to_string();
    let cwd = std::path::Path::new(&scan_dir);

    // Canonical cwd (used to exclude the project itself from suggestions).
    let cwd_canon = std::fs::canonicalize(cwd).ok();

    // Canonical set of already-attached roots — suggestions never repeat
    // what's mounted. We canonicalize each so a `./foo` vs `/abs/foo`
    // mismatch still dedups.
    let mut already: std::collections::HashSet<std::path::PathBuf> =
        std::collections::HashSet::new();
    if let Ok(rows) = state.store.list_workspace_roots(id.clone()).await {
        for r in rows {
            if let Ok(c) = std::fs::canonicalize(&r.path) {
                already.insert(c);
            }
        }
    }

    // (relative-or-abs path string from the manifest, label) pairs.
    let mut candidates: Vec<(String, String)> = Vec::new();

    // package.json — dependencies / devDependencies values starting with
    // `file:` or `link:` point at a local path.
    if let Ok(txt) = std::fs::read_to_string(cwd.join("package.json")) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&txt) {
            for section in ["dependencies", "devDependencies"] {
                if let Some(map) = v.get(section).and_then(|s| s.as_object()) {
                    for (name, val) in map {
                        if let Some(spec) = val.as_str() {
                            for prefix in ["file:", "link:"] {
                                if let Some(rest) = spec.strip_prefix(prefix) {
                                    candidates.push((rest.to_string(), name.clone()));
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Cargo.toml — line scan for inline `path = "..."` (covers both
    // `name = { path = "..." }` and a `path = "..."` line inside a
    // `[dependencies.name]` table). Label is best-effort: the crate name
    // to the left of `=` if present, else the path basename.
    if let Ok(txt) = std::fs::read_to_string(cwd.join("Cargo.toml")) {
        for line in txt.lines() {
            let trimmed = line.trim();
            if let Some(rel) = extract_quoted_after(trimmed, "path") {
                let name = trimmed
                    .split('=')
                    .next()
                    .map(|s| s.trim().trim_matches(['{', ' ']))
                    .filter(|s| !s.is_empty() && !s.starts_with('['))
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| basename_of(&rel));
                candidates.push((rel, name));
            }
        }
    }

    // go.mod — `replace <module> => <target> [version]` where the target is
    // a local path (./ ../ or absolute). Label = path basename.
    if let Ok(txt) = std::fs::read_to_string(cwd.join("go.mod")) {
        for line in txt.lines() {
            let trimmed = line.trim();
            let body = trimmed.strip_prefix("replace ").unwrap_or(trimmed);
            if let Some((_, rhs)) = body.split_once("=>") {
                if let Some(target) = rhs.split_whitespace().next() {
                    if target.starts_with("./")
                        || target.starts_with("../")
                        || target.starts_with('/')
                    {
                        candidates.push((target.to_string(), basename_of(target)));
                    }
                }
            }
        }
    }

    // pyproject.toml — under `[tool.uv.sources]`, lines of the form
    // `name = { path = "..." }`. Label = name (else basename).
    if let Ok(txt) = std::fs::read_to_string(cwd.join("pyproject.toml")) {
        let mut in_uv_sources = false;
        for line in txt.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with('[') {
                in_uv_sources = trimmed == "[tool.uv.sources]";
                continue;
            }
            if in_uv_sources {
                if let Some(rel) = extract_quoted_after(trimmed, "path") {
                    let name = trimmed
                        .split('=')
                        .next()
                        .map(|s| s.trim())
                        .filter(|s| !s.is_empty())
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| basename_of(&rel));
                    candidates.push((rel, name));
                }
            }
        }
    }

    // pom.xml — Maven deps are jar coordinates (groupId/artifactId/version),
    // not local paths, so we can't read a path out of the manifest. Instead we
    // LOCATE local Maven projects on disk whose own `artifactId` matches a
    // declared dependency, covering the two common local layouts. Only runs
    // when the scanned dir actually has a pom.xml. Candidates are pushed as
    // absolute paths so they flow through the same canonicalize/exclude/dedup
    // pipeline below as every other ecosystem.
    if let Ok(pom) = std::fs::read_to_string(cwd.join("pom.xml")) {
        // (1) Multi-module reactor: each <module>REL</module> is a local
        // subdir scanDir/REL. Suggest it if scanDir/REL/pom.xml exists. Label
        // = the module's own artifactId if cheaply available, else REL.
        for rel in xml_tag_values(&pom, "module") {
            let module_dir = cwd.join(&rel);
            let module_pom = module_dir.join("pom.xml");
            if module_pom.is_file() {
                let label = std::fs::read_to_string(&module_pom)
                    .ok()
                    .and_then(|m| own_artifact_id(&m))
                    .unwrap_or(rel);
                candidates.push((module_dir.to_string_lossy().into_owned(), label));
            }
        }

        // (2) Sibling projects checked out next to this one. Collect every
        // <artifactId> referenced anywhere in the scanned pom (over-collecting
        // our own/parent/plugin ids is fine — they just won't match a real
        // sibling project, or if they do the user simply won't click). Then
        // scan the parent dir's immediate children for Maven projects whose
        // OWN artifactId is in that referenced set.
        let referenced: std::collections::HashSet<String> =
            xml_tag_values(&pom, "artifactId").into_iter().collect();
        if !referenced.is_empty() {
            if let Some(parent) = cwd.parent() {
                if let Ok(entries) = std::fs::read_dir(parent) {
                    // Bound the scan so a huge parent dir can't blow up the
                    // request — only the first 200 child dirs are considered.
                    for entry in entries.flatten().take(200) {
                        let child = entry.path();
                        if !child.is_dir() {
                            continue;
                        }
                        let name = entry.file_name();
                        let name = name.to_string_lossy();
                        // Skip the scanned dir itself, hidden dirs, and the
                        // usual build/vendor noise.
                        if name.starts_with('.')
                            || name == "target"
                            || name == "node_modules"
                        {
                            continue;
                        }
                        // The scanned dir itself is among these children but is
                        // excluded downstream by the cwd_canon check, so we
                        // needn't special-case it here.
                        let child_pom = child.join("pom.xml");
                        if !child_pom.is_file() {
                            continue;
                        }
                        if let Ok(child_xml) = std::fs::read_to_string(&child_pom) {
                            if let Some(aid) = own_artifact_id(&child_xml) {
                                if referenced.contains(&aid) {
                                    candidates
                                        .push((child.to_string_lossy().into_owned(), aid));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Resolve each candidate relative to cwd, canonicalize, keep only
    // existing dirs, drop the cwd itself + already-attached + dupes.
    let mut seen: std::collections::HashSet<std::path::PathBuf> =
        std::collections::HashSet::new();
    let mut out: Vec<WorkspaceRoot> = Vec::new();
    for (rel, label) in candidates {
        let raw = std::path::Path::new(&rel);
        let joined = if raw.is_absolute() {
            raw.to_path_buf()
        } else {
            cwd.join(raw)
        };
        let canon = match std::fs::canonicalize(&joined) {
            Ok(c) => c,
            Err(_) => continue,
        };
        if !canon.is_dir() {
            continue;
        }
        if Some(&canon) == cwd_canon.as_ref() {
            continue;
        }
        if already.contains(&canon) {
            continue;
        }
        if !seen.insert(canon.clone()) {
            continue;
        }
        out.push(WorkspaceRoot {
            id: String::new(),
            path: canon.to_string_lossy().into_owned(),
            role: "dependency".to_string(),
            label: Some(label),
            parent_id: None,
            branch: None, // suggestion only — not attached yet
        });
    }

    Json(out)
}

/// Pull the first `"..."`-quoted value that follows `<key>` (optionally with
/// `=`) on a single manifest line, e.g. `extract_quoted_after("foo = { path
/// = \"../bar\" }", "path")` → `Some("../bar")`. Returns `None` if the key or
/// a quoted value isn't present. Deliberately simple — these are best-effort
/// suggestion parsers, not a TOML implementation.
fn extract_quoted_after(line: &str, key: &str) -> Option<String> {
    let idx = line.find(key)?;
    let after_key = &line[idx + key.len()..];
    // Require an `=` between the key and the opening quote so we don't match
    // e.g. a `paths = [...]` array as a single path.
    let eq = after_key.find('=')?;
    let after_eq = &after_key[eq + 1..];
    let start = after_eq.find('"')? + 1;
    let rest = &after_eq[start..];
    let end = rest.find('"')?;
    let val = &rest[..end];
    if val.is_empty() {
        None
    } else {
        Some(val.to_string())
    }
}

/// Last path component of a path string, used as a fallback dependency label.
fn basename_of(p: &str) -> String {
    std::path::Path::new(p)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.to_string())
}

/// Crude XML scan: return the trimmed inner text of every `<tag>...</tag>`
/// occurrence in `xml`. Used for Maven pom.xml `<artifactId>` and `<module>`
/// extraction. Deliberately not a real XML parser — these are best-effort
/// suggestion inputs, so namespaces, comments, and attributes are ignored.
fn xml_tag_values(xml: &str, tag: &str) -> Vec<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(&open) {
        let after = &rest[start + open.len()..];
        let Some(end) = after.find(&close) else { break };
        let val = after[..end].trim();
        if !val.is_empty() {
            out.push(val.to_string());
        }
        rest = &after[end + close.len()..];
    }
    out
}

/// Extract a Maven pom's OWN `artifactId` (not its parent's). A `<parent>`
/// block carries its own `<artifactId>`; to skip it we start searching after
/// the first `</parent>` (if any), else from the start, then take the first
/// `<artifactId>...</artifactId>`.
fn own_artifact_id(xml: &str) -> Option<String> {
    let search_from = xml
        .find("</parent>")
        .map(|i| i + "</parent>".len())
        .unwrap_or(0);
    xml_tag_values(&xml[search_from..], "artifactId")
        .into_iter()
        .next()
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

    // Resolve the direction (thread) this spell runs in. Precedence:
    //   1. explicit `req.thread_id` — a UI launcher targeting a direction;
    //   2. the caller's own thread — a sub-spell inherits the orchestrator's;
    //   3. the workspace's main thread — oldest row, auto-created at creation;
    //   4. None — a legacy workspace with no thread rows (= main, cwd = ws.cwd).
    // The resolved thread drives (a) the cwd for isolated directions, (b) the
    // `thread_id` stamped on every spawned agent, and (c) the `{thread_slug}`
    // blackboard prefix so two directions don't clobber each other's ledgers.
    let resolved_thread: Option<ThreadRecord> = {
        let explicit = req.thread_id.clone();
        let caller_tid = if explicit.is_some() {
            None
        } else if let Some(caller) = req.caller_agent_id.as_ref() {
            state
                .store
                .get_thread_id_for_agent(caller.clone())
                .await
                .ok()
                .flatten()
        } else {
            None
        };
        match explicit.or(caller_tid) {
            // `get_thread` returns deleted rows too ("caller checks"), and
            // `req.thread_id` is an untrusted wire field — reject a soft-deleted
            // or foreign-workspace thread. On rejection `resolved_thread` is
            // None, which the rest of the handler treats as the main direction.
            Some(tid) => state
                .store
                .get_thread(tid)
                .await
                .ok()
                .flatten()
                .filter(|t| t.deleted_at.is_none() && t.workspace_id == workspace.id),
            None => state
                .store
                .list_threads(workspace.id.clone())
                .await
                .ok()
                .and_then(|mut v| (!v.is_empty()).then(|| v.remove(0))),
        }
    };
    let thread_id: Option<String> = resolved_thread.as_ref().map(|t| t.id.clone());
    let thread_slug: String = resolved_thread
        .as_ref()
        .map(|t| t.slug.clone())
        .unwrap_or_else(|| "main".to_string());

    // Pick the workspace layout. For shared_workspace spells we use the
    // explicit `workspace_dir` if the client sent one (M6a UX: the
    // SpellsLauncher exposes a text input); otherwise default to the
    // workspace's `cwd` so the spell runs in the project the user picked
    // in CreateWizard. PerAgent spells get per-agent subdirs under
    // `workspaces_root` as before — cwd and workspace_id are orthogonal
    // (filesystem layer vs. UI grouping layer).
    let layout: WorkspaceLayout = if spell.manifest.shared_workspace {
        let dir = match resolved_thread.as_ref() {
            // An isolated direction runs in its own git worktree, full stop —
            // that copy is the whole reason the direction exists.
            Some(t) if t.isolation == "worktree" => PathBuf::from(&t.cwd),
            // Shared / main direction: preserve the M6a `workspace_dir` override
            // (SpellsLauncher text input), else the workspace's own cwd.
            _ => req
                .workspace_dir
                .as_deref()
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(&workspace.cwd)),
        };
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
            None, // spells don't carry a per-worker model overlay (yet)
            layout.clone(),
            workspace.id.clone(),
            Some(spell_run_id.clone()),
            thread_id.clone(),
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
        // F2: render {workspace_id}/{task} into depends_on the SAME way the
        // prompt is rendered, so a manifest can write a workspace-scoped key
        // (e.g. "{workspace_id}/api.done") and have it match the producer's
        // rendered handoff_signal — the wake match is exact-string, so an
        // un-substituted placeholder would silently never match. role_to_id
        // isn't built yet here (producers may not be spawned), so {<role>_id}
        // in depends_on can't be resolved; the lint below flags any survivor.
        let empty_roles: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        let rendered_deps: Vec<String> = resolved
            .depends_on
            .iter()
            .map(|d| spells::render_prompt(d, &req.task, &workspace_id, &thread_slug, &empty_roles))
            .collect();
        for d in &rendered_deps {
            if d.contains('{') && d.contains('}') {
                tracing::warn!(
                    role = %resolved.role, dep = %d,
                    "spell depends_on still has an unresolved {{...}} placeholder after \
                     {{workspace_id}}/{{task}} substitution — a typo, or a {{<role>_id}} ref \
                     (unsupported in depends_on: the producer's agent_id isn't known when deps \
                     are registered). Wake matching is exact-string and will miss this key."
                );
            }
        }
        crate::wake::register_wake_subs(
            &state.wake_subs,
            out.agent_id.clone(),
            rendered_deps,
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
        // Render the producer's signal the same way (F2) so a workspace-scoped
        // handoff_signal lines up with dependents' rendered depends_on.
        let handoff_signal = spells::render_prompt(
            &handoff_signal,
            &req.task,
            &workspace_id,
            &thread_slug,
            &empty_roles,
        );
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
        let prompt =
            spells::render_prompt(raw_prompt, &req.task, &workspace_id, &thread_slug, &role_to_id);
        // Bootstrap inject — shared with spawn_worker (see
        // spawn_bootstrap_inject). Spell agents inject the RENDERED prompt
        // ({task}/{workspace_id}/{<role>_id} substituted above) and pass the
        // role keys so a surviving placeholder is flagged in the log.
        spawn_bootstrap_inject(
            state.registry.clone(),
            out.lifecycle_rx.resubscribe(),
            out.agent_id.clone(),
            prompt,
            BootstrapCtx {
                source: "spell",
                spell: spell_name.clone(),
                role_keys: role_to_id.keys().cloned().collect(),
            },
            // Spell agents (today: the init orchestrator) carry no blackboard
            // deps → empty gate = inject immediately, unchanged behaviour. If a
            // future spell defines dep-bearing role agents, thread their
            // resolved keys here to gate them too.
            Vec::new(),
            state.swarm.clone(),
            now_ms(),
        );
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


#[cfg(test)]
mod p0_tests {
    use super::*;
    use crate::roles::RoleRegistry;
    use flockmux_protocol::rest::ConsumeRef;

    fn consume(from: &str, kind: &str) -> ConsumeRef {
        ConsumeRef { from_role: from.into(), kind: kind.into() }
    }

    #[test]
    fn resolve_consumes_happy_mints_scoped_keys() {
        let reg = RoleRegistry::builtin();
        let deps = resolve_consumes_to_deps(
            &reg,
            "reviewer",
            &[consume("backend", "done"), consume("frontend", "done")],
            "ws1",
            "dark-mode",
        )
        .unwrap();
        assert_eq!(
            deps,
            vec![
                "ws1/dark-mode/backend.done".to_string(),
                "ws1/dark-mode/frontend.done".to_string(),
            ]
        );
    }

    #[test]
    fn resolve_consumes_unknown_role_suggests_closest() {
        let reg = RoleRegistry::builtin();
        let err = resolve_consumes_to_deps(&reg, "reviewer", &[consume("bakend", "done")], "ws1", "main")
            .unwrap_err();
        assert!(err.contains("unknown role 'bakend'"), "got: {err}");
        assert!(err.contains("did you mean 'backend'"), "got: {err}");
    }

    #[test]
    fn resolve_consumes_rejects_kind_not_produced() {
        let reg = RoleRegistry::builtin();
        // builtin backend produces ["done"] only.
        let err = resolve_consumes_to_deps(&reg, "reviewer", &[consume("backend", "spec")], "ws1", "main")
            .unwrap_err();
        assert!(err.contains("does not produce kind 'spec'"), "got: {err}");
    }

    #[test]
    fn resolve_consumes_rejects_self_dependency() {
        let reg = RoleRegistry::builtin();
        let err = resolve_consumes_to_deps(&reg, "frontend", &[consume("frontend", "done")], "ws1", "main")
            .unwrap_err();
        assert!(err.contains("self-dependency"), "got: {err}");
    }

    #[test]
    fn resolve_consumes_rejects_empty_from_role() {
        let reg = RoleRegistry::builtin();
        let err = resolve_consumes_to_deps(&reg, "frontend", &[consume("", "done")], "ws1", "main")
            .unwrap_err();
        assert!(err.contains("empty from_role"), "got: {err}");
    }

    #[test]
    fn closest_match_within_threshold_only() {
        let cands = vec!["frontend".to_string(), "backend".to_string(), "reviewer".to_string()];
        assert_eq!(closest_match("fronend", &cands).as_deref(), Some("frontend"));
        // Garbage far from everything → no suggestion.
        assert_eq!(closest_match("zzzzzzzz", &cands), None);
    }

    #[test]
    fn build_worker_prompt_injects_minted_keys_and_error_branch() {
        let p = build_worker_prompt(
            "do the thing",
            &["ws1/main/frontend.done".to_string()],
            "ws1/main/frontend.done.error",
            &[],
        );
        assert!(p.starts_with("do the thing"));
        assert!(p.contains("ws1/main/frontend.done"));
        assert!(p.contains("ws1/main/frontend.done.error"));
        assert!(p.contains("VERBATIM"));
        assert!(!p.contains("INPUTS"), "no deps → no inputs gate");
        // No keys at all → prompt returned unchanged (fire-and-forget, no deps).
        assert_eq!(build_worker_prompt("x", &[], "x.error", &[]), "x");
    }

    #[test]
    fn readiness_gate_first_unsatisfied_dep() {
        use std::collections::HashSet;
        let deps = vec!["ws/main/backend.done".to_string(), "ws/main/db.ready".to_string()];

        // nothing present → first dep is unsatisfied
        let empty = HashSet::new();
        assert_eq!(
            first_unsatisfied_dep(&deps, &empty).as_deref(),
            Some("ws/main/backend.done")
        );

        // first present, second missing → returns the second
        let mut p1: HashSet<String> = HashSet::new();
        p1.insert("ws/main/backend.done".into());
        assert_eq!(
            first_unsatisfied_dep(&deps, &p1).as_deref(),
            Some("ws/main/db.ready")
        );

        // both present → all satisfied
        let mut p2 = p1.clone();
        p2.insert("ws/main/db.ready".into());
        assert_eq!(first_unsatisfied_dep(&deps, &p2), None);

        // a `.error` alias counts as satisfied (fail-loud: wake to handle it)
        let mut p3: HashSet<String> = HashSet::new();
        p3.insert("ws/main/backend.done.error".into());
        p3.insert("ws/main/db.ready.failed".into());
        assert_eq!(first_unsatisfied_dep(&deps, &p3), None);

        // empty deps → never blocks
        assert_eq!(first_unsatisfied_dep(&[], &empty), None);
    }

    #[test]
    fn build_worker_prompt_adds_inputs_wait_gate_when_deps_present() {
        let p = build_worker_prompt(
            "review it",
            &["ws1/main/reviewer.done".to_string()],
            "ws1/main/reviewer.done.error",
            &["ws1/main/backend.done".to_string()],
        );
        // The wait-gate must name the dep and forbid acting / writing handoff early.
        assert!(p.contains("INPUTS"));
        assert!(p.contains("ws1/main/backend.done"));
        assert!(p.contains("do NOT write your"));
        assert!(p.contains("STOP"));
        // Still carries its own handoff block.
        assert!(p.contains("ws1/main/reviewer.done"));
        // A dep-only worker with no produces still gets the inputs gate.
        let q = build_worker_prompt("x", &[], "x.error", &["ws1/main/dep.done".to_string()]);
        assert!(q.contains("INPUTS"));
        assert!(q.contains("ws1/main/dep.done"));
    }
}
