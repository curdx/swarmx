//! flockmux-server: axum-based HTTP/WS gateway.
//!
//! M1 surface:
//!   GET  /api/plugins          — list available CLI plugins
//!   POST /api/agent            — spawn an agent ({ cli: "claude"|"codex" })
//!   DELETE /api/agent/:id      — kill an agent
//!   WS   /ws/pty/:id           — bidirectional PTY bridge
//!
//! Bind: 127.0.0.1:7777 (loopback only, single-user local). There is no
//! token auth yet, but `require_local_origin` rejects any request carrying a
//! non-local `Origin` header — this is what stops a random web page the user
//! visits from driving their agents over a cross-site WebSocket (WS bypasses
//! CORS, so the CORS layer alone is not a security boundary).

mod acp;
mod models_config;
mod plugins;
mod pre_spawn;
mod pty_stream;
mod registry;
mod roles;
mod routes;
mod spawn;
mod spells;
mod transcript;
mod wake;
// Git worktree helpers for thread isolation, wired into the thread REST
// handlers (rename → background `worktree add`). `is_git_repo` stays as a
// documented helper even though the current flow relies on idempotent init.
mod worktree;

use anyhow::{Context, Result};
use axum::{
    extract::Request,
    http::{header, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, patch, post},
    Router,
};
use flockmux_storage::Store;
use flockmux_swarm::{Swarm, WatcherHandle};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tower::ServiceExt; // ServeDir::oneshot in the path-aware SPA fallback
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;
use tracing::info;

