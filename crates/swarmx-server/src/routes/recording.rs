//! Recording REST endpoints (M3 #19). The asciicast v2 file lives on disk;
//! SQLite tracks the metadata + lifecycle timestamps.
//!
//! Loopback-only — no auth (same posture as the rest of the API).

use crate::AppState;
use axum::{
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use swarmx_protocol::rest::RecordingInfo;
use serde::Deserialize;
use serde_json::json;

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    pub agent_id: Option<String>,
}

pub async fn list_recordings(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> impl IntoResponse {
    match state.store.list_recordings(q.agent_id).await {
        Ok(rows) => {
            let items: Vec<RecordingInfo> = rows
                .into_iter()
                .map(|r| RecordingInfo {
                    id: r.id,
                    agent_id: r.agent_id,
                    started_at: r.started_at,
                    finalized_at: r.finalized_at,
                    duration_ms: r.duration_ms,
                    cols: r.cols,
                    rows: r.rows,
                    last_seq: r.last_seq,
                })
                .collect();
            Json(items).into_response()
        }
        Err(e) => {
            tracing::warn!(?e, "list_recordings failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
                .into_response()
        }
    }
}

/// `GET /api/recording/:id` — reads the whole .cast file into memory and
/// returns it in one response (the recorder caps a single cast at
/// `DEFAULT_MAX_CAST_BYTES` = 64 MiB, so this is bounded). Content-Type is
/// `application/x-asciicast` (de facto for asciicast files); clients that
/// don't know it can treat the body as JSON-lines since each line is valid
/// JSON.
///
/// Follow-up: switch to true streaming (`ReaderStream` + `Body::from_stream`)
/// to avoid buffering the whole cast; deferred because that loses the current
/// "file missing → 404 JSON" path once headers are already on the wire.
pub async fn get_recording(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let row = match state.store.get_recording(id.clone()).await {
        Ok(Some(r)) => r,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": format!("recording {id} not found")})),
            )
                .into_response();
        }
        Err(e) => {
            tracing::warn!(?e, %id, "get_recording lookup failed");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
                .into_response();
        }
    };

    let bytes = match tokio::fs::read(&row.path).await {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!(?e, %id, path = %row.path, "read .cast file failed");
            return (
                StatusCode::NOT_FOUND,
                Json(json!({
                    "error": format!("cast file missing for {id}: {e}"),
                })),
            )
                .into_response();
        }
    };

    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/x-asciicast")],
        bytes,
    )
        .into_response()
}
