//! `/api/cron` CRUD + `/api/cron/:id/run` — scheduled prompts.
//!
//! Jobs are persisted (`cron_jobs`) and fired by the background scheduler
//! (`crate::cron::spawn_scheduler`). The `/run` endpoint fires a job immediately
//! (same `run_job` path) so the action is testable without waiting for the
//! schedule.

use axum::{
    extract::{Path, Query, State},
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

/// Real UTC offsets span -12:00 (-720 min) to +14:00 (+840 min). Anything
/// outside that is a malformed client value (or a deliberate one) that would
/// shift the scheduler's wall-clock matching by a nonsense amount. Returns
/// `Some(400 response)` when out of range, `None` when acceptable.
fn validate_tz_offset(offset: i32) -> Option<axum::response::Response> {
    if !(-720..=840).contains(&offset) {
        return Some(
            (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": format!(
                        "tz_offset_minutes {offset} out of range (-720..=840)"
                    )
                })),
            )
                .into_response(),
        );
    }
    None
}

pub async fn list_cron(State(state): State<AppState>) -> impl IntoResponse {
    let jobs = state.store.list_cron_jobs().await.unwrap_or_default();
    let now_secs = now_ms() / 1000;
    // Enrich each row with `next_run` (unix ms, UTC) so the list can show "next
    // fires at X" — the single most useful column for a schedule. Disabled jobs
    // have no next run. Computed with the job's own offset so the instant is the
    // one the scheduler will actually pick.
    //
    // `next_after` is a minute-by-minute scan that, for a valid-but-never-firing
    // expr (e.g. `0 0 30 2 *`), walks the full ~366-day window — pure CPU. Run
    // the whole enrichment on a blocking thread so a pathological job list can't
    // stall the async executor handling other requests.
    let enriched: Vec<serde_json::Value> = tokio::task::spawn_blocking(move || {
        jobs.iter()
            .map(|j| {
                let next_run = if j.enabled {
                    crate::cron::next_after(&j.cron_expr, now_secs, j.tz_offset_minutes)
                        .map(|s| s * 1000)
                } else {
                    None
                };
                let mut v = serde_json::to_value(j).unwrap_or_else(|_| json!({}));
                v["next_run"] = json!(next_run);
                v
            })
            .collect()
    })
    .await
    .unwrap_or_default();
    Json(json!({ "jobs": enriched }))
}

#[derive(Deserialize)]
pub struct PreviewQuery {
    expr: String,
    /// Minutes east of UTC the expression is written in (0 = UTC). Optional so
    /// older clients keep working.
    #[serde(default)]
    offset: i32,
}

/// Live validation + next-run preview for the create form. `valid=false` → the
/// expr is malformed or out-of-range; `next_run` is the next fire time (unix ms,
/// UTC) the scheduler would pick, or null when valid-but-no-occurrence-within-a-
/// year (e.g. `0 0 30 2 *`). Reuses the scheduler's own matcher, evaluated in the
/// supplied offset so the preview matches what the user means locally.
pub async fn preview_cron(Query(q): Query<PreviewQuery>) -> impl IntoResponse {
    let now_secs = now_ms() / 1000;
    // Same rationale as `list_cron`: `next_after` is a minute-by-minute scan that
    // for a valid-but-never-firing expr walks the full ~366-day window — pure CPU.
    // Run validation + scan on a blocking thread so a pathological live-preview
    // keystroke can't stall the async executor handling other requests.
    let (valid, next_run) = tokio::task::spawn_blocking(move || {
        let valid = crate::cron::is_valid(&q.expr);
        let next_run = if valid {
            crate::cron::next_after(&q.expr, now_secs, q.offset).map(|s| s * 1000)
        } else {
            None
        };
        (valid, next_run)
    })
    .await
    .unwrap_or((false, None));
    Json(json!({ "valid": valid, "next_run": next_run }))
}

#[derive(Deserialize)]
pub struct CreateCronRequest {
    workspace_id: String,
    name: String,
    cron_expr: String,
    prompt: String,
    /// Minutes east of UTC the expression is written in (0 = UTC). Optional so
    /// older clients default to the prior UTC behaviour.
    #[serde(default)]
    tz_offset_minutes: i32,
}