#[derive(Clone)]
pub struct AppState {
    pub plugins: Arc<plugins::PluginRegistry>,
    /// Loaded spell manifests. Optional — a fresh checkout may have no
    /// `spells/` directory and the server still starts. Spells are read
    /// once at startup; no hot-reload (each spell run is a fresh read of
    /// in-memory state).
    pub spells: Arc<spells::SpellRegistry>,
    /// Loaded role manifests from `roles/`. Same load-once-no-hot-reload
    /// semantics as spells. Empty registry is fine; spells without
    /// `role_ref` work without any roles loaded.
    pub roles: Arc<roles::RoleRegistry>,
    /// F1 per-CLI model config: maps abstract tiers → concrete model ids per
    /// CLI (user-editable via the 模型 settings page / `/api/models`). Read at
    /// the spawn chokepoint to turn a role/spawn tier into the right concrete
    /// model for whichever CLI is launching. `RwLock` so a PUT can hot-swap it.
    pub models: Arc<tokio::sync::RwLock<models_config::ModelConfig>>,
    pub registry: registry::Registry,
    pub shim_path: PathBuf,
    /// Absolute path to the `flockmux-mcp` binary. Baked into per-spawn
    /// MCP config entries so claude / codex launch the swarm bridge with
    /// no PATH lookup.
    pub mcp_bin: PathBuf,
    /// Base URL of this server's own REST API (loopback only today).
    /// Stamped into MCP entries so the subprocess knows where to talk.
    pub server_url: String,
    pub workspaces_root: PathBuf,
    pub store: Arc<Store>,
    pub swarm: Arc<Swarm>,
    pub blackboard_root: PathBuf,
    pub recordings_root: PathBuf,
    /// Keeps the notify-debouncer alive for the program's lifetime. Wrapped
    /// in `Arc` so `AppState` stays `Clone`. Drop terminates the watcher.
    pub _watcher: Arc<WatcherHandle>,
    /// M6b: per-agent `depends_on` table. `agent_id → blackboard keys this
    /// agent is waiting on`. Populated at spell launch, cleaned up at
    /// agent kill. The `WakeCoordinator` background task consults this on
    /// every `SwarmEvent::BlackboardChanged` and proactively wakes
    /// matching subscribers.
    pub wake_subs: wake::WakeSubs,
    /// M6c step 5: per-agent expected handoff signal. `agent_id → key
    /// this agent is *supposed* to write before it exits`. The
    /// `WakeCoordinator` watches `SwarmEvent::AgentState{Exited}` and,
    /// when the agent dies without producing this key, synthesizes a
    /// `<key>.error` write so downstream dependents can branch into
    /// their upstream-failed path instead of waiting forever.
    pub exit_keys: wake::ExitKeys,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,flockmux=debug".into()),
        )
        .init();

    // L5a: layered registry — bundled `cli-plugins/` first, then the user
    // override layer `~/.flockmux/cli-plugins/` (last-writer-wins by id). Lets
    // a user customize or add a CLI without forking the repo.
    let plugins_dir = plugins::default_plugins_dir();
    let mut plugin_layers = vec![plugins_dir.clone()];
    if let Some(user_dir) = plugins::user_plugins_dir() {
        plugin_layers.push(user_dir);
    }
    info!(layers = ?plugin_layers, "loading cli plugins (bundled + user override)");
    let plugin_registry = plugins::PluginRegistry::load_layered(&plugin_layers);
    info!(count = plugin_registry.list().len(), "plugins loaded");

    let spells_dir = spells::default_spells_dir();
    info!(dir = %spells_dir.display(), "loading spells");
    let spell_registry = spells::SpellRegistry::load_dir(&spells_dir)
        .with_context(|| format!("load spells from {}", spells_dir.display()))?;
    info!(count = spell_registry.list().len(), "spells loaded");

    // Two-layer role registry: built-ins compiled into the binary (so a
    // deployed server with no `roles/` dir still works) overlaid by the repo's
    // `roles/` dir (dev override by slug). Per-workspace `.flockmux/roles/`
    // overlay happens at spawn time, where the workspace cwd is known.
    let roles_dir = roles::default_roles_dir();
    info!(dir = %roles_dir.display(), "loading roles (builtin + dir overlay)");
    let mut role_registry = roles::RoleRegistry::builtin();
    match roles::RoleRegistry::load_dir(&roles_dir) {
        Ok(dir_roles) => role_registry.overlay(dir_roles),
        Err(e) => {
            tracing::warn!(?e, dir = %roles_dir.display(), "roles dir overlay failed; using builtins only")
        }
    }
    info!(count = role_registry.list().len(), "roles loaded");

    // F1: per-CLI model config (tier → concrete model). Shipped defaults if no
    // ~/.flockmux/models.json (≡ legacy behaviour). Hot-swapped on PUT /api/models.
    let model_config = models_config::load_or_default();
    info!(clis = model_config.clis.len(), "model config loaded");

    let shim_path = spawn::locate_shim().context("locate flockmux-shim")?;
    info!(shim = %shim_path.display(), "shim located");

    let mcp_bin = spawn::locate_mcp().context("locate flockmux-mcp")?;
    info!(mcp = %mcp_bin.display(), "mcp binary located");

    let workspaces_root = workspaces_root_default();
    std::fs::create_dir_all(&workspaces_root)?;
    info!(workspaces = %workspaces_root.display(), "workspaces root");

    let db_path = db_path_default();
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Single-instance gate. flockmux state is split across sqlite, the
    // blackboard fs tree, and per-spawn workspaces, all under the same
    // data dir. A second server racing the first's `mark_orphan_*` calls
    // will silently flip the first's live agents to killed in the DB
    // (in-memory state survives, but the next restart inherits the
    // corruption). Take an exclusive flock on a lockfile next to the
    // DB; the lock auto-releases when this process exits (fd close).
    acquire_singleton_lock(&db_path)?;

    info!(db = %db_path.display(), "opening sqlite store");
    let store = Arc::new(Store::open(&db_path).await.context("open store")?);
    // Any agent / recording left "live" in the DB belongs to a previous
    // server process that died before kill / finalize. Settle them so the
    // UI doesn't keep reattaching to dead PTYs or showing "● live" forever.
    let orphan_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    match store.mark_orphan_agents_killed(orphan_at).await {
        Ok(n) if n > 0 => info!(orphans = n, "settled orphan live agents"),
        Ok(_) => {}
        Err(err) => tracing::warn!(?err, "mark_orphan_agents_killed failed"),
    }
    match store.mark_orphan_recordings_finalized(orphan_at).await {
        Ok(n) if n > 0 => info!(orphans = n, "settled orphan live recordings"),
        Ok(_) => {}
        Err(err) => tracing::warn!(?err, "mark_orphan_recordings_finalized failed"),
    }

    let blackboard_root = blackboard_root_default();
    std::fs::create_dir_all(&blackboard_root)?;
    info!(blackboard = %blackboard_root.display(), "blackboard root");
    let swarm = Swarm::new(store.clone(), blackboard_root.clone());
    let watcher = WatcherHandle::spawn(blackboard_root.clone(), swarm.clone())
        .context("spawn blackboard watcher")?;
    let watcher = Arc::new(watcher);

    // F6 completion: backfill op-log rows for any blackboard file left on disk
    // without one by a prior crash mid-write, so it's visible to discovery
    // again. Cheap + idempotent (one query + inserts only for missing paths).
    match swarm.reconcile_oplog_from_disk().await {
        Ok(n) if n > 0 => info!(reconciled = n, "backfilled blackboard op rows missing from a prior crash"),
        Ok(_) => {}
        Err(err) => tracing::warn!(?err, "blackboard op-log reconcile failed"),
    }

    // F5 retention: the three append-only tables (blackboard_ops, messages,
    // pty_recordings) otherwise grow forever. Trim rows past the retention
    // window — but only ones that are no longer load-bearing (superseded
    // blackboard history, consumed wakes + read messages, old finalized
    // recordings). FLOCKMUX_RETENTION_DAYS=0 keeps everything. Runs once at
    // boot; never blocks startup on failure.
    if let Some(window_ms) = retention_window_ms() {
        let cutoff = orphan_at.saturating_sub(window_ms);
        match store.prune_expired(cutoff).await {
            Ok(stats) if !stats.is_empty() => info!(
                blackboard_ops = stats.blackboard_ops,
                messages = stats.messages,
                recordings = stats.recordings,
                cast_files = stats.recording_files_removed,
                "pruned expired rows past the retention window"
            ),
            Ok(_) => {}
            Err(err) => tracing::warn!(?err, "retention prune failed; continuing startup"),
        }
    }

    let recordings_root = recordings_root_default();
    std::fs::create_dir_all(&recordings_root)?;
    info!(recordings = %recordings_root.display(), "recordings root");

    // FLOCKMUX_PORT is parsed here (not just at bind time) because it also
    // drives the URL we bake into spawned agents' wake-check hook + swarm MCP.
    // A parallel instance set with only FLOCKMUX_PORT used to leave agents
    // pointing at the 7777 default (their messages then invisible to this
    // server). Derive SERVER_URL from PORT so PORT alone is sufficient;
    // an explicit FLOCKMUX_SERVER_URL still wins.
    let port: u16 = std::env::var("FLOCKMUX_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(7777);
    let server_url = std::env::var("FLOCKMUX_SERVER_URL")
        .unwrap_or_else(|_| format!("http://127.0.0.1:{port}"));

    let wake_subs: wake::WakeSubs =
        std::sync::Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new()));
    let exit_keys: wake::ExitKeys =
        std::sync::Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new()));

    let state = AppState {
        plugins: Arc::new(plugin_registry),
        spells: Arc::new(spell_registry),
        roles: Arc::new(role_registry),
        models: Arc::new(tokio::sync::RwLock::new(model_config)),
        registry: registry::Registry::new(),
        shim_path,
        mcp_bin,
        server_url,
        workspaces_root,
        store,
        swarm,
        blackboard_root,
        recordings_root,
        _watcher: watcher,
        wake_subs: wake_subs.clone(),
        exit_keys: exit_keys.clone(),
    };

    // M6b: launch the wake coordinator. Lives for the whole process; the
    // returned JoinHandle is intentionally dropped because the task exits
    // only when `state.swarm`'s broadcast closes (program shutdown).
    let _wake_handle = wake::WakeCoordinator::spawn(
        state.swarm.clone(),
        state.registry.clone(),
        wake_subs,
        exit_keys,
        state.store.clone(),
    );
    info!("wake coordinator started");

    let mut app = Router::new()
        .route("/api/plugins", get(routes::rest::list_plugins))
        // F1 模型设置页: per-CLI tier→concrete-model mapping (read/write).
        .route(
            "/api/models",
            get(routes::models_admin::get_models).put(routes::models_admin::put_models),
        )
        // MCP admin (「快捷装 MCP」页面): runtime probe + read/add/remove via the
        // CLIs' own `mcp` subcommands.
        .route("/api/mcp/env", get(routes::mcp_admin::mcp_env))
        .route("/api/mcp/status", get(routes::mcp_admin::mcp_status))
        .route("/api/mcp/install", post(routes::mcp_admin::mcp_install))
        .route("/api/mcp/uninstall", post(routes::mcp_admin::mcp_uninstall))
        .route(
            "/api/agent",
            get(routes::rest::list_agents).post(routes::rest::spawn),
        )
        .route("/api/worker", post(routes::rest::spawn_worker))
        .route("/api/roles", get(routes::rest::list_roles))
        .route("/api/agent/:id", delete(routes::rest::kill))
        .route("/api/agent/:id/wake", post(routes::rest::wake_agent))
        .route("/api/agent/:id/interrupt", post(routes::rest::interrupt))
        .route("/api/agent/:id/resume", post(routes::rest::resume))
        // Internal: the agent's own flockmux-mcp pings this when its tool list
        // is fetched (MCP ready), so the bootstrap can inject without a fixed wait.
        .route("/api/agent/:id/mcp-ready", post(routes::rest::mcp_ready))
        .route(
            "/api/agent/interrupt-all",
            post(routes::rest::interrupt_all),
        )
        .route(
            "/api/message",
            get(routes::swarm::list_messages).post(routes::swarm::send_message),
        )
        .route(
            "/api/message/read",
            post(routes::swarm::mark_messages_read),
        )
        .route(
            "/api/message/unread_count",
            get(routes::swarm::unread_count),
        )
        .route(
            "/api/message/consume_wakes",
            post(routes::swarm::consume_wakes),
        )
        .route(
            "/api/blackboard",
            get(routes::swarm::list_blackboard_paths),
        )
        .route(
            "/api/blackboard/*path",
            get(routes::swarm::read_blackboard).put(routes::swarm::write_blackboard),
        )
        .route(
            "/api/blackboard-history/*path",
            get(routes::swarm::blackboard_history),
        )
        .route(
            "/api/recording",
            get(routes::recording::list_recordings),
        )
        .route("/api/recording/:id", get(routes::recording::get_recording))
        .route(
            "/api/workspaces",
            get(routes::rest::list_workspaces_handler)
                .post(routes::rest::create_workspace_handler),
        )
        .route(
            "/api/workspaces/:id",
            delete(routes::rest::delete_workspace_handler),
        )
        .route(
            "/api/workspaces/:id/roots",
            post(routes::rest::add_workspace_root_handler)
                .delete(routes::rest::delete_workspace_root_handler),
        )
        .route(
            "/api/workspaces/:id/root-suggestions",
            get(routes::rest::suggest_workspace_roots_handler),
        )
        // Local branches of a workspace's repo — for the "open existing branch
        // as a direction" picker.
        .route(
            "/api/workspaces/:id/branches",
            get(routes::rest::list_branches_handler),
        )
        // Threads (directions) within a workspace.
        .route(
            "/api/workspaces/:id/threads",
            get(routes::rest::list_threads_handler).post(routes::rest::create_thread_handler),
        )
        .route(
            "/api/workspaces/:id/threads/:tid",
            patch(routes::rest::update_thread_handler).delete(routes::rest::delete_thread_handler),
        )
        .route(
            "/api/workspaces/:id/threads/:tid/diff",
            get(routes::rest::thread_diff_handler),
        )
        .route(
            "/api/workspaces/:id/threads/:tid/merge",
            post(routes::rest::merge_thread_handler),
        )
        .route("/api/spells", get(routes::rest::list_spells))
        .route("/api/spell/run", post(routes::rest::run_spell))
        .route("/ws/swarm", get(routes::ws_swarm::ws_swarm))
        .route("/ws/pty/:agent_id", get(routes::pty_ws::pty_ws));

    // Serve the built web bundle so `http://127.0.0.1:<port>` actually shows the
    // UI (apiBase.ts / require_local_origin already assume the server hosts it;
    // it was never wired → bare 404). The fallback serves static assets and, for
    // unknown client-routes, index.html (SPA history routing) — BUT a path under
    // /api or /ws must 404 cleanly instead of being swallowed into a 200
    // index.html (that would hand API callers HTML + a confusing JSON parse
    // error). No bundle on disk (a dev-only checkout) → stay API-only.
    if let Some(web_dir) = resolve_web_dir() {
        let serve = ServeDir::new(&web_dir).fallback(ServeFile::new(web_dir.join("index.html")));
        app = app.fallback(move |req: Request| {
            let serve = serve.clone();
            async move {
                let p = req.uri().path();
                if p.starts_with("/api/") || p.starts_with("/ws/") {
                    return StatusCode::NOT_FOUND.into_response();
                }
                serve.oneshot(req).await.into_response()
            }
        });
        info!(web = %web_dir.display(), "serving web bundle");
    } else {
        info!("no web bundle on disk; API-only (serve UI via `npm run dev` or Tauri)");
    }

    let app = app
        // CORS stays permissive so the Tauri webview's cross-origin fetch
        // (tauri://localhost → 127.0.0.1:7777) keeps working. The actual
        // security boundary is `require_local_origin` below, which rejects
        // any request whose Origin isn't a local host — including the
        // cross-site WebSocket upgrades that CORS does NOT cover.
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        // Outermost layer (added last ⇒ runs first): drop cross-site
        // requests before they reach any handler.
        .layer(middleware::from_fn(require_local_origin))
        .with_state(state.clone());

    // `port` parsed above (it also derives server_url). Bind loopback only.
    let addr: SocketAddr = ([127, 0, 0, 1], port).into();
    info!(%addr, "flockmux-server listening (loopback only; cross-origin requests rejected)");
    let listener = tokio::net::TcpListener::bind(addr).await?;

    // Magentic-One restart resilience: for every alive workspace whose
    // orchestrator was settled-as-killed by the orphan sweep above,
    // re-spawn it. The orchestrator's prompt detects an existing
    // `task.ledger.md` on the blackboard and short-circuits Phase A
    // (no re-scan, no duplicate greeting) — so the user perceives the
    // restart as a brief PTY blip, ledger and task progress survive.
    //
    // Done as tokio::spawn so it doesn't delay axum::serve. The
    // orchestrator role is loaded in spell_registry above; if the
    // user has deleted it or pinned to a different role, the spell
    // run fails loudly and the user can re-create the workspace.
    tokio::spawn({
        let respawn_state = state.clone();
        async move {
            if let Err(err) = auto_respawn_orchestrators(&respawn_state).await {
                tracing::warn!(?err, "auto-respawn orchestrators failed");
            }
        }
    });

    axum::serve(listener, app).await?;
    Ok(())
}

