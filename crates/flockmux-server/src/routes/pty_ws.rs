//! `/ws/pty/:agent_id` — bidirectional PTY bridge.
//!
//! Server → client:
//!   * binary frame `[4B seq][PTY bytes]` (seq starts at 1, monotonic)
//!   * text frame: ServerControl JSON (Hello, ShimReady, ShimExit, Eof, Error)
//!
//! Client → server:
//!   * binary frame: raw keystroke bytes
//!   * text frame: ClientControl JSON (Resize / Ack / Resume / Signal / Kill / Detach)
//!
//! Resume protocol (M3):
//!   The client passes `?last_seq=N` on the WebSocket URL. The server snapshots
//!   the per-agent `PtyStream`:
//!
//!     - `last_seq` absent / 0  → cursor = current head; client gets live tail
//!       only (no replay).
//!     - `last_seq` = X with X+1 in the buffer → cursor = X; client gets the
//!       intervening bytes immediately and Hello.seq_start = X+1.
//!     - `last_seq` = X but the buffer has evicted X+1 → cursor = current head;
//!       Hello.seq_start > X+1 (the client infers the gap from the jump and
//!       drops local scrollback). An `Error` frame is also sent for UX.
//!
//!   The single writer task uses `fetch_since(cursor)` + `wait_changed` to
//!   relay bytes; it also subscribes to the per-agent lifecycle broadcast so
//!   ShimReady/ShimExit fire live to every attached client (with the snapshot
//!   inlined in Hello so resume attaches see correct status without waiting).

use crate::registry::LifecycleEvent;
use crate::AppState;
use axum::{
    extract::{
        ws::{Message, WebSocket},
        Path, Query, State, WebSocketUpgrade,
    },
    response::IntoResponse,
};
use bytes::Bytes;
use flockmux_protocol::ws_pty::{ClientControl, ServerControl, Signal};
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

#[derive(Debug, Deserialize, Default)]
pub struct AttachQuery {
    /// Last seq the client successfully received on its previous attach.
    /// Server tries to replay `last_seq+1..=head`; falls back to live-tail
    /// with a gap signal if the buffer no longer holds that range.
    #[serde(default)]
    last_seq: Option<u32>,
}

pub async fn pty_ws(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Query(params): Query<AttachQuery>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state, agent_id, params))
}

/// Serialize a `ServerControl` and send it on `sink`, logging (NOT panicking)
/// on the practically-impossible serialize error. Generic over the sink so it
/// works for both the unsplit `WebSocket` (pre-attach) and the `SplitSink`
/// (the writer pump). Returns the send result so callers that must bail on a
/// dead socket (e.g. the Hello frame) can still observe it. Replaces a row of
/// `serde_json::to_string(&ctrl).unwrap()` panics on the WS hot path.
async fn send_ctrl<S, E>(sink: &mut S, ctrl: &ServerControl) -> Result<(), E>
where
    S: SinkExt<Message, Error = E> + Unpin,
{
    match serde_json::to_string(ctrl) {
        Ok(s) => sink.send(Message::Text(s)).await,
        Err(e) => {
            warn!(?e, "failed to serialize ServerControl frame; dropping it");
            Ok(())
        }
    }
}