pub async fn create_cron(
    State(state): State<AppState>,
    Json(req): Json<CreateCronRequest>,
) -> impl IntoResponse {
    if !crate::cron::is_valid(&req.cron_expr) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": format!("invalid cron expr `{}` (5 fields, each in range: min 0-59, hour 0-23, dom 1-31, month 1-12, dow 0-7)", req.cron_expr) })),
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
    if let Some(resp) = validate_tz_offset(req.tz_offset_minutes) {
        return resp;
    }
    // A job pointing at a non-existent workspace can never fire usefully — the
    // scheduler would just retry `revive_orchestrator` every minute, failing on
    // "workspace not found" and spamming the log. Reject at creation time.
    match state
        .store
        .get_workspace_by_id(req.workspace_id.clone())
        .await
    {
        Ok(Some(_)) => {}
        Ok(None) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": format!("unknown workspace_id: {}", req.workspace_id) })),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("workspace lookup failed: {e}") })),
            )
                .into_response();
        }
    }
    let rec = CronJobRecord {
        id: uuid::Uuid::new_v4().to_string(),
        workspace_id: req.workspace_id,
        name: if req.name.trim().is_empty() {
            req.cron_expr.clone()
        } else {
            req.name
        },
        cron_expr: req.cron_expr,
        prompt: req.prompt,
        enabled: true,
        created_at: now_ms(),
        last_run_at: None,
        tz_offset_minutes: req.tz_offset_minutes,
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

#[derive(Deserialize)]
pub struct UpdateCronRequest {
    workspace_id: String,
    name: String,
    cron_expr: String,
    prompt: String,
    #[serde(default)]
    tz_offset_minutes: i32,
}

/// Edit an existing job's workspace/name/expr/prompt/offset. Same validation as
/// create; `enabled`/`created_at`/`last_run_at` are preserved. 404 if no such id.
pub async fn update_cron(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<UpdateCronRequest>,
) -> impl IntoResponse {
    if !crate::cron::is_valid(&req.cron_expr) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": format!("invalid cron expr `{}` (5 fields, each in range: min 0-59, hour 0-23, dom 1-31, month 1-12, dow 0-7)", req.cron_expr) })),
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
    if let Some(resp) = validate_tz_offset(req.tz_offset_minutes) {
        return resp;
    }
    // Same rationale as create: don't let an edit repoint a job at a workspace
    // that doesn't exist, which would make every scheduler tick fail to revive.
    match state
        .store
        .get_workspace_by_id(req.workspace_id.clone())
        .await
    {
        Ok(Some(_)) => {}
        Ok(None) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": format!("unknown workspace_id: {}", req.workspace_id) })),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("workspace lookup failed: {e}") })),
            )
                .into_response();
        }
    }
    let name = if req.name.trim().is_empty() {
        req.cron_expr.clone()
    } else {
        req.name
    };
    match state
        .store
        .update_cron_job(
            id,
            req.workspace_id,
            name,
            req.cron_expr,
            req.prompt,
            req.tz_offset_minutes,
        )
        .await
    {
        Ok(0) => (StatusCode::NOT_FOUND, Json(json!({ "error": "no such job" }))).into_response(),
        Ok(_) => (StatusCode::OK, Json(json!({ "ok": true }))).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

pub async fn delete_cron(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
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
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "no such job" })),
        )
            .into_response();
    };
    // Dedup against the scheduler: if this job already fired in the current
    // minute (whether by the 30s scheduler tick or a prior manual click), a
    // second delivery this minute would double-post the prompt to the
    // orchestrator. `run_job` stamps `last_run_at` on success, so the same
    // minute-bucket comparison the scheduler uses (cron.rs:301) is the source
    // of truth here too. Report it explicitly rather than silently no-op'ing.
    let cur_min = now_ms() / 60_000;
    if job.last_run_at.map(|l| l / 60_000) == Some(cur_min) {
        return (
            StatusCode::OK,
            Json(json!({ "ok": true, "skipped": "already ran this minute" })),
        )
            .into_response();
    }
    match crate::cron::run_job(&state, &job).await {
        Ok(()) => (StatusCode::OK, Json(json!({ "ok": true }))).into_response(),
        // P1-33: run_job has no "skip" path — when there's no live orchestrator it
        // revives one, so every Err here is a REAL failure (DB error, revive/spawn
        // failure, message-send failure). Masking it as 200 {skipped} hid genuine
        // breakage from the UI. Surface it as a 5xx with the real reason.
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "ok": false, "error": err })),
        )
            .into_response(),
    }
}
