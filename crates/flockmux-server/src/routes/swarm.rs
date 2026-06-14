//! Swarm REST: `/api/message`, `/api/blackboard`, `/api/blackboard/*path`.

use crate::AppState;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use flockmux_protocol::rest::{
    BlackboardEntry, BlackboardHistoryEntry, BlackboardSnapshot, MarkReadRequest, MarkReadResponse,
    MessageRecord, SendMessageRequest, ThoughtTrace, ThoughtTraceStep, UnreadCountResponse,
    WriteBlackboardRequest,
};
use flockmux_storage::{ListMessagesOpts, ThoughtTraceRecord as StoreThoughtTraceRecord};
use flockmux_swarm::{path_safe, NewMessage, SwarmEvent};
use serde::Deserialize;
use serde_json::json;

#[derive(Debug, Deserialize, Default)]
pub struct ListMessagesQuery {
    pub to: Option<String>,
    pub from: Option<String>,
    pub q: Option<String>,
    pub limit: Option<i64>,
    /// Scope history to one direction (thread) so a quiet thread's older
    /// messages aren't pushed out of the global `limit` window. P1-04.
    pub thread_id: Option<String>,
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
                thread_id: q.thread_id,
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
                thread_id: r.thread_id,
                meta: r.meta,
                thought_trace: r.thought_trace.as_ref().map(storage_trace_to_rest),
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
            // Agent / user free-text via REST carries no server-stamped
            // structure; the UI classifies these with its body heuristics.
            meta: None,
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
        thread_id: record.thread_id,
        meta: record.meta,
        thought_trace: record.thought_trace.as_ref().map(storage_trace_to_rest),
    }))
}

fn storage_trace_to_rest(trace: &StoreThoughtTraceRecord) -> ThoughtTrace {
    let summary =
        serde_json::from_str::<Vec<flockmux_storage::ThoughtTraceStep>>(&trace.summary_json)
            .unwrap_or_default()
            .into_iter()
            .map(|s| ThoughtTraceStep {
                phase: s.phase,
                label: s.label,
                source: s.source,
                at: s.at,
            })
            .collect();
    ThoughtTrace {
        id: trace.id.clone(),
        trigger_message_id: trace.trigger_message_id,
        response_message_id: trace.response_message_id,
        agent_id: trace.agent_id.clone(),
        workspace_id: trace.workspace_id.clone(),
        thread_id: trace.thread_id.clone(),
        status: trace.status.clone(),
        started_at: trace.started_at,
        completed_at: trace.completed_at,
        summary,
        updated_at: trace.updated_at,
    }
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

/// M6f: atomically claim all pending wakes for an agent.
///
/// Replaces `unread_count` as `wake_check`'s primary signal. Returns the
/// ids of `kind="wake"` messages that were unread before this call AND
/// have now been marked read. If the list is non-empty, `wake_check`
/// should emit `block` with a reason that lists those wakes.
///
/// Why a dedicated endpoint vs reusing `unread_count` + `mark_read`:
///   - **Atomicity**: this collapses "see if there are wakes" and "mark
///     them read" into one SQL transaction. The two-call alternative
///     opens a window where a wake arriving between SELECT and UPDATE
///     would be marked-read without being delivered to `wake_check`.
///   - **Semantic clarity**: wake messages aren't human mail. They're
///     consumed by the Stop hook. Having a dedicated verb keeps that
///     distinction visible in the routes table.
///   - **Bug source for M6f**: the previous design relied on
///     `swarm_list_messages` (called by the LLM) marking wakes read.
///     During long turns the LLM would mid-turn-list and silently mark
///     a freshly-arrived wake read before `wake_check` ever saw it,
///     stranding the agent until manual ⚡ wake. Observed in 2026-05-23
///     strict e2e #6.
#[derive(Debug, serde::Serialize)]
pub struct ConsumeWakesResponse {
    pub to: String,
    pub count: i64,
    pub ids: Vec<i64>,
}

pub async fn consume_wakes(
    State(state): State<AppState>,
    Query(q): Query<UnreadCountQuery>,
) -> Result<Json<ConsumeWakesResponse>, (StatusCode, Json<serde_json::Value>)> {
    let at = now_ms();
    let ids = state
        .store
        .consume_wakes(q.to.clone(), at)
        .await
        .map_err(internal_err)?;
    // Broadcast message_read so the UI badge updates promptly. Match
    // the shape that mark_messages_read emits — same event kind, same
    // ids field — so the FE doesn't need a new handler.
    if !ids.is_empty() {
        use flockmux_protocol::ws_swarm::SwarmEvent;
        state.swarm.publish_event(SwarmEvent::MessageRead {
            ids: ids.clone(),
            to_agent: q.to.clone(),
            at,
        });
    }
    Ok(Json(ConsumeWakesResponse {
        to: q.to,
        count: ids.len() as i64,
        ids,
    }))
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
                content: if include_content {
                    Some(r.content)
                } else {
                    None
                },
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
            // Hide paths whose latest op is a `delete` tombstone: the file is
            // gone from disk, so listing it would resurrect a ghost the user
            // can't open. The op-log row is kept (history stays truthful) —
            // `blackboard_history` still shows the delete.
            .filter(|r| r.op != "delete")
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

/// `DELETE /api/blackboard/*path` — remove a single blackboard file and
/// record a `delete` tombstone op so history stays truthful.
///
/// Path safety: this reuses the SAME jail the read handler uses
/// (`path_safe::resolve_existing` against the swarm's blackboard root) — no
/// weaker check. A missing/escaping path is rejected with 400 (consistent with
/// `read_blackboard`'s `bad_request_err`), and a path that resolves outside the
/// root can never reach `fs::remove_file`.
pub async fn delete_blackboard(
    State(state): State<AppState>,
    Path(path): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let root = state.swarm.blackboard_root().to_path_buf();
    let target = path_safe::resolve_existing(&root, &path).map_err(bad_request_err)?;

    // Remove the file off the runtime thread (same as the write path's fs work).
    tokio::task::spawn_blocking(move || -> std::io::Result<()> {
        match std::fs::remove_file(&target) {
            Ok(()) => Ok(()),
            // Already gone on disk is fine — we still want to record the
            // tombstone + broadcast so the op-log and UI converge.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    })
    .await
    .map_err(|e| internal_err(anyhow::anyhow!("spawn_blocking remove_file: {e}")))?
    .map_err(|e| internal_err(anyhow::anyhow!("remove blackboard file: {e}")))?;

    let at = now_ms();
    // Record the delete op (history). A failed op-log insert must NOT swallow
    // the broadcast — the file IS gone, so dependents/UI still need to converge.
    // Mirror write_blackboard's posture: log, broadcast with id=-1, return Ok.
    let id = match state
        .store
        .record_blackboard_delete(None, path.clone(), at)
        .await
    {
        Ok(record) => record.id,
        Err(e) => {
            tracing::warn!(
                ?e,
                path = %path,
                "blackboard delete op-log insert failed; file IS removed — broadcasting anyway (id=-1)"
            );
            -1
        }
    };
    state.swarm.publish_event(SwarmEvent::BlackboardChanged {
        id,
        agent_id: None,
        op: "delete".into(),
        path: path.clone(),
        sha256: String::new(),
        at,
    });
    Ok(Json(json!({ "ok": true, "path": path, "at": at })))
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
