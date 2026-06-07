//! `/ws/terminal` — a plain interactive shell in the browser.
//!
//! Distinct from `/ws/pty/:agent_id` (which bridges a *worker's* PTY with the
//! seq/Hello/Resume/recording protocol): this is a throwaway `$SHELL` the
//! developer opens for ad-hoc commands. No resume, no recording, no registry —
//! a fresh shell each attach, killed when the socket closes. Protocol is
//! deliberately minimal:
//!   server → client: binary frames = raw PTY bytes
//!   client → server: binary = keystrokes; text = `{"type":"resize","cols","rows"}`
//!
//! Gated by the global `require_local_origin` middleware (WS upgrade is an HTTP
//! request and carries an Origin). The shell inherits the same env allowlist as
//! workers (flockmux-pty `env_clear` + the vars we set), so it doesn't leak the
//! server's ambient secrets.

use axum::{
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    response::IntoResponse,
};
use bytes::Bytes;
use flockmux_pty::{PtyBridge, PtyHandles, SpawnOpts};
use futures::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::path::PathBuf;
use tracing::{debug, warn};

pub async fn terminal_ws(ws: WebSocketUpgrade) -> impl IntoResponse {
    ws.on_upgrade(handle_terminal)
}

async fn handle_terminal(socket: WebSocket) {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
    let home = std::env::var_os("HOME").map(PathBuf::from);

    // Allowlist: what an interactive shell legitimately needs. flockmux-pty
    // clears the rest, so the server's ambient secrets don't reach the shell.
    let mut env = HashMap::new();
    for k in ["HOME", "PATH", "LANG", "LC_ALL", "LC_CTYPE", "TMPDIR", "USER", "LOGNAME"] {
        if let Ok(v) = std::env::var(k) {
            env.insert(k.to_string(), v);
        }
    }
    env.entry("TERM".to_string()).or_insert_with(|| "xterm-256color".to_string());

    let argv = vec![shell.clone()];
    let opts = SpawnOpts {
        argv: &argv,
        cwd: home.as_deref(),
        env,
        cols: 80,
        rows: 24,
    };
    let PtyHandles { bridge, mut output_rx } = match PtyBridge::spawn(opts) {
        Ok(h) => h,
        Err(e) => {
            warn!(?e, %shell, "terminal: failed to spawn shell");
            return;
        }
    };
    debug!(%shell, "terminal: shell spawned");
    let input = bridge.input_sender();
    let (mut sink, mut stream) = socket.split();

    // PTY output → WS binary frames.
    let writer = tokio::spawn(async move {
        while let Some(chunk) = output_rx.recv().await {
            if sink.send(Message::Binary(chunk.to_vec())).await.is_err() {
                break; // client gone
            }
        }
        // PTY EOF (shell exited) → close the socket.
        let _ = sink.send(Message::Close(None)).await;
    });

    // WS → PTY (keystrokes) + resize control.
    while let Some(Ok(msg)) = stream.next().await {
        match msg {
            Message::Binary(b) => {
                if input.send(Bytes::from(b)).await.is_err() {
                    break; // shell gone
                }
            }
            Message::Text(t) => {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&t) {
                    if v.get("type").and_then(|x| x.as_str()) == Some("resize") {
                        let cols = v.get("cols").and_then(|x| x.as_u64()).unwrap_or(80) as u16;
                        let rows = v.get("rows").and_then(|x| x.as_u64()).unwrap_or(24) as u16;
                        let _ = bridge.resize(cols, rows);
                    }
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }

    // Socket closed or shell input channel dead → tear down: aborting the
    // writer drops output_rx, and dropping `bridge` kills the shell process
    // group (PtyBridge::Drop).
    writer.abort();
    drop(bridge);
    debug!("terminal: session closed");
}