async fn handle_socket(
    mut socket: WebSocket,
    state: AppState,
    agent_id: String,
    params: AttachQuery,
) {
    let slot_arc = match state.registry.get(&agent_id) {
        Some(s) => s,
        None => {
            let _ = send_ctrl(
                &mut socket,
                &ServerControl::Error {
                    message: format!("agent {agent_id} not found"),
                },
            )
            .await;
            let _ = socket.close().await;
            return;
        }
    };

    // Clone the bits we need so we can drop the slot lock before any await.
    let parts = {
        let slot = slot_arc.lock();
        let lifecycle_snapshot = *slot.lifecycle.lock();
        match (slot.pty_stream(), slot.pty_input()) {
            (Some(stream), Some(input_tx)) => {
                Some((stream, input_tx, slot.lifecycle_tx.clone(), lifecycle_snapshot))
            }
            // Non-PTY agent (ACP): no terminal stream to attach to.
            _ => None,
        }
    };
    let (stream, input_tx, lifecycle_tx, lifecycle_snapshot) = match parts {
        Some(p) => p,
        None => {
            // Non-PTY agent (ACP): no terminal stream to attach. Send a
            // readable control frame so a client that still reached this
            // endpoint (e.g. an old `?tab=terminal` deep link) shows an
            // explanatory message instead of a bare WS 1005 close. The drawer's
            // normal path no longer opens this socket for ACP agents.
            let _ = send_ctrl(
                &mut socket,
                &ServerControl::Error {
                    message: format!(
                        "agent {agent_id} runs over ACP — no PTY terminal; see the Activity tab"
                    ),
                },
            )
            .await;
            let _ = socket.close().await;
            return;
        }
    };

    // Resolve `last_seq` against the buffer's current state. `cursor` is the
    // last seq the client is considered to have already received; the writer
    // task will start emitting bytes with seq > cursor.
    let snap = stream.snapshot();
    let head = snap.next_seq.saturating_sub(1);
    let mut gap_seq_lost: Option<(u32, u32)> = None;
    let cursor: u32 = match params.last_seq {
        None | Some(0) => {
            // Fresh attach: replay the ENTIRE remaining ring buffer.
            //
            // Old behaviour was cursor=head (live-tail only) on the theory
            // "new attach = new observer, don't waste bandwidth resending
            // bytes they never asked for". But the dominant client is xterm
            // in AgentDrawer — every time the drawer closes and reopens,
            // React unmounts the XtermPane and the next mount gets a brand
            // new Terminal with empty scrollback. cursor=head meant the
            // user saw an empty pane unless the agent happened to be
            // streaming new bytes right then; if the agent had STOPped
            // (most common case for a one-shot scout/planner/critic), the
            // pane stayed permanently blank.
            //
            // Replaying the buffer head costs at most MAX_BUFFER_BYTES
            // (1 MiB) per attach, which is fine for the human-paced
            // "click avatar" trigger.
            match snap.oldest_buffered {
                Some(oldest) => oldest.saturating_sub(1),
                None => head, // buffer truly empty (no PTY output yet)
            }
        }
        Some(x) => {
            if x >= head {
                // Client claims to know more than us — clamp to head, no replay.
                head
            } else {
                // Caller wants x+1..=head. Check buffer can satisfy.
                match snap.oldest_buffered {
                    Some(oldest) if x + 1 >= oldest => x,
                    Some(oldest) => {
                        gap_seq_lost = Some((x + 1, oldest.saturating_sub(1)));
                        head
                    }
                    None => head, // buffer empty; nothing to replay anyway
                }
            }
        }
    };
    let seq_start = cursor.saturating_add(1);

    if send_ctrl(
        &mut socket,
        &ServerControl::Hello {
            seq_start,
            agent_id: agent_id.clone(),
            shim_ready: lifecycle_snapshot.shim_ready,
            shim_exit: lifecycle_snapshot.shim_exit,
        },
    )
    .await
    .is_err()
    {
        return;
    }
    if let Some((lo, hi)) = gap_seq_lost {
        let _ = send_ctrl(
            &mut socket,
            &ServerControl::Error {
                message: format!(
                    "resume gap: seqs {lo}..={hi} evicted before reconnect; \
                     restarting from {seq_start}"
                ),
            },
        )
        .await;
    }

    info!(agent = %agent_id, %cursor, last_seq = ?params.last_seq, "pty ws attached");

    let (mut sender, mut receiver) = socket.split();

    // ---- server → client pump --------------------------------------------
    let agent_for_writer = agent_id.clone();
    let stream_for_writer = stream.clone();
    let slot_for_writer = slot_arc.clone();
    let mut lifecycle_rx = lifecycle_tx.subscribe();
    let writer_task = tokio::spawn(async move {
        let mut cursor = cursor;
        loop {
            // Drain everything currently available.
            match stream_for_writer.fetch_since(cursor) {
                crate::pty_stream::FetchResult::Ok(entries) => {
                    for (seq, bytes) in entries {
                        let frame = flockmux_protocol::ws_pty::pack_binary(seq, &bytes);
                        if sender.send(Message::Binary(frame)).await.is_err() {
                            debug!(agent = %agent_for_writer, "ws writer closed mid-drain");
                            return;
                        }
                        cursor = seq;
                    }
                }
                crate::pty_stream::FetchResult::Gap { current_seq } => {
                    // Buffer evicted bytes we promised. Tell client and jump
                    // forward; this should only happen if the writer falls
                    // behind producer by more than MAX_BUFFER_BYTES, which is
                    // a pathological case (slow network on heavy CLI output).
                    let _ = send_ctrl(
                        &mut sender,
                        &ServerControl::Error {
                            message: format!(
                                "byte buffer overran subscriber; jumped from \
                                 seq {cursor} to {current_seq}"
                            ),
                        },
                    )
                    .await;
                    cursor = current_seq;
                }
            }

            let snap = stream_for_writer.snapshot();
            if snap.closed && cursor >= snap.next_seq.saturating_sub(1) {
                // Drained + closed. Send Eof and exit.
                let _ = send_ctrl(&mut sender, &ServerControl::Eof).await;
                let _ = sender.close().await;
                return;
            }

            // Wait for new bytes OR a lifecycle event, whichever lands first.
            tokio::select! {
                _ = stream_for_writer.wait_changed(cursor) => {
                    // loop around to fetch_since
                }
                event = lifecycle_rx.recv() => {
                    match event {
                        Ok(LifecycleEvent::ShimReady) => {
                            let _ = send_ctrl(&mut sender, &ServerControl::ShimReady).await;
                        }
                        Ok(LifecycleEvent::ShimExit(code)) => {
                            let _ = send_ctrl(&mut sender, &ServerControl::ShimExit { code }).await;
                        }
                        Ok(LifecycleEvent::HealthFail { .. }) => {
                            // Auth/quota health failures surface via the swarm
                            // event stream (AgentState::Error), not the PTY
                            // control channel — nothing to relay to the terminal.
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                            // We missed a lifecycle event. The shim_ready /
                            // shim_exit snapshot in Hello covered initial
                            // state; if it changed since then we just lost
                            // the live signal. Re-query and synthesise.
                            // (broadcast capacity is 16 so this is unlikely.)
                            let snap_l = *slot_for_writer.lock().lifecycle.lock();
                            if snap_l.shim_ready {
                                let _ = send_ctrl(&mut sender, &ServerControl::ShimReady).await;
                            }
                            if let Some(code) = snap_l.shim_exit {
                                let _ =
                                    send_ctrl(&mut sender, &ServerControl::ShimExit { code }).await;
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            // Producer dropped — agent died. Loop will see
                            // stream.closed on next iter.
                        }
                    }
                }
            }
        }
    });

    // ---- client → server pump --------------------------------------------
    while let Some(msg) = receiver.next().await {
        let msg = match msg {
            Ok(m) => m,
            Err(err) => {
                warn!(?err, "ws recv error");
                break;
            }
        };
        match msg {
            Message::Binary(bytes) => {
                // Raw keystroke bytes from the browser. xterm's onData emits
                // strings, but the client encodes them as UTF-8 binary frames
                // so we don't re-encode here.
                if input_tx.send(Bytes::from(bytes)).await.is_err() {
                    debug!("input channel closed");
                    break;
                }
            }
            Message::Text(text) => {
                // Some clients (and xterm onData fallback) send text. Try
                // to parse as ClientControl; if that fails, treat the text
                // itself as input keystrokes (escape hatch for hand-testing
                // with `websocat`).
                match serde_json::from_str::<ClientControl>(&text) {
                    Ok(ctrl) => apply_control(ctrl, &slot_arc, &input_tx, &state, &agent_id).await,
                    Err(_) => {
                        if input_tx.send(Bytes::from(text.into_bytes())).await.is_err() {
                            break;
                        }
                    }
                }
            }
            Message::Ping(_) => {
                // axum auto-replies.
            }
            Message::Close(_) => break,
            _ => {}
        }
    }

    info!(agent = %agent_id, "pty ws detached");
    writer_task.abort();
    // Don't auto-kill the agent on WS detach — Detach control allows
    // background-running. Explicit `DELETE /api/agent/:id` (or `Kill` control)
    // tears it down.
}

async fn apply_control(
    ctrl: ClientControl,
    slot: &std::sync::Arc<parking_lot::Mutex<crate::registry::AgentSlot>>,
    input_tx: &mpsc::Sender<Bytes>,
    state: &AppState,
    agent_id: &str,
) {
    match ctrl {
        ClientControl::Resize { cols, rows } => {
            // No-op for non-PTY agents (ACP has no resizable terminal).
            if let Some(bridge) = slot.lock().pty_bridge() {
                if let Err(err) = bridge.resize(cols, rows) {
                    warn!(?err, "resize failed");
                }
            }
        }
        ClientControl::Ack { seq } => {
            // M3: parsed but not yet used as a back-pressure signal — the
            // ring buffer evicts on byte cap, not on ack. Ack is retained in
            // the protocol so we can wire selective retention later (e.g.,
            // recorder watermark, multi-attach slowest-consumer).
            let _ = seq;
        }
        ClientControl::Resume { last_seq } => {
            // Post-attach Resume is a no-op in M3 — clients should reconnect
            // with `?last_seq=N` instead, which avoids the mid-stream
            // cursor-rewind race. Logged for visibility.
            tracing::debug!(last_seq, "Resume control received post-attach; ignored");
        }
        ClientControl::Signal { sig } => {
            // Emit the raw control byte the PTY would deliver for a terminal
            // keypress. (Real signal delivery via kill(2) lands later.)
            let byte: u8 = match sig {
                Signal::Sigint => 0x03,  // Ctrl+C
                Signal::Sigterm => 0x03, // fall back to Ctrl+C semantically
                Signal::Sighup => 0x04,  // Ctrl+D
            };
            let _ = input_tx.send(Bytes::from(vec![byte])).await;
        }
        ClientControl::Kill => {
            // Full teardown — same path as REST DELETE /api/agent/:id (F1), so a
            // WS Kill can't leave a "zombie" half-removed from registry / swarm /
            // wake_subs. (Previously this only did bridge.kill().)
            crate::routes::rest::teardown_agent(state, agent_id).await;
        }
        ClientControl::Detach => {
            // Detach is implicit when the WS closes; nothing to do here
            // except acknowledge the message exists.
        }
    }
}