/// 遍历所有未删 workspace,对没活的 orchestrator 的 workspace 拉一个新的
/// (走 init spell 路径)。直接调 `routes::rest::run_spell` 不绕 HTTP,因为
/// state 已经持有所有必要句柄(spell registry / role registry / store /
/// registry / swarm / wake / exit_keys)。
///
/// 调用时机:server 启动 + orphan settle 之后,axum 开始 serve 之后(用
/// `tokio::spawn` 异步并发,不阻塞 listen)。
async fn auto_respawn_orchestrators(state: &AppState) -> Result<()> {
    use axum::extract::{Json as AxJson, State};
    use flockmux_protocol::rest::RunSpellRequest;

    let workspaces = state
        .store
        .list_workspaces(false)
        .await
        .context("list_workspaces")?;
    if workspaces.is_empty() {
        info!("auto-respawn: no workspaces, skipping");
        return Ok(());
    }

    let agents = state.store.list_agents().await.context("list_agents")?;

    let mut respawned = 0usize;
    let mut skipped = 0usize;
    for ws in &workspaces {
        let has_live_orchestrator = agents.iter().any(|a| {
            a.workspace_id.as_deref() == Some(ws.id.as_str())
                && a.role == "orchestrator"
                && a.killed_at.is_none()
        });
        if has_live_orchestrator {
            skipped += 1;
            continue;
        }

        // Don't respawn into a directory that no longer exists — the init
        // spell's "create shared workspace" step would just fail and log
        // noise. The workspace row stays (user can delete it from the UI).
        if !std::path::Path::new(&ws.cwd).is_dir() {
            skipped += 1;
            info!(workspace_id = %ws.id, "auto-respawn: cwd missing, skipping");
            continue;
        }

        // Don't respawn a FINISHED workspace on boot. Re-spawning runs the
        // init spell, which injects a bootstrap prompt and burns a full LLM
        // turn — for a completed task that just re-concludes "nothing to do",
        // wasting subscription budget on every server restart. The progress
        // ledger carries an `all_done` marker once the orchestrator wrapped
        // up; if we see it, leave the workspace member-less. The UI's
        // 0-member chat state offers a "wake orchestrator" button that runs
        // init on demand when the user actually returns to the project.
        //
        // Auto-respawn revives the MAIN direction, whose ledger now lives at
        // `{ws}/main/progress.ledger.md` (per-direction prefix). Also check the
        // legacy `{ws}/progress.ledger.md` so workspaces completed before the
        // thread-prefix migration still skip (one-time; regenerates on wake).
        let main_key = format!("{}/main/progress.ledger.md", ws.id);
        let legacy_key = format!("{}/progress.ledger.md", ws.id);
        let mut all_done = false;
        for key in [&main_key, &legacy_key] {
            if let Ok(Some(content)) = state.swarm.read_blackboard(key).await {
                if content.contains("all_done") {
                    all_done = true;
                    break;
                }
            }
        }
        if all_done {
            skipped += 1;
            info!(workspace_id = %ws.id, "auto-respawn: task all_done, skipping (revive on demand)");
            continue;
        }
        let req = RunSpellRequest {
            name: "init".into(),
            task: String::new(),
            workspace_dir: Some(ws.cwd.clone()),
            workspace_id: Some(ws.id.clone()),
            caller_agent_id: None,
            // Auto-respawn revives the orchestrator on the workspace's main
            // thread; run_spell resolves it (oldest thread) when this is None.
            thread_id: None,
        };
        match routes::rest::run_spell(State(state.clone()), AxJson(req)).await {
            Ok(resp) => {
                respawned += 1;
                info!(
                    workspace_id = %ws.id,
                    workspace_name = %ws.name,
                    spawned = resp.0.agents.len(),
                    "auto-respawn: orchestrator re-spawned"
                );
            }
            Err((status, body)) => {
                tracing::warn!(
                    workspace_id = %ws.id,
                    %status,
                    body = %body.0,
                    "auto-respawn: init spell failed"
                );
            }
        }
    }
    info!(
        respawned,
        skipped,
        total = workspaces.len(),
        "auto-respawn: complete"
    );
    Ok(())
}

