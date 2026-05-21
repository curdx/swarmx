//! Swarm REST: `/api/message`, `/api/blackboard`, `/api/blackboard/*path`.

use crate::AppState;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use flockmux_protocol::rest::{
    BlackboardEntry, BlackboardHistoryEntry, BlackboardSnapshot, MarkReadRequest, MarkReadResponse,
    MessageRecord, SendMessageRequest, UnreadCountResponse, WriteBlackboardRequest,
};
use flockmux_storage::ListMessagesOpts;
use flockmux_swarm::NewMessage;
use serde::Deserialize;
use serde_json::json;

#[derive(Debug, Deserialize, Default)]
pub struct ListMessagesQuery {
    pub to: Option<String>,
    pub from: Option<String>,
    pub q: Option<String>,
    pub limit: Option<i64>,
    #[serde(default)]
    pub only_undelivered: bool,
}

pub async fn list_messages(
    State(state): State<AppState>,
    Query(q): Query<ListMessagesQuery>,
) -> Result<Json<Vec<MessageRecord>>, (StatusCode, Json<serde_json::Value>)> {
    let items = if let Some(query) = q.q {
        state
            .store
            .search_messages(query)
            .await
            .map_err(internal_err)?
    } else {
        state
            .store
            .list_messages(ListMessagesOpts {
                to_agent: q.to,
                from_agent: q.from,
                only_undelivered: q.only_undelivered,
                limit: q.limit.unwrap_or(200),
            })
            .await
            .map_err(internal_err)?
    };
    Ok(Json(
        items
            .into_iter()
            .map(|r| MessageRecord {
                id: r.id,
                from_agent: r.from_agent,
                to_agent: r.to_agent,
                kind: r.kind,
                body: r.body,
                sent_at: r.sent_at,
                delivered_at: r.delivered_at,
                read_at: r.read_at,
                in_reply_to: r.in_reply_to,
            })
            .collect(),
    ))
}

pub async fn send_message(
    State(state): State<AppState>,
    Json(req): Json<SendMessageRequest>,
) -> Result<Json<MessageRecord>, (StatusCode, Json<serde_json::Value>)> {
    let from = req.from.unwrap_or_else(|| "system".into());
    let record = state
        .swarm
        .send_message(NewMessage {
            from_agent: from,
            to_agent: req.to,
            kind: req.kind,
            body: req.body,
            sent_at: now_ms(),
            in_reply_to: req.in_reply_to,
        })
        .await
        .map_err(internal_err)?;
    Ok(Json(MessageRecord {
        id: record.id,
        from_agent: record.from_agent,
        to_agent: record.to_agent,
        kind: record.kind,
        body: record.body,
        sent_at: record.sent_at,
        delivered_at: record.delivered_at,
        read_at: record.read_at,
        in_reply_to: record.in_reply_to,
    }))
}

/// `POST /api/message/read` — caller declares which messages it has read.
/// The server filters by `to_agent` so cross-agent marks are silently
/// dropped (no error, just an empty `marked` list).
pub async fn mark_messages_read(
    State(state): State<AppState>,
    Json(req): Json<MarkReadRequest>,
) -> Result<Json<MarkReadResponse>, (StatusCode, Json<serde_json::Value>)> {
    let at = now_ms();
    let marked = state
        .swarm
        .mark_read(req.to, req.ids)
        .await
        .map_err(internal_err)?;
    Ok(Json(MarkReadResponse { marked, at }))
}

#[derive(Debug, Deserialize)]
pub struct UnreadCountQuery {
    pub to: String,
}

pub async fn unread_count(
    State(state): State<AppState>,
    Query(q): Query<UnreadCountQuery>,
) -> Result<Json<UnreadCountResponse>, (StatusCode, Json<serde_json::Value>)> {
    let count = state
        .store
        .count_unread(q.to.clone())
        .await
        .map_err(internal_err)?;
    Ok(Json(UnreadCountResponse { to: q.to, count }))
}

#[derive(Debug, Deserialize, Default)]
pub struct BlackboardHistoryQuery {
    #[serde(default)]
    pub limit: Option<i64>,
    #[serde(default)]
    pub include_content: Option<bool>,
}

pub async fn blackboard_history(
    State(state): State<AppState>,
    Path(path): Path<String>,
    Query(opts): Query<BlackboardHistoryQuery>,
) -> Result<Json<Vec<BlackboardHistoryEntry>>, (StatusCode, Json<serde_json::Value>)> {
    let ops = state
        .store
        .list_blackboard_ops(Some(path))
        .await
        .map_err(internal_err)?;
    let include_content = opts.include_content.unwrap_or(false);
    let limit = opts.limit.unwrap_or(50).max(1) as usize;
    Ok(Json(
        ops.into_iter()
            .take(limit)
            .map(|r| BlackboardHistoryEntry {
                id: r.id,
                agent_id: r.agent_id,
                op: r.op,
                path: r.path,
                sha256: r.sha256,
                at: r.at,
                content: if include_content { Some(r.content) } else { None },
            })
            .collect(),
    ))
}

pub async fn list_blackboard_paths(
    State(state): State<AppState>,
) -> Result<Json<Vec<BlackboardEntry>>, (StatusCode, Json<serde_json::Value>)> {
    let latest = state
        .store
        .list_blackboard_ops(None)
        .await
        .map_err(internal_err)?;
    Ok(Json(
        latest
            .into_iter()
            .map(|r| BlackboardEntry {
                path: r.path,
                sha256: r.sha256,
                at: r.at,
                op: r.op,
            })
            .collect(),
    ))
}

pub async fn read_blackboard(
    State(state): State<AppState>,
    Path(path): Path<String>,
) -> Result<Json<BlackboardSnapshot>, (StatusCode, Json<serde_json::Value>)> {
    let content = state
        .swarm
        .read_blackboard(&path)
        .await
        .map_err(bad_request_err)?;
    let content = match content {
        Some(c) => c,
        None => {
            return Err((
                StatusCode::NOT_FOUND,
                Json(json!({"error": format!("blackboard path not found: {path}")})),
            ))
        }
    };
    // Find the latest op for this path so we can return sha + at.
    let ops = state
        .store
        .list_blackboard_ops(Some(path.clone()))
        .await
        .map_err(internal_err)?;
    let (sha, at) = ops
        .first()
        .map(|r| (r.sha256.clone(), r.at))
        .unwrap_or_else(|| (sha256_hex(content.as_bytes()), 0));
    Ok(Json(BlackboardSnapshot {
        path,
        content,
        sha256: sha,
        at,
    }))
}

pub async fn write_blackboard(
    State(state): State<AppState>,
    Path(path): Path<String>,
    Json(req): Json<WriteBlackboardRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let record = state
        .swarm
        .write_blackboard(req.agent_id, &path, &req.content)
        .await
        .map_err(bad_request_err)?;
    Ok(Json(json!({
        "id": record.id,
        "path": record.path,
        "sha256": record.sha256,
        "at": record.at,
    })))
}

fn internal_err(e: anyhow::Error) -> (StatusCode, Json<serde_json::Value>) {
    tracing::warn!(?e, "swarm route error");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"error": e.to_string()})),
    )
}

fn bad_request_err(e: anyhow::Error) -> (StatusCode, Json<serde_json::Value>) {
    tracing::debug!(?e, "swarm route bad request");
    (
        StatusCode::BAD_REQUEST,
        Json(json!({"error": e.to_string()})),
    )
}

fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    let out = h.finalize();
    let mut s = String::with_capacity(out.len() * 2);
    for b in out {
        use std::fmt::Write;
        let _ = write!(s, "{b:02x}");
    }
    s
}
