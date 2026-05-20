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
mod registry;
mod routes;
mod spawn;

use anyhow::{Context, Result};
use axum::{
    routing::{delete, get, post},
    Router,
};
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

    let state = AppState {
        plugins: Arc::new(plugin_registry),
        registry: registry::Registry::new(),
        shim_path,
        workspaces_root,
    };

    let app = Router::new()
        .route("/api/plugins", get(routes::rest::list_plugins))
        .route("/api/agent", post(routes::rest::spawn))
        .route("/api/agent/:id", delete(routes::rest::kill))
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