fn workspaces_root_default() -> PathBuf {
    if let Ok(p) = std::env::var("FLOCKMUX_WORKSPACES_DIR") {
        return PathBuf::from(p);
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".flockmux").join("workspaces");
    }
    PathBuf::from(".flockmux/workspaces")
}

/// Retention window in ms from `FLOCKMUX_RETENTION_DAYS` (default 30 days).
/// Returns `None` when the var is set to 0 or negative — "keep everything,
/// never prune". A non-numeric value falls back to the default.
fn retention_window_ms() -> Option<i64> {
    let days: i64 = std::env::var("FLOCKMUX_RETENTION_DAYS")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(30);
    (days > 0).then(|| days * 24 * 60 * 60 * 1000)
}

fn db_path_default() -> PathBuf {
    if let Ok(p) = std::env::var("FLOCKMUX_DB_PATH") {
        return PathBuf::from(p);
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".flockmux").join("flockmux.db");
    }
    PathBuf::from(".flockmux/flockmux.db")
}

fn blackboard_root_default() -> PathBuf {
    if let Ok(p) = std::env::var("FLOCKMUX_BLACKBOARD_DIR") {
        return PathBuf::from(p);
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".flockmux").join("blackboard");
    }
    PathBuf::from(".flockmux/blackboard")
}

