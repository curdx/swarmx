//! `/ws/swarm` — broadcast feed of every `SwarmEvent` the server produces.
//!
//! No resume / no per-subscriber cursor: this is a live feed only. UIs that
//! need history hit `/api/message` and `/api/blackboard` first, then attach
//! the WS for incremental updates.
//!
//! A slow subscriber that lags past the broadcast channel's capacity
//! (256) is disconnected — the alternative is unbounded memory growth.

use crate::AppState;
use axum::{
    extract::{
        ws::{Message, WebSocket},
        State, WebSocketUpgrade,
    },
    response::IntoResponse,
};
use flockmux_protocol::ws_swarm::SwarmEvent;
use futures::{SinkExt, StreamExt};
use tokio::sync::broadcast::error::RecvError;
use tracing::{debug, warn};

pub async fn ws_swarm(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle(socket, state))
}

async fn handle(socket: WebSocket, state: AppState) {
    let (mut sender, mut receiver) = socket.split();
    let mut rx = state.swarm.subscribe();

    // Background reader: just drain any client→server frames so the WS
    // keeps draining its read buffer; we don't process commands here yet.
    let reader_task = tokio::spawn(async move {
        while let Some(Ok(_msg)) = receiver.next().await {
            // No commands defined yet for /ws/swarm. Silently drop.
        }
    });

    loop {
        match rx.recv().await {
            Ok(ev) => {
                let payload = match serde_json::to_string(&ev) {
                    Ok(s) => s,
                    Err(e) => {
                        warn!(?e, "ws/swarm serialise failed");
                        continue;
                    }
                };
                if sender.send(Message::Text(payload)).await.is_err() {
                    debug!("ws/swarm client disconnected");
                    break;
                }
            }
            Err(RecvError::Lagged(n)) => {
                let warn_payload = serde_json::to_string(&SwarmEvent::AgentState {
                    agent_id: "__system__".into(),
                    state: flockmux_protocol::ws_swarm::AgentState::Idle,
                })
                .unwrap_or_default();
                let _ = sender
                    .send(Message::Text(format!(
                        "{{\"type\":\"error\",\"message\":\"swarm subscriber lagged {n} events; reconnect to resync\"}}"
                    )))
                    .await;
                debug!(lagged = n, "ws/swarm subscriber lagged; closing");
                let _ = warn_payload; // silence unused
                break;
            }
            Err(RecvError::Closed) => {
                debug!("ws/swarm broadcast channel closed");
                break;
            }
        }
    }
    reader_task.abort();
}
