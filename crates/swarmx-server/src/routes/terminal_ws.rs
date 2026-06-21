//! `/ws/terminal` — a persistent interactive shell in the browser.
//!
//! Distinct from `/ws/pty/:agent_id` (which bridges a *worker's* PTY with the
//! seq/Hello/Resume/recording protocol): this is a `$SHELL` the developer opens
//! for ad-hoc commands. Unlike a worker PTY it has no recording or resume
//! protocol, but it IS persistent across navigation: the client passes a stable
//! `?session=<id>` (one per browser tab, kept in sessionStorage) and the PTY
//! lives in a process-wide registry keyed by that id. Closing the socket
//! (navigating away, F5) detaches but does NOT kill the shell — a reattach
//! replays the scrollback ring and resumes streaming, so a running command or
//! REPL survives a page switch. A reaper kills sessions left with no attached
//! client for `IDLE_REAP` so abandoned tabs don't leak shells.
//!
//! Protocol (unchanged, deliberately minimal):
//!   server → client: binary frames = raw PTY bytes
//!   client → server: binary = keystrokes; text = `{"type":"resize","cols","rows"}`
//!
//! Gated by the global `require_local_origin` middleware (WS upgrade is an HTTP
//! request and carries an Origin). The shell inherits the same env allowlist as
//! workers (swarmx-pty `env_clear` + the vars we set), so it doesn't leak the
//! server's ambient secrets.
//!
//! ## SECURITY: the `session` id is a bearer capability for a full `$SHELL`
//!
//! Whoever presents a given `?session=<id>` attaches to *that* shell — full
//! keystroke injection and scrollback (which may hold secrets the user typed).
//! The id is client-chosen and the server does NOT bind it to any identity, so
//! it is effectively an unauthenticated capability token. Two things keep this
//! safe TODAY, and a change to either reopens the hole:
//!
//!   1. `require_local_origin` makes this endpoint reachable only from a
//!      loopback origin — i.e. the single local user's own browser/native
//!      client. There is no remote attacker who can present a guessed id.
//!   2. The id space is large/opaque in practice (the client uses a UUID, or a
//!      per-tab/per-workspace random string in sessionStorage), so even sibling
//!      tabs of the same user don't accidentally collide onto one shell.
//!
//! We do NOT server-generate-and-pin the id, because the whole point is that a
//! reload/reattach with the SAME id resumes the SAME shell (scrollback replay).
//! What we DO enforce here is shape validation (`valid_session_id`): a junk or
//! oversized id never becomes a registry key and never attaches to a foreign
//! session — it falls back to a fresh ephemeral shell. If this endpoint ever
//! becomes reachable cross-origin or multi-user, this id MUST be replaced with
//! (or wrapped by) a server-issued, identity-scoped token.

use crate::AppState;
use axum::{
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    extract::{Query, State},
    response::IntoResponse,
};
use bytes::Bytes;
use swarmx_pty::{PtyBridge, PtyHandles, SpawnOpts};
use futures::{SinkExt, StreamExt};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;
use tracing::{debug, warn};

/// Lock a `std::sync::Mutex`, recovering from poisoning instead of propagating
/// it. A poisoned mutex means *some other* task panicked while holding the lock;
/// `.lock().unwrap()` would then panic in turn, and because these are
/// process-wide statics (the session registry and each session's scrollback
/// ring) the poison is permanent — one stray panic would brick the ENTIRE
/// terminal subsystem for every session until the server restarts.
///
/// The data these locks guard is plain bookkeeping (a `HashMap` of sessions, a
/// scrollback `VecDeque`); a panic mid-mutation can at worst leave a half-pushed
/// chunk, never an unsafe invariant. So we recover the guard and carry on rather
/// than weaponising one panic into a total outage.
fn lock_recover<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| {
        warn!("terminal: recovering from poisoned mutex (a prior task panicked)");
        poisoned.into_inner()
    })
}

/// Bytes of raw PTY output kept for replay on reattach. Visual state (cursor,
/// colours, alt-screen) is reconstructed by feeding the escape-sequence stream
/// back to a fresh xterm, so this is just the scrollback the user re-sees.
const SCROLLBACK_CAP: usize = 256 * 1024;
/// Live-output fanout depth (chunks). A client that lags past this loses
/// intermediate bytes (logged) but stays usable — fine for a dev shell.
const BCAST_DEPTH: usize = 1024;
/// A session left with zero attached clients this long is reclaimed.
const IDLE_REAP: Duration = Duration::from_secs(30 * 60);