fn recordings_root_default() -> PathBuf {
    if let Ok(p) = std::env::var("FLOCKMUX_RECORDINGS_DIR") {
        return PathBuf::from(p);
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".flockmux").join("recordings");
    }
    PathBuf::from(".flockmux/recordings")
}

/// Try to acquire an exclusive non-blocking flock on `<db_dir>/server.lock`.
/// The fd is intentionally leaked so the lock outlives this function — it is
/// released by the kernel when the process exits, which is exactly the
/// "lock until shutdown" semantic we want. We also stamp the holding PID
/// into the lockfile body so users running `cat ~/.flockmux/server.lock`
/// can see who owns it.
///
/// On contention we exit(2) with a multi-line error pointing the user at
/// the holding pid and a remediation command. We do this *before* opening
/// the sqlite store so a refused boot leaves zero footprint.
fn acquire_singleton_lock(db_path: &std::path::Path) -> Result<()> {
    use fs2::FileExt;
    use std::io::{Read, Write};

    let lock_path = db_path
        .parent()
        .map(|p| p.join("server.lock"))
        .unwrap_or_else(|| PathBuf::from(".flockmux-server.lock"));

    let mut lock_file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .with_context(|| format!("open lockfile {}", lock_path.display()))?;

    if let Err(e) = lock_file.try_lock_exclusive() {
        let mut holder_pid = String::new();
        let _ = lock_file.read_to_string(&mut holder_pid);
        let holder_pid = holder_pid.trim();
        eprintln!();
        eprintln!("✗ flockmux-server: another instance is already running.");
        eprintln!();
        eprintln!("  Lock file : {}", lock_path.display());
        if !holder_pid.is_empty() {
            eprintln!("  Holder PID: {holder_pid}");
        }
        eprintln!("  Reason    : {e}");
        eprintln!();
        eprintln!("  Two instances share ~/.flockmux (sqlite, blackboard, workspaces)");
        eprintln!("  and would corrupt each other's state. Stop the existing process");
        eprintln!("  first, e.g.:");
        if !holder_pid.is_empty() {
            eprintln!("    kill {holder_pid}");
        } else {
            eprintln!("    pkill -f flockmux-server");
        }
        eprintln!();
        std::process::exit(2);
    }

    // Lock acquired — rewrite the body with our PID for the next contender.
    let _ = lock_file.set_len(0);
    let _ = lock_file.write_all(format!("{}\n", std::process::id()).as_bytes());

    // Move ownership into a Box::leak so the fd stays open for the rest
    // of the process. Dropping the File would release the lock prematurely
    // if the function returned without this.
    Box::leak(Box::new(lock_file));
    info!(lock = %lock_path.display(), "acquired singleton lock");
    Ok(())
}

