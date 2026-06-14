//! `/ws/swarm` — broadcast feed of every `SwarmEvent` the server produces.
//!
//! No resume / no per-subscriber cursor: this is a live feed only. UIs that
//! need history hit `/api/message` and `/api/blackboard` first, then attach
//! the WS for incremental updates.
//!
//! A slow subscriber that lags past the broadcast channel's capacity
//! (1024, see `Swarm::new` in `flockmux-swarm/src/swarm.rs`) is
//! disconnected — the alternative is unbounded memory growth.

use crate::AppState;
use axum::{
    extract::{
        ws::{Message, WebSocket},
        State, WebSocketUpgrade,
    },
    response::IntoResponse,
};
use futures::{SinkExt, StreamExt};
use std::time::Duration;
use tokio::sync::broadcast::error::RecvError;
use tracing::{debug, warn};

/// How often we ping an otherwise-idle subscriber. A swarm feed can sit silent
/// for a long time (no agents running); without traffic a client that vanished
/// (laptop slept, network dropped, tab killed without a clean Close) is
/// invisible to us and its broadcast subscription + this task leak forever.
/// The ping forces a write on the socket so a dead peer surfaces as a send
/// error, and gives any intermediary a keepalive.
const HEARTBEAT: Duration = Duration::from_secs(30);

pub async fn ws_swarm(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle(socket, state))
}

async fn handle(socket: WebSocket, state: AppState) {
    let (mut sender, mut receiver) = socket.split();
    let mut rx = state.swarm.subscribe();
    let mut heartbeat = tokio::time::interval(HEARTBEAT);
    // First tick fires immediately; skip it so we don't ping the instant a
    // client connects, and never let a stall queue up a burst of pings.
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    heartbeat.tick().await;

    loop {
        tokio::select! {
            // Broadcast event → forward to the client.
            recv = rx.recv() => match recv {
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
                    let _ = sender
                        .send(Message::Text(format!(
                            "{{\"type\":\"error\",\"message\":\"swarm subscriber lagged {n} events; reconnect to resync\"}}"
                        )))
                        .await;
                    debug!(lagged = n, "ws/swarm subscriber lagged; closing");
                    break;
                }
                Err(RecvError::Closed) => {
                    debug!("ws/swarm broadcast channel closed");
                    break;
                }
            },

            // Client→server frames. We define no commands yet, but we MUST poll
            // the read half: it's how we observe the peer closing (or a recv
            // error) on an otherwise-idle feed. `None`/`Err` here = the client
            // is gone, so break and clean up instead of leaking the connection
            // and its broadcast subscription until the next (maybe never) event.
            incoming = receiver.next() => match incoming {
                Some(Ok(Message::Close(_))) | None => {
                    debug!("ws/swarm client closed");
                    break;
                }
                Some(Ok(_)) => {
                    // Pong / Ping (axum auto-replies to Ping) / unexpected data:
                    // no commands defined yet, drop it.
                }
                Some(Err(e)) => {
                    debug!(?e, "ws/swarm client recv error; closing");
                    break;
                }
            },

            // Idle keepalive: ping so a silently-dead peer surfaces as a send
            // error here (or on the next event) rather than lingering.
            _ = heartbeat.tick() => {
                if sender.send(Message::Ping(Vec::new())).await.is_err() {
                    debug!("ws/swarm heartbeat ping failed; client gone");
                    break;
                }
            }
        }
    }
}
