//! `/ws/pty/:agent_id` — bidirectional PTY bridge.
//!
//! Server → client:
//!   * binary frame `[4B seq][PTY bytes]` (seq starts at 1, monotonic)
//!   * text frame: ServerControl JSON (Hello, ShimReady, ShimExit, Eof, Error)
//!
//! Client → server:
//!   * binary frame (raw keystroke bytes, ignored seq prefix not used here)
//!   * text frame: ClientControl JSON (Resize / Ack / Resume / Signal / Kill / Detach)
//!
//! M1 caveats:
//!   * No ring buffer yet → `Resume` is acknowledged with `Hello{seq_start: …}`
//!     but real replay lands in M3.
//!   * Ack is parsed and ignored (a future cursor for the ring buffer).
//!
//! Translation source: hermes-agent `hermes_cli/web_server.py:3402-3509`.

use crate::AppState;
use axum::{
    extract::{
        ws::{Message, WebSocket},
        Path, State, WebSocketUpgrade,
    },
    response::IntoResponse,
};
use bytes::Bytes;
use flockmux_protocol::ws_pty::{ClientControl, ServerControl, Signal};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

// Recognise the shim's OSC ready / exit markers in the PTY byte stream so we
// can surface them as structured `ServerControl` events. These match
// `flockmux-shim/src/main.rs`.
const OSC_READY: &[u8] = b"\x1b]633;A\x07";
const OSC_EXIT_PREFIX: &[u8] = b"\x1b]633;D;";
const OSC_TERMINATOR: u8 = 0x07;

pub async fn pty_ws(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state, agent_id))
}

async fn handle_socket(mut socket: WebSocket, state: AppState, agent_id: String) {
    let slot_arc = match state.registry.get(&agent_id) {
        Some(s) => s,
        None => {
            let _ = socket
                .send(Message::Text(
                    serde_json::to_string(&ServerControl::Error {
                        message: format!("agent {agent_id} not found"),
                    })
                    .unwrap(),
                ))
                .await;
            let _ = socket.close().await;
            return;
        }
    };

    // Claim the output receiver. Single-consumer for M1 — second attach
    // gets a clean error.
    let (output_rx, input_tx) = {
        let mut slot = slot_arc.lock();
        let rx = slot.output_rx.take();
        (rx, slot.input_tx.clone())
    };

    let mut output_rx = match output_rx {
        Some(rx) => rx,
        None => {
            let _ = socket
                .send(Message::Text(
                    serde_json::to_string(&ServerControl::Error {
                        message: "agent already attached; M2 will allow multi-attach".into(),
                    })
                    .unwrap(),
                ))
                .await;
            let _ = socket.close().await;
            return;
        }
    };

    // Send Hello first so the client knows it can start counting seq.
    let hello = serde_json::to_string(&ServerControl::Hello {
        seq_start: 1,
        agent_id: agent_id.clone(),
    })
    .unwrap();
    if socket.send(Message::Text(hello)).await.is_err() {
        return;
    }

    info!(agent = %agent_id, "pty ws attached");

    let seq = Arc::new(AtomicU32::new(0));

    // Split the socket for concurrent send / recv.
    let (mut sender, mut receiver) = socket.split();

    // ---- server → client pump ----
    let seq_writer = seq.clone();
    let agent_for_writer = agent_id.clone();
    let writer_task = tokio::spawn(async move {
        let mut osc_buf: Vec<u8> = Vec::new();
        while let Some(chunk) = output_rx.recv().await {
            // Scan the chunk for OSC ready / exit markers. Buffer across
            // chunk boundaries since OSC sequences can span reads.
            scan_osc(&mut osc_buf, &chunk, &mut sender).await;

            let next = seq_writer.fetch_add(1, Ordering::Relaxed) + 1;
            let frame = flockmux_protocol::ws_pty::pack_binary(next, &chunk);
            if sender.send(Message::Binary(frame)).await.is_err() {
                debug!(agent = %agent_for_writer, "ws writer closed");
                break;
            }
        }
        // PTY EOF → tell client.
        let eof = serde_json::to_string(&ServerControl::Eof).unwrap();
        let _ = sender.send(Message::Text(eof)).await;
        let _ = sender.close().await;
    });

    // ---- client → server pump ----
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
                    Ok(ctrl) => apply_control(ctrl, &slot_arc, &input_tx).await,
                    Err(_) => {
                        if input_tx.send(Bytes::from(text.into_bytes())).await.is_err() {
                            break;
                        }
                    }
                }
            }
            Message::Ping(p) => {
                // axum auto-replies, but be explicit for completeness.
                let _ = p;
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
    slot: &Arc<parking_lot::Mutex<crate::registry::AgentSlot>>,
    input_tx: &mpsc::Sender<Bytes>,
) {
    match ctrl {
        ClientControl::Resize { cols, rows } => {
            let bridge = slot.lock().bridge.clone();
            if let Err(err) = bridge.resize(cols, rows) {
                warn!(?err, "resize failed");
            }
        }
        ClientControl::Ack { seq } => {
            // M1: ignored. M3 will advance the ring buffer's confirmed
            // cursor here.
            let _ = seq;
        }
        ClientControl::Resume { last_seq } => {
            // M1: no replay. Just log so we know the client tried.
            let _ = last_seq;
        }
        ClientControl::Signal { sig } => {
            // Emit the raw control byte the PTY would deliver for a terminal
            // keypress. (Real signal delivery via kill(2) lands in M2.)
            let byte: u8 = match sig {
                Signal::Sigint => 0x03,  // Ctrl+C
                Signal::Sigterm => 0x03, // fall back to Ctrl+C semantically
                Signal::Sighup => 0x04,  // Ctrl+D
            };
            let _ = input_tx.send(Bytes::from(vec![byte])).await;
        }
        ClientControl::Kill => {
            let bridge = slot.lock().bridge.clone();
            bridge.kill();
        }
        ClientControl::Detach => {
            // Detach is implicit when the WS closes; nothing to do here
            // except acknowledge the message exists.
        }
    }
}