/// Locate the built web bundle (`dist/` with an `index.html`) to serve at the
/// server root. Resolution order: `FLOCKMUX_WEB_DIR` (explicit, e.g. the Tauri
/// bundle's resource dir) → `<cwd>/web/dist` or `<cwd>/dist` (running from the
/// repo) → next to / two levels up from the executable (`target/release` →
/// repo `web/dist`). Returns `None` when nothing is found, so a bare dev
/// checkout stays API-only instead of crashing.
fn resolve_web_dir() -> Option<PathBuf> {
    let has_index = |p: &PathBuf| p.join("index.html").is_file();
    if let Ok(p) = std::env::var("FLOCKMUX_WEB_DIR") {
        let pb = PathBuf::from(p);
        if has_index(&pb) {
            return Some(pb);
        }
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("web/dist"));
        candidates.push(cwd.join("dist"));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("web/dist"));
            candidates.push(dir.join("../../web/dist")); // target/release → repo/web/dist
        }
    }
    candidates.into_iter().find(has_index)
}

/// Reject cross-site requests. flockmux binds loopback with no token auth,
/// but a browser will happily open a cross-origin WebSocket to
/// `ws://127.0.0.1:7777` (WS bypasses CORS and sends no preflight), so any
/// web page the user happens to visit could otherwise spawn agents, inject
/// keystrokes into a live PTY, or read the blackboard.
///
/// Gate on the `Origin` header:
///   * no `Origin` → native client (the MCP subprocess via reqwest, curl,
///     the Tauri webview's own asset-scheme requests) → **allow**.
///   * `Origin` present → allow only if its host is a local host (loopback or
///     `*.localhost`): covers vite dev (`http://localhost:5173`), the bundle
///     served on `:7777`, and the Tauri webview (`tauri://localhost` /
///     `tauri.localhost`). Anything else (e.g. `http://evil.com`) → **403**.
async fn require_local_origin(req: Request, next: Next) -> Response {
    if let Some(origin) = req.headers().get(header::ORIGIN) {
        let allowed = origin.to_str().ok().map(origin_is_local).unwrap_or(false);
        if !allowed {
            tracing::warn!(?origin, "rejected cross-origin request");
            return (
                StatusCode::FORBIDDEN,
                "cross-origin request rejected (flockmux is loopback-only)",
            )
                .into_response();
        }
    }
    next.run(req).await
}

