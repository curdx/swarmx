//! `GET /api/tasks` + `POST /api/tasks/:id/status` — the Kanban control plane.
//!
//! Each worker IS a task. Its *effective* status is normally derived from
//! lifecycle signals (alive→running, handoff written→done, `.error`→blocked,
//! killed→archived) so the board reflects ground truth with zero operator
//! effort. The operator can also OVERRIDE it from the board (mark blocked /
//! archived / done) — that human decision wins and persists in
//! `workers.task_status`. This turns the read-only ledger into a writable
//! board while staying in sync with the existing handoff/wake machinery.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use flockmux_storage::TaskRecord;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::AppState;

/// Statuses the operator may set, plus the derived ones. Kept as a flat set so
/// the board and the API agree.
const VALID_STATUSES: &[&str] = &[
    "triage", "todo", "ready", "running", "blocked", "done", "archived",
];

/// Derive a task's effective status from its raw signals. Human override wins;
/// otherwise read lifecycle. Pure (no IO) so it's unit-tested below.
fn effective_status(t: &TaskRecord) -> String {
    if let Some(s) = &t.task_status {
        return s.clone();
    }
    if t.error_present {
        return "blocked".into();
    }
    if t.handoff_done {
        return "done".into();
    }
    if t.killed_at.is_some() {
        // Killed before producing its handoff — aborted, not completed.
        return "archived".into();
    }
    if t.last_activity_at.is_some() {
        return "running".into();
    }
    "todo".into()
}

#[derive(Serialize)]
struct TaskView {
    status: String,
    /// True when `status` came from a human override (vs derived).
    overridden: bool,
    #[serde(flatten)]
    task: TaskRecord,
}

pub async fn list_tasks(State(state): State<AppState>) -> impl IntoResponse {
    let rows = state.store.list_tasks().await.unwrap_or_default();
    let views: Vec<TaskView> = rows
        .into_iter()
        .map(|t| TaskView {
            status: effective_status(&t),
            overridden: t.task_status.is_some(),
            task: t,
        })
        .collect();
    Json(json!({ "tasks": views }))
}

#[derive(Deserialize)]
pub struct SetStatusRequest {
    /// New status, or null to clear the override and fall back to derived.
    status: Option<String>,
}

pub async fn set_task_status(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Json(req): Json<SetStatusRequest>,
) -> impl IntoResponse {
    if let Some(s) = &req.status {
        if !VALID_STATUSES.contains(&s.as_str()) {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": format!("invalid status `{s}`; allowed: {VALID_STATUSES:?}") })),
            )
                .into_response();
        }
    }
    match state.store.set_task_status(agent_id, req.status).await {
        Ok(()) => (StatusCode::OK, Json(json!({ "ok": true }))).into_response(),
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

    fn base() -> TaskRecord {
        TaskRecord {
            agent_id: "a".into(),
            parent_agent_id: "p".into(),
            role_label: "writer".into(),
            role_slug: None,
            handoff_signal: Some("ws/main/writer.done".into()),
            task_status: None,
            spawned_at: 0,
            killed_at: None,
            shim_exit_code: None,
            last_activity_at: None,
            workspace_id: None,
            thread_id: None,
            handoff_done: false,
            error_present: false,
        }
    }

    #[test]
    fn derives_lifecycle_statuses() {
        assert_eq!(effective_status(&base()), "todo");

        let mut running = base();
        running.last_activity_at = Some(1);
        assert_eq!(effective_status(&running), "running");

        let mut done = base();
        done.handoff_done = true;
        assert_eq!(effective_status(&done), "done");

        let mut blocked = base();
        blocked.error_present = true;
        assert_eq!(effective_status(&blocked), "blocked");

        let mut aborted = base();
        aborted.killed_at = Some(1);
        assert_eq!(effective_status(&aborted), "archived");
    }

    #[test]
    fn error_beats_done_when_both_present() {
        // A producer that wrote .error after a partial handoff is blocked, not done.
        let mut t = base();
        t.handoff_done = true;
        t.error_present = true;
        assert_eq!(effective_status(&t), "blocked");
    }

    #[test]
    fn human_override_wins_over_lifecycle() {
        let mut t = base();
        t.last_activity_at = Some(1); // would derive "running"
        t.handoff_done = true; // would derive "done"
        t.task_status = Some("blocked".into());
        assert_eq!(effective_status(&t), "blocked");
    }
}
