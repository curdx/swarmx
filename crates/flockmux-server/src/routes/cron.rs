//! `/api/cron` CRUD + `/api/cron/:id/run` — scheduled prompts.
//!
//! Jobs are persisted (`cron_jobs`) and fired by the background scheduler
//! (`crate::cron::spawn_scheduler`). The `/run` endpoint fires a job immediately
//! (same `run_job` path) so the action is testable without waiting for the
//! schedule.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use flockmux_storage::CronJobRecord;
use serde::Deserialize;
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::AppState;

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub async fn list_cron(State(state): State<AppState>) -> impl IntoResponse {
    let jobs = state.store.list_cron_jobs().await.unwrap_or_default();
    Json(json!({ "jobs": jobs }))
}

#[derive(Deserialize)]
pub struct CreateCronRequest {
    workspace_id: String,
    name: String,
    cron_expr: String,
    prompt: String,
}

pub async fn create_cron(
    State(state): State<AppState>,
    Json(req): Json<CreateCronRequest>,
) -> impl IntoResponse {
    if !crate::cron::is_valid(&req.cron_expr) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": format!("invalid cron expr `{}` (need 5 space-separated fields)", req.cron_expr) })),
        )
            .into_response();
    }
    if req.workspace_id.trim().is_empty() || req.prompt.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "workspace_id and prompt are required" })),
        )
            .into_response();
    }
    let rec = CronJobRecord {
        id: uuid::Uuid::new_v4().to_string(),
        workspace_id: req.workspace_id,
        name: if req.name.trim().is_empty() { req.cron_expr.clone() } else { req.name },
        cron_expr: req.cron_expr,
        prompt: req.prompt,
        enabled: true,
        created_at: now_ms(),
        last_run_at: None,
    };
    match state.store.record_cron_job(rec.clone()).await {
        Ok(()) => (StatusCode::OK, Json(json!({ "ok": true, "id": rec.id }))).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

pub async fn delete_cron(State(state): State<AppState>, Path(id): Path<String>) -> impl IntoResponse {
    match state.store.delete_cron_job(id).await {
        Ok(()) => (StatusCode::OK, Json(json!({ "ok": true }))).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
pub struct ToggleRequest {
    enabled: bool,
}

pub async fn toggle_cron(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<ToggleRequest>,
) -> impl IntoResponse {
    match state.store.set_cron_enabled(id, req.enabled).await {
        Ok(()) => (StatusCode::OK, Json(json!({ "ok": true }))).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

pub async fn run_cron(State(state): State<AppState>, Path(id): Path<String>) -> impl IntoResponse {
    let jobs = state.store.list_cron_jobs().await.unwrap_or_default();
    let Some(job) = jobs.into_iter().find(|j| j.id == id) else {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "no such job" }))).into_response();
    };
    match crate::cron::run_job(&state, &job).await {
        Ok(()) => (StatusCode::OK, Json(json!({ "ok": true }))).into_response(),
        // A "skipped" (no live orchestrator) is not a server error — report it.
        Err(skip) => (StatusCode::OK, Json(json!({ "ok": false, "skipped": skip }))).into_response(),
    }
}