/// True if an `Origin` header value (e.g. `http://localhost:5173`,
/// `tauri://localhost`, `https://tauri.localhost`) points at a local host.
/// A literal `null` origin (sandboxed iframe / `file://`) has no `://` host
/// component → not local → rejected.
fn origin_is_local(origin: &str) -> bool {
    let host = match origin.split_once("://") {
        Some((_scheme, rest)) => rest.split(['/', ':']).next().unwrap_or(""),
        None => return false,
    };
    host == "127.0.0.1"
        || host == "localhost"
        || host == "::1"
        || host == "[::1]"
        || host.ends_with(".localhost")
}

#[cfg(test)]
mod origin_tests {
    use super::origin_is_local;

    #[test]
    fn allows_local_origins() {
        for o in [
            "http://localhost:5173",   // vite dev
            "http://127.0.0.1:7777",   // bundle served by the server
            "http://localhost:7777",
            "http://127.0.0.1:4173",   // vite preview / sidecar test recipe
            "tauri://localhost",       // macOS asset scheme
            "https://tauri.localhost", // Windows/Linux asset scheme
            "http://tauri.localhost",
        ] {
            assert!(origin_is_local(o), "should allow {o}");
        }
    }

    #[test]
    fn rejects_remote_and_null() {
        for o in [
            "http://evil.com",
            "https://attacker.example:443",
            "http://localhost.evil.com", // suffix attack — host is .evil.com
            "http://notlocalhost",
            "null", // sandboxed iframe / file://
            "",
        ] {
            assert!(!origin_is_local(o), "should reject {o}");
        }
    }
}
