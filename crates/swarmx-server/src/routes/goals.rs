//! `/api/goals` — workspace-level Goal Mode state.
//!
//! Goals are the durable control surface for long-running agent work: objective,
//! acceptance criteria, status, and an optional token budget. The storage layer
//! keeps criteria as JSON text; this route is the typed API seam callers use.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use swarmx_storage::{GoalRecord, NewGoal, NewGoalEvidence};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::AppState;

const VALID_GOAL_STATUSES: &[&str] = &["active", "paused", "blocked", "complete", "archived"];

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn valid_status(status: &str) -> bool {
    VALID_GOAL_STATUSES.contains(&status)
}

fn completed_at_for(status: &str, at: i64) -> Option<i64> {
    matches!(status, "complete" | "archived").then_some(at)
}

fn normalize_optional(s: Option<String>) -> Option<String> {
    s.map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

/// True if a storage error is a SQLite foreign-key violation — i.e. the row
/// points at a workspace/thread/goal that doesn't exist. SQLite's message
/// (`FOREIGN KEY constraint failed`) is stable across versions; we match on
/// the full error chain (`{e:#}`) so a `.context(...)`-wrapped error still
/// classifies. Lets the route return a friendly 404 instead of dumping the
/// raw SQLite text in a 500.
fn is_foreign_key_violation(e: &anyhow::Error) -> bool {
    format!("{e:#}").contains("FOREIGN KEY constraint failed")
}

#[derive(Deserialize)]
pub struct ListGoalsQuery {
    workspace_id: Option<String>,
    /// Empty/absent = no thread filter; literal `null` scopes to workspace-main
    /// goals (`thread_id IS NULL`); any other value filters exact thread id.
    thread_id: Option<String>,
}

#[derive(Serialize)]
struct GoalView {
    id: String,
    workspace_id: String,
    thread_id: Option<String>,
    objective: String,
    success_criteria: Vec<String>,
    status: String,
    budget_tokens: Option<i64>,
    created_at: i64,
    updated_at: i64,
    completed_at: Option<i64>,
}

fn view_from_record(rec: GoalRecord) -> GoalView {
    let success_criteria =
        serde_json::from_str::<Vec<String>>(&rec.success_criteria).unwrap_or_else(|_| Vec::new());
    GoalView {
        id: rec.id,
        workspace_id: rec.workspace_id,
        thread_id: rec.thread_id,
        objective: rec.objective,
        success_criteria,
        status: rec.status,
        budget_tokens: rec.budget_tokens,
        created_at: rec.created_at,
        updated_at: rec.updated_at,
        completed_at: rec.completed_at,
    }
}

pub async fn list_goals(
    State(state): State<AppState>,
    Query(q): Query<ListGoalsQuery>,
) -> impl IntoResponse {
    let workspace_id = q.workspace_id.filter(|s| !s.trim().is_empty());
    let thread_id = match q.thread_id.as_deref() {
        Some("null") => Some(None),
        Some(t) if !t.trim().is_empty() => Some(Some(t.to_string())),
        _ => None,
    };
    match state.store.list_goals(workspace_id, thread_id).await {
        Ok(goals) => {
            Json(json!({ "goals": goals.into_iter().map(view_from_record).collect::<Vec<_>>() }))
                .into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
pub struct CreateGoalRequest {
    workspace_id: String,
    thread_id: Option<String>,
    objective: String,
    #[serde(default)]
    success_criteria: Vec<String>,
    budget_tokens: Option<i64>,
    status: Option<String>,
}

pub async fn create_goal(
    State(state): State<AppState>,
    Json(req): Json<CreateGoalRequest>,
) -> impl IntoResponse {
    let workspace_id = req.workspace_id.trim();
    let objective = req.objective.trim();
    if workspace_id.is_empty() || objective.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "workspace_id and objective are required" })),
        )
            .into_response();
    }
    let status = req.status.unwrap_or_else(|| "active".into());
    if !valid_status(&status) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": format!("invalid status `{status}`; allowed: {VALID_GOAL_STATUSES:?}") })),
        )
            .into_response();
    }
    if req.budget_tokens.is_some_and(|n| n <= 0) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "budget_tokens must be positive when provided" })),
        )
            .into_response();
    }

    // Validate the FK parents up front (mirrors cron.rs) so a non-existent
    // workspace/thread returns a friendly 404 instead of a raw SQLite
    // "FOREIGN KEY constraint failed" 500 from the insert below.
    match state.store.get_workspace_by_id(workspace_id.to_string()).await {
        Ok(Some(_)) => {}
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": format!("unknown workspace_id: {workspace_id}") })),
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
    let thread_id = req.thread_id.filter(|s| !s.trim().is_empty());
    if let Some(tid) = thread_id.as_deref() {
        match state.store.get_thread(tid.to_string()).await {
            Ok(Some(_)) => {}
            Ok(None) => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(json!({ "error": format!("unknown thread_id: {tid}") })),
                )
                    .into_response();
            }
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "error": format!("thread lookup failed: {e}") })),
                )
                    .into_response();
            }
        }
    }

    let at = now_ms();
    let criteria: Vec<String> = req
        .success_criteria
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let success_criteria = match serde_json::to_string(&criteria) {
        Ok(v) => v,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    };
    let rec = NewGoal {
        id: uuid::Uuid::new_v4().to_string(),
        workspace_id: workspace_id.to_string(),
        thread_id,
        objective: objective.to_string(),
        success_criteria,
        status: status.clone(),
        budget_tokens: req.budget_tokens,
        created_at: at,
        updated_at: at,
        completed_at: completed_at_for(&status, at),
    };
    let id = rec.id.clone();
    match state.store.upsert_goal(rec).await {
        Ok(()) => (StatusCode::OK, Json(json!({ "ok": true, "id": id }))).into_response(),
        // Belt-and-suspenders: the workspace/thread existed when we checked, but
        // a concurrent delete could still trip the FK at insert time. Surface
        // that as a friendly 404 rather than a raw SQLite 500.
        Err(e) if is_foreign_key_violation(&e) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "workspace or thread no longer exists" })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
