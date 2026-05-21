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
mod routes;
mod spawn;

use anyhow::{Context, Result};
use axum::{
    routing::{delete, get},
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
    pub registry: registry::Registry,
    pub shim_path: PathBuf,
    pub workspaces_root: PathBuf,
    pub store: Arc<Store>,
    pub swarm: Arc<Swarm>,
    pub blackboard_root: PathBuf,
    pub recordings_root: PathBuf,
    /// Keeps the notify-debouncer alive for the program's lifetime. Wrapped
    /// in `Arc` so `AppState` stays `Clone`. Drop terminates the watcher.
    pub _watcher: Arc<WatcherHandle>,
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

    let shim_path = spawn::locate_shim().context("locate flockmux-shim")?;
    info!(shim = %shim_path.display(), "shim located");

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

    let state = AppState {
        plugins: Arc::new(plugin_registry),
        registry: registry::Registry::new(),
        shim_path,
        workspaces_root,
        store,
        swarm,
        blackboard_root,
        recordings_root,
        _watcher: watcher,
    };

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
            "/api/blackboard",
            get(routes::swarm::list_blackboard_paths),
        )
        .route(
            "/api/blackboard/*path",
            get(routes::swarm::read_blackboard).put(routes::swarm::write_blackboard),
        )
        .route(
            "/api/recording",
            get(routes::recording::list_recordings),
        )
        .route("/api/recording/:id", get(routes::recording::get_recording))
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
