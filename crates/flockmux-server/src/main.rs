//! flockmux-server: axum-based HTTP/WS gateway.
//!
//! M1 surface:
//!   GET  /api/plugins          — list available CLI plugins
//!   POST /api/agent            — spawn an agent ({ cli: "claude"|"codex" })
//!   DELETE /api/agent/:id      — kill an agent
//!   WS   /ws/pty/:id           — bidirectional PTY bridge
//!
//! Bind: 127.0.0.1:7777 (loopback only — no auth, single-user local).

mod plugins;
mod pre_spawn;
mod pty_stream;
mod registry;
mod roles;
mod routes;
mod spawn;
mod spells;
mod wake;

use anyhow::{Context, Result};
use axum::{
    routing::{delete, get, post},
    Router,
};
use flockmux_storage::Store;
use flockmux_swarm::{Swarm, WatcherHandle};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tower_http::cors::CorsLayer;
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

    let plugins_dir = plugins::default_plugins_dir();
    info!(dir = %plugins_dir.display(), "loading cli plugins");
    let plugin_registry = plugins::PluginRegistry::load_dir(&plugins_dir)
        .with_context(|| format!("load plugins from {}", plugins_dir.display()))?;
    info!(count = plugin_registry.list().len(), "plugins loaded");

    let spells_dir = spells::default_spells_dir();
    info!(dir = %spells_dir.display(), "loading spells");
    let spell_registry = spells::SpellRegistry::load_dir(&spells_dir)
        .with_context(|| format!("load spells from {}", spells_dir.display()))?;
    info!(count = spell_registry.list().len(), "spells loaded");

    let roles_dir = roles::default_roles_dir();
    info!(dir = %roles_dir.display(), "loading roles");
    let role_registry = roles::RoleRegistry::load_dir(&roles_dir)
        .with_context(|| format!("load roles from {}", roles_dir.display()))?;
    info!(count = role_registry.list().len(), "roles loaded");

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

    let recordings_root = recordings_root_default();
    std::fs::create_dir_all(&recordings_root)?;
    info!(recordings = %recordings_root.display(), "recordings root");

    let server_url = std::env::var("FLOCKMUX_SERVER_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:7777".into());

    let wake_subs: wake::WakeSubs =
        std::sync::Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new()));
    let exit_keys: wake::ExitKeys =
        std::sync::Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new()));

    let state = AppState {
        plugins: Arc::new(plugin_registry),
        spells: Arc::new(spell_registry),
        roles: Arc::new(role_registry),
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

    let app = Router::new()
        .route("/api/plugins", get(routes::rest::list_plugins))
        .route(
            "/api/agent",
            get(routes::rest::list_agents).post(routes::rest::spawn),
        )
        .route("/api/worker", post(routes::rest::spawn_worker))
        .route("/api/agent/:id", delete(routes::rest::kill))
        .route("/api/agent/:id/wake", post(routes::rest::wake_agent))
        .route("/api/agent/:id/interrupt", post(routes::rest::interrupt))
        .route("/api/agent/:id/resume", post(routes::rest::resume))
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
        .route("/api/spells", get(routes::rest::list_spells))
        .route("/api/spell/run", post(routes::rest::run_spell))
        .route("/ws/swarm", get(routes::ws_swarm::ws_swarm))
        .route("/ws/pty/:agent_id", get(routes::pty_ws::pty_ws))
        .layer(CorsLayer::permissive())  // localhost dev convenience
        .layer(TraceLayer::new_for_http())
        .with_state(state.clone());

    // FLOCKMUX_PORT env override — used to stand up a parallel test
    // instance (e.g. the .app sidecar) without colliding with a dev
    // backend already bound to 7777. Frontend defaults already assume
    // 7777, so do NOT set this in production.
    let port: u16 = std::env::var("FLOCKMUX_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(7777);
    let addr: SocketAddr = ([127, 0, 0, 1], port).into();
    info!(%addr, "flockmux-server listening (loopback only, no auth)");
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
        let progress_key = format!("{}/progress.ledger.md", ws.id);
        if let Ok(Some(content)) = state.swarm.read_blackboard(&progress_key).await {
            if content.contains("all_done") {
                skipped += 1;
                info!(workspace_id = %ws.id, "auto-respawn: task all_done, skipping (revive on demand)");
                continue;
            }
        }
        let req = RunSpellRequest {
            name: "init".into(),
            task: String::new(),
            workspace_dir: Some(ws.cwd.clone()),
            workspace_id: Some(ws.id.clone()),
            caller_agent_id: None,
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