pub struct UpdateGoalStatusRequest {
    status: String,
}

pub async fn update_goal_status(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<UpdateGoalStatusRequest>,
) -> impl IntoResponse {
    if !valid_status(&req.status) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": format!("invalid status `{}`; allowed: {VALID_GOAL_STATUSES:?}", req.status) })),
        )
            .into_response();
    }
    let at = now_ms();
    match state
        .store
        .update_goal_status(
            id,
            req.status.clone(),
            at,
            completed_at_for(&req.status, at),
        )
        .await
    {
        Ok(true) => (StatusCode::OK, Json(json!({ "ok": true }))).into_response(),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "no such goal" })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
pub struct ListGoalEvidenceQuery {
    limit: Option<usize>,
}

pub async fn list_goal_evidence(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<ListGoalEvidenceQuery>,
) -> impl IntoResponse {
    match state
        .store
        .list_goal_evidence(id, q.limit.unwrap_or(50))
        .await
    {
        Ok(evidence) => Json(json!({ "evidence": evidence })).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
pub struct AddGoalEvidenceRequest {
    kind: String,
    summary: String,
    source_agent_id: Option<String>,
    blackboard_path: Option<String>,
    command: Option<String>,
}

pub async fn add_goal_evidence(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<AddGoalEvidenceRequest>,
) -> impl IntoResponse {
    let kind = req.kind.trim();
    let summary = req.summary.trim();
    if kind.is_empty() || summary.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "kind and summary are required" })),
        )
            .into_response();
    }
    let at = now_ms();
    let rec = NewGoalEvidence {
        id: uuid::Uuid::new_v4().to_string(),
        goal_id: id,
        kind: kind.to_string(),
        summary: summary.to_string(),
        source_agent_id: normalize_optional(req.source_agent_id),
        blackboard_path: normalize_optional(req.blackboard_path),
        command: normalize_optional(req.command),
        created_at: at,
    };
    let evidence_id = rec.id.clone();
    match state.store.add_goal_evidence(rec).await {
        Ok(()) => (
            StatusCode::OK,
            Json(json!({ "ok": true, "id": evidence_id })),
        )
            .into_response(),
        // The `goal_id` FK is the existence gate (there's no goal-lookup seam
        // here): a missing goal trips `FOREIGN KEY constraint failed`. Return a
        // friendly 404 instead of the raw SQLite text in a 500.
        Err(e) if is_foreign_key_violation(&e) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "no such goal" })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_goal_statuses() {
        assert!(valid_status("active"));
        assert!(valid_status("blocked"));
        assert!(!valid_status("running"));
    }

    #[test]
    fn completion_statuses_stamp_completed_at() {
        assert_eq!(completed_at_for("active", 42), None);
        assert_eq!(completed_at_for("complete", 42), Some(42));
        assert_eq!(completed_at_for("archived", 42), Some(42));
    }

    #[test]
    fn evidence_optional_fields_are_trimmed_and_empty_dropped() {
        assert_eq!(
            normalize_optional(Some("  agent-1  ".into())).as_deref(),
            Some("agent-1")
        );
        assert_eq!(normalize_optional(Some("   ".into())), None);
        assert_eq!(normalize_optional(None), None);
    }

    #[test]
    fn detects_foreign_key_violation_through_context_wrapping() {
        // The storage layer wraps the rusqlite error with `.context(...)`, so the
        // FK signature lives deeper in the chain — `{e:#}` must still classify it.
        let inner = anyhow::anyhow!("FOREIGN KEY constraint failed");
        let wrapped = inner.context("spawn_blocking add_goal_evidence");
        assert!(is_foreign_key_violation(&wrapped));

        // A generic error (e.g. disk I/O) must NOT be classified as a 404 case.
        let other = anyhow::anyhow!("database is locked").context("spawn_blocking upsert_goal");
        assert!(!is_foreign_key_violation(&other));
    }
}
