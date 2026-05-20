//! REST endpoints. Loopback-only; no auth (per user decision — local single-user).

use crate::plugins::CliPlugin;
use crate::spawn::spawn_agent;
use crate::AppState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use flockmux_protocol::rest::{CliPluginInfo, SpawnAgentRequest, SpawnAgentResponse};
use serde_json::json;
use std::path::PathBuf;

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

    let result = spawn_agent(&plugin, req.role, &workspace_root, &state.shim_path)
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
        })?;

    let resp = SpawnAgentResponse {
        agent_id: result.agent_id.clone(),
        cli: result.slot.cli.clone(),
        role: result.slot.role.clone(),
        workspace: result.slot.workspace.clone(),
    };
    state.registry.insert(result.agent_id, result.slot);
    Ok(Json(resp))
}

pub async fn kill(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
) -> impl IntoResponse {
    match state.registry.remove(&agent_id) {
        Some(slot) => {
            let slot = slot.lock();
            slot.bridge.kill();
            (StatusCode::NO_CONTENT, Json(json!({"ok": true})))
        }
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("agent {agent_id} not found")})),
        ),
    }
}
