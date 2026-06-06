//! Models admin — 给「模型」设置页用的后端。per-CLI 的 tier→具体模型映射读写。
//! loopback 单用户、无 auth（同本仓库其它 REST）。
//!
//!   GET /api/models  → { config, clis } —— 当前生效配置（shipped 合并后）+ 真实
//!                       CLI 列表（id / display_name / supports_model），让页面每
//!                       个 CLI 一张卡、不支持 model 覆盖的灰置。
//!   PUT /api/models  → 收整个 ModelConfig，校验 cli id 在 plugin registry，原子写
//!                       ~/.flockmux/models.json + 热替换 AppState，返回持久化后的
//!                       config。整体编辑（同 MCP 页），不做 per-key 子路由。

use crate::models_config::{self, ModelConfig};
use crate::AppState;
use axum::{extract::State, http::StatusCode, Json};
use serde_json::{json, Value};

/// GET /api/models — current effective config + the real CLI list.
pub async fn get_models(State(state): State<AppState>) -> Json<Value> {
    let config = state.models.read().await.clone();
    let clis: Vec<Value> = state
        .plugins
        .list()
        .iter()
        .map(|p| {
            json!({
                "id": p.id,
                "display_name": p.display_name,
                // CLIs whose manifest declares no model_args can't take a
                // --model override; the UI greys these out.
                "supports_model": !p.model_args.is_empty(),
                // Whether the opus/sonnet/haiku tier names are THIS CLI's own
                // model aliases (only claude). The page shows tier rows only
                // when true; codex etc. get just a default-model row.
                "native_tiers": p.native_tiers,
            })
        })
        .collect();
    Json(json!({ "config": config, "clis": clis }))
}

/// PUT /api/models — validate, persist atomically, hot-swap the in-memory copy.
pub async fn put_models(
    State(state): State<AppState>,
    Json(config): Json<ModelConfig>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let mut known: Vec<String> = state.plugins.list().iter().map(|p| p.id.clone()).collect();
    known.sort();
    for (cli, m) in &config.clis {
        if !known.iter().any(|k| k == cli) {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({"error": format!("unknown cli '{cli}' — valid: {known:?}")})),
            ));
        }
        if m.tiers.keys().any(|k| k.trim().is_empty()) {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({"error": format!("cli '{cli}' has an empty tier key")})),
            ));
        }
        // Effort is an abstract level the plugin maps per-CLI; constrain it so a
        // typo doesn't silently become a no-op (empty = the model's own default).
        let eff = m.effort.trim();
        if !eff.is_empty() && !["low", "medium", "high", "xhigh", "max"].contains(&eff) {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({"error": format!("cli '{cli}' has invalid effort '{eff}' — valid: low|medium|high|xhigh|max or empty")})),
            ));
        }
    }

    if let Err(e) = models_config::save(&config) {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("save failed: {e}")})),
        ));
    }
    *state.models.write().await = config.clone();
    tracing::info!(clis = config.clis.len(), "model config updated via /api/models");
    Ok(Json(json!({ "config": config })))
}