/// One live shell, kept alive across socket attach/detach.
struct TermSession {
    /// Held here so the shell stays alive while detached; dropping the last
    /// `Arc` (session removed from the registry) kills the process group.
    bridge: Arc<PtyBridge>,
    /// Scrollback ring (chunks + running byte total) replayed on reattach.
    ring: Arc<Mutex<(VecDeque<Bytes>, usize)>>,
    /// Live PTY-output fanout; each attach subscribes.
    bcast: broadcast::Sender<Bytes>,
    /// Attached-client count; stamps `detached_at` when it hits 0.
    attached: usize,
    detached_at: Option<Instant>,
}

/// Process-wide terminal-session registry. Lazily initialised (also arms the
/// idle reaper). Mirrors the `OnceLock<Mutex<HashMap>>` caches elsewhere.
fn registry() -> &'static Mutex<HashMap<String, TermSession>> {
    static R: OnceLock<Mutex<HashMap<String, TermSession>>> = OnceLock::new();
    R.get_or_init(|| {
        spawn_reaper();
        Mutex::new(HashMap::new())
    })
}

/// Every minute, reclaim sessions that have been detached for `IDLE_REAP`.
/// Removing a `TermSession` drops its `Arc<PtyBridge>` → the shell is killed.
fn spawn_reaper() {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(60));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            let now = Instant::now();
            let mut reg = lock_recover(registry());
            reg.retain(|sid, s| {
                let stale = s.attached == 0
                    && s.detached_at
                        .map(|t| now.duration_since(t) >= IDLE_REAP)
                        .unwrap_or(false);
                if stale {
                    debug!(%sid, "terminal: reaping idle-detached session");
                }
                !stale
            });
        }
    });
}

/// Spawn a fresh `$SHELL` with the worker env allowlist (swarmx-pty clears the
/// rest, so the server's ambient secrets don't reach the shell). Starts in
/// `cwd` (the picked workspace's root) when given, else $HOME.
fn spawn_shell(cwd: Option<PathBuf>) -> anyhow::Result<PtyHandles> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
    let spawn_cwd = cwd.or_else(|| std::env::var_os("HOME").map(PathBuf::from));

    let mut env = HashMap::new();
    for k in [
        "HOME", "PATH", "LANG", "LC_ALL", "LC_CTYPE", "TMPDIR", "USER", "LOGNAME",
    ] {
        if let Ok(v) = std::env::var(k) {
            env.insert(k.to_string(), v);
        }
    }
    env.entry("TERM".to_string())
        .or_insert_with(|| "xterm-256color".to_string());

    let argv = vec![shell];
    PtyBridge::spawn(SpawnOpts {
        argv: &argv,
        cwd: spawn_cwd.as_deref(),
        env,
        cols: 80,
        rows: 24,
    })
}

#[derive(serde::Deserialize)]
pub struct TermQuery {
    /// Stable per-tab session id. Absent → an ephemeral one-shot session.
    session: Option<String>,
    /// Workspace whose `cwd` the shell starts in. Only honoured on the FIRST
    /// spawn of a session id (a reattach resumes the existing shell wherever
    /// the user `cd`'d it); per-workspace session ids on the client keep this
    /// a non-issue — each workspace has its own first spawn.
    workspace_id: Option<String>,
}

/// Longest client session id we'll use as a registry key. The legit client
/// sends a UUID (36 chars) or a short per-workspace token; anything past this is
/// junk (or an attempt to bloat the keyspace), so we don't honour it.
const MAX_SESSION_ID_LEN: usize = 128;

/// Shape check for a client-supplied `?session=<id>` before it becomes a
/// registry key (and thus a capability handle onto a live `$SHELL` — see the
/// module-level SECURITY note). We accept only a bounded, opaque token charset
/// (the UUIDs / random per-tab strings legit clients actually send): ASCII
/// alphanumerics plus `-` and `_`. A failing id is NOT rejected with an error
/// (that would needlessly break the shell for a quirky-but-harmless client);
/// the caller instead falls back to a fresh server-generated ephemeral id, so a
/// junk value can never attach to — or squat — a foreign session's registry key.
fn valid_session_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= MAX_SESSION_ID_LEN
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

pub async fn terminal_ws(
    State(state): State<AppState>,
    ws: WebSocketUpgrade,
    Query(q): Query<TermQuery>,
) -> impl IntoResponse {
    // Only a well-formed id may become a registry key (= a capability handle on
    // a live shell). A junk/oversized id is dropped here and replaced by a fresh
    // server-generated one, so it gets its own throwaway shell instead of
    // squatting or hijacking another session's key. See the module SECURITY note.
    let sid = q
        .session
        .filter(|s| valid_session_id(s))
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    // Resolve the workspace cwd server-side (don't trust a raw client path).
    let cwd = match q.workspace_id.as_deref().filter(|s| !s.is_empty()) {
        Some(id) => state
            .store
            .get_workspace_by_id(id.to_string())
            .await
            .ok()
            .flatten()
            .map(|w| PathBuf::from(w.cwd)),
        None => None,
    };
    ws.on_upgrade(move |sock| handle_terminal(sock, sid, cwd))
}