// ---- OSC scanner ---------------------------------------------------------

use axum::extract::ws::Message as WsMsg;
use futures::stream::SplitSink;
use futures::SinkExt;
use futures::StreamExt;

async fn send_text(sender: &mut SplitSink<WebSocket, WsMsg>, ev: ServerControl) {
    let _ = sender
        .send(Message::Text(serde_json::to_string(&ev).unwrap()))
        .await;
}

/// Look for `\x1b]633;A\x07` (ready) and `\x1b]633;D;<code>\x07` (exit) in
/// the PTY output. Anything found is surfaced as a `ServerControl` JSON
/// event *in addition* to the raw bytes being forwarded — the front-end
/// keeps writing them to the terminal as well (the OSC is harmless if the
/// terminal recognises it; ours doesn't render it).
async fn scan_osc(
    buf: &mut Vec<u8>,
    chunk: &[u8],
    sender: &mut SplitSink<WebSocket, WsMsg>,
) {
    buf.extend_from_slice(chunk);

    loop {
        // Cap buffer growth — OSC sequences are short, so anything beyond
        // a few hundred bytes without finding one means we're holding non-
        // OSC junk forever.
        if buf.len() > 4096 {
            let keep_from = buf.len() - 256;
            buf.drain(..keep_from);
        }

        if let Some(pos) = find(&buf, OSC_READY) {
            buf.drain(..pos + OSC_READY.len());
            send_text(sender, ServerControl::ShimReady).await;
            continue;
        }
        if let Some(pos) = find(&buf, OSC_EXIT_PREFIX) {
            let after = pos + OSC_EXIT_PREFIX.len();
            if let Some(end_rel) = buf[after..].iter().position(|&b| b == OSC_TERMINATOR) {
                let code_bytes = &buf[after..after + end_rel];
                let code = std::str::from_utf8(code_bytes)
                    .ok()
                    .and_then(|s| s.parse::<i32>().ok())
                    .unwrap_or(-1);
                buf.drain(..after + end_rel + 1);
                send_text(sender, ServerControl::ShimExit { code }).await;
                continue;
            } else {
                // Wait for more bytes.
                break;
            }
        }
        break;
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|w| w == needle)
}
