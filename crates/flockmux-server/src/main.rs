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
    };

    // M6b: launch the wake coordinator. Lives for the whole process; the
    // returned JoinHandle is intentionally dropped because the task exits
    // only when `state.swarm`'s broadcast closes (program shutdown).
    let _wake_handle = wake::WakeCoordinator::spawn(
        state.swarm.clone(),
        state.registry.clone(),
        wake_subs,
    );
    info!("wake coordinator started");

    let app = Router::new()
        .route("/api/plugins", get(routes::rest::list_plugins))
        .route(
            "/api/agent",
            get(routes::rest::list_agents).post(routes::rest::spawn),
        )
        .route("/api/agent/:id", delete(routes::rest::kill))
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
        .route("/api/spells", get(routes::rest::list_spells))
        .route("/api/spell/run", post(routes::rest::run_spell))
        .route("/ws/swarm", get(routes::ws_swarm::ws_swarm))
        .route("/ws/pty/:agent_id", get(routes::pty_ws::pty_ws))
        .layer(CorsLayer::permissive())  // localhost dev convenience
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let addr: SocketAddr = "127.0.0.1:7777".parse()?;
    info!(%addr, "flockmux-server listening (loopback only, no auth)");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
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