async fn handle_terminal(socket: WebSocket, sid: String, cwd: Option<PathBuf>) {
    // Attach to an existing session or create one. The replay snapshot and the
    // live subscribe both happen under the ring lock (the pump appends+sends
    // under the same lock) so the pump can't slip a chunk between them — no
    // duplicate and no gap at the replay→live boundary.
    let (bridge, mut rx, replay) = {
        let mut reg = lock_recover(registry());
        if let Some(s) = reg.get_mut(&sid) {
            s.attached += 1;
            s.detached_at = None;
            let guard = lock_recover(&s.ring);
            let rx = s.bcast.subscribe();
            let replay: Vec<u8> = guard.0.iter().flat_map(|b| b.iter().copied()).collect();
            drop(guard);
            debug!(%sid, "terminal: reattached (replay {} bytes)", replay.len());
            (s.bridge.clone(), rx, replay)
        } else {
            let PtyHandles {
                bridge,
                mut output_rx,
            } = match spawn_shell(cwd) {
                Ok(h) => h,
                Err(e) => {
                    warn!(?e, "terminal: failed to spawn shell");
                    return;
                }
            };
            let bridge = Arc::new(bridge);
            let ring = Arc::new(Mutex::new((VecDeque::<Bytes>::new(), 0usize)));
            let (bcast, rx) = broadcast::channel::<Bytes>(BCAST_DEPTH);
            // Pump: drain PTY output for the session's whole lifetime (not just
            // while a client is attached) into the scrollback ring + fanout.
            {
                let ring = ring.clone();
                let bcast = bcast.clone();
                let sid = sid.clone();
                tokio::spawn(async move {
                    while let Some(chunk) = output_rx.recv().await {
                        let mut g = lock_recover(&ring);
                        g.1 += chunk.len();
                        g.0.push_back(chunk.clone());
                        while g.1 > SCROLLBACK_CAP {
                            match g.0.pop_front() {
                                Some(old) => g.1 -= old.len(),
                                None => break,
                            }
                        }
                        // Send under the ring lock so a concurrent attach's
                        // (subscribe + snapshot) is serialised against it.
                        let _ = bcast.send(chunk);
                    }
                    // PTY EOF (shell exited) → drop the session so a later
                    // attach with the same id starts a fresh shell.
                    lock_recover(registry()).remove(&sid);
                    debug!(%sid, "terminal: shell exited, session removed");
                });
            }
            reg.insert(
                sid.clone(),
                TermSession {
                    bridge: bridge.clone(),
                    ring,
                    bcast,
                    attached: 1,
                    detached_at: None,
                },
            );
            debug!(%sid, "terminal: shell spawned");
            (bridge, rx, Vec::new())
        }
    };

    let input = bridge.input_sender();
    let (mut sink, mut stream) = socket.split();

    // PTY output → WS: replay the scrollback first, then stream live chunks.
    let writer = tokio::spawn(async move {
        if !replay.is_empty() && sink.send(Message::Binary(replay)).await.is_err() {
            return; // client gone before we finished replaying
        }
        loop {
            match rx.recv().await {
                Ok(chunk) => {
                    if sink.send(Message::Binary(chunk.to_vec())).await.is_err() {
                        break; // client gone
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    debug!(skipped = n, "terminal: client lagged, dropping frames");
                }
                Err(broadcast::error::RecvError::Closed) => {
                    // Pump ended (shell exited) → close the socket.
                    let _ = sink.send(Message::Close(None)).await;
                    break;
                }
            }
        }
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

    // Socket closed (navigation / reload / shell exit). Detach WITHOUT killing
    // the shell: stop this client's writer, decrement the attach count, and
    // stamp detached_at when we're the last viewer so the reaper can reclaim it.
    // If the shell already exited, the pump removed the session — get_mut is a
    // no-op then.
    writer.abort();
    {
        let mut reg = lock_recover(registry());
        if let Some(s) = reg.get_mut(&sid) {
            s.attached = s.attached.saturating_sub(1);
            if s.attached == 0 {
                s.detached_at = Some(Instant::now());
            }
        }
    }
    debug!(%sid, "terminal: client detached (session kept alive)");
}
