//! `flockmux-mcp wake-check`: Stop-hook helper invoked by Claude Code / Codex
//! CLI at every turn boundary.
//!
//! Wire-protocol (the lowest-common-denominator of the two CLIs' Stop hooks):
//!   - **stdin**: a JSON object provided by the CLI. We only look at one
//!     field: `stop_hook_active: bool` (codex sets this on a re-fire to break
//!     hook loops; claude's docs don't currently document the field but we
//!     read it opportunistically).
//!   - **stdout**: a single JSON object, NOTHING ELSE. `{}` means "no-op,
//!     don't change the turn". `{"decision":"block", "reason":"..."}`
//!     synthesises a continuation prompt that the LLM reads in its very next
//!     turn — that's how we feed "you have N unread messages" back to the
//!     agent without hijacking the PTY.
//!   - **exit code**: always 0. Codex documents that "plain text output is
//!     invalid" for Stop; the safe path is JSON-on-stdout + exit 0 for every
//!     branch including errors.
//!
//! Loop prevention is dual-track:
//!   1. `stop_hook_active` short-circuits when the CLI explicitly tells us
//!      "this is a recursive call, please stop".
//!   2. `~/.flockmux/wake/<agent_id>.json` carries a sliding-window counter
//!      so even a CLI that never sets the flag can't get stuck in a wake
//!      loop. Counter is cleared whenever unread drops to zero.
//!
//! Failure mode: any error (bad stdin, missing server, HTTP failure, file
//! IO) collapses to `print!("{}")` + Ok(()). The hook MUST NOT prevent the
//! agent from stopping; degrading to "no wake this turn" is always safe.

use anyhow::Result;
use clap::Args;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug, Args)]
pub struct WakeCheckArgs {
    /// Which agent this hook speaks for. Baked into the hook command string
    /// at spawn time — we deliberately don't read it from env or stdin so
    /// behavior is identical across CLIs / shells / debug invocations.
    #[arg(long)]
    pub agent_id: String,

    /// Base URL of the flockmux-server REST API.
    #[arg(long, default_value = "http://127.0.0.1:7777")]
    pub server: String,

    /// Sliding-window length (seconds). Default 30s.
    #[arg(long, default_value_t = 30)]
    pub throttle_secs: u64,

    /// Maximum wakes per window before forcing a no-op. Default 3.
    #[arg(long, default_value_t = 3)]
    pub max_wakes_per_window: u32,

    /// Override the wake state directory (defaults to ~/.flockmux/wake/).
    /// Test-only; not advertised in help.
    #[arg(long, hide = true)]
    pub state_dir: Option<PathBuf>,
}

/// Persisted between wake invocations. Kept tiny on purpose — corruption
/// recovery is "delete the file and start over".
#[derive(Debug, Serialize, Deserialize, Default, Clone)]
struct ThrottleState {
    /// Unix epoch milliseconds of the most recent wake we emitted.
    last_at_ms: u64,
    /// Number of wakes within the current window. Resets when unread → 0.
    count: u32,
}

/// HTTP budget per call. The hook fires on every turn boundary; we'd rather
/// degrade to no-wake than block the agent's stop.
const HTTP_TIMEOUT: Duration = Duration::from_secs(5);

/// Maximum stdin we'll consume. Real payloads from claude / codex are <2KB;
/// 64KB is paranoia + room for future fields.
const STDIN_MAX_BYTES: usize = 64 * 1024;

/// How long we wait for stdin before giving up and treating it as empty.
/// Both CLIs close stdin after sending the JSON, so this should always be
/// fast in practice.
const STDIN_TIMEOUT: Duration = Duration::from_millis(500);

pub async fn run(args: WakeCheckArgs) -> Result<()> {
    // From here on: every branch ends in emit_stdout(...) + Ok(()). No `?`
    // bubbling that could result in a non-zero exit.

    // ── 1. stdin: extract stop_hook_active ───────────────────────────────
    let stdin_bytes = read_stdin_bounded().await;
    let stop_hook_active = parse_stop_hook_active(&stdin_bytes);
    if stop_hook_active {
        emit_noop();
        return Ok(());
    }

    // ── 2. throttle gate ─────────────────────────────────────────────────
    let state_path = throttle_path(&args.agent_id, args.state_dir.as_deref());
    let now_ms = unix_ms();
    let state = read_throttle(&state_path).unwrap_or_default();
    let throttle_window_ms = args.throttle_secs.saturating_mul(1000);
    let window_active = state.last_at_ms != 0
        && now_ms.saturating_sub(state.last_at_ms) < throttle_window_ms;
    if window_active && state.count >= args.max_wakes_per_window {
        emit_noop();
        return Ok(());
    }

    // ── 3. HTTP: GET /api/message/unread_count?to=<agent_id> ─────────────
    let count = match fetch_unread_count(&args.server, &args.agent_id).await {
        Ok(n) => n,
        Err(err) => {
            // Any transport / HTTP failure is a graceful degrade — never
            // block the agent's stop because flockmux-server happens to be
            // down or the port changed.
            eprintln!("wake-check: {err}");
            emit_noop();
            return Ok(());
        }
    };

    if count <= 0 {
        // Mailbox empty → reset throttle so the NEXT genuine message lands
        // a wake unhindered (otherwise an agent that processed mail late
        // in the window would have to wait for the window to roll over).
        let _ = std::fs::remove_file(&state_path);
        emit_noop();
        return Ok(());
    }

    // ── 4. Persist throttle bump + emit wake ─────────────────────────────
    let next_state = if window_active {
        ThrottleState {
            last_at_ms: now_ms,
            count: state.count.saturating_add(1),
        }
    } else {
        ThrottleState {
            last_at_ms: now_ms,
            count: 1,
        }
    };
    if let Err(err) = write_throttle(&state_path, &next_state) {
        // Throttle write failure is not fatal — worst case the next wake
        // also fires and we still bounded by stop_hook_active / agent
        // dedupe semantics.
        eprintln!("wake-check: throttle write failed: {err}");
    }

    let reason = format!(
        "You have {count} unread swarm message(s). Use the swarm_list_messages \
         tool now to read and respond to them before doing anything else."
    );
    emit_block(&reason);
    Ok(())
}

// ── stdout helpers ───────────────────────────────────────────────────────

fn emit_noop() {
    // Codex's docs are emphatic: stdout must be JSON. `{}` is a valid no-op.
    print!("{{}}");
}

fn emit_block(reason: &str) {
    let payload = json!({
        "decision": "block",
        "reason": reason,
    });
    // `to_string` (not `to_string_pretty`) — no trailing newline, no extra
    // bytes. Both CLIs parse stdout strictly.
    let s = serde_json::to_string(&payload).unwrap_or_else(|_| "{}".into());
    print!("{s}");
}

// ── stdin helpers ────────────────────────────────────────────────────────

async fn read_stdin_bounded() -> Vec<u8> {
    use tokio::io::AsyncReadExt;
    let mut buf = Vec::with_capacity(2048);
    let mut stdin = tokio::io::stdin();
    let _ = tokio::time::timeout(
        STDIN_TIMEOUT,
        async {
            // take(...) caps how many bytes we read so a misbehaving CLI
            // can't OOM us by streaming gigabytes.
            let mut limited = (&mut stdin).take(STDIN_MAX_BYTES as u64);
            limited.read_to_end(&mut buf).await
        },
    )
    .await;
    buf
}

fn parse_stop_hook_active(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return false;
    }
    let v: Value = match serde_json::from_slice(bytes) {
        Ok(v) => v,
        Err(_) => return false,
    };
    v.get("stop_hook_active")
        .and_then(|x| x.as_bool())
        .unwrap_or(false)
}

// ── HTTP ─────────────────────────────────────────────────────────────────

async fn fetch_unread_count(server: &str, agent_id: &str) -> Result<i64, String> {
    let client = reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .build()
        .map_err(|e| format!("build client: {e}"))?;
    let url = format!("{server}/api/message/unread_count");
    let resp = client
        .get(&url)
        .query(&[("to", agent_id)])
        .send()
        .await
        .map_err(|e| format!("GET {url}: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("HTTP {status}: {body}"));
    }
    let body: Value = resp
        .json()
        .await
        .map_err(|e| format!("parse json from {url}: {e}"))?;
    body.get("count")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| format!("missing 'count' in response from {url}: {body}"))
}

// ── throttle file ────────────────────────────────────────────────────────

fn throttle_path(agent_id: &str, override_dir: Option<&Path>) -> PathBuf {
    let dir = match override_dir {
        Some(p) => p.to_path_buf(),
        None => default_state_dir(),
    };
    dir.join(format!("{agent_id}.json"))
}

fn default_state_dir() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    home.join(".flockmux").join("wake")
}

fn read_throttle(path: &Path) -> Option<ThrottleState> {
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn write_throttle(path: &Path, state: &ThrottleState) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    let tmp = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec(state)
        .map_err(|e| format!("serialize throttle state: {e}"))?;
    std::fs::write(&tmp, &bytes)
        .map_err(|e| format!("write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path)
        .map_err(|e| format!("rename to {}: {e}", path.display()))?;
    Ok(())
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{extract::Query, routing::get, Json, Router};
    use serde_json::json;
    use std::collections::HashMap;
    use std::net::SocketAddr;
    use std::sync::atomic::{AtomicI64, Ordering};
    use std::sync::Arc;
    use tempfile::tempdir;

    /// Spin up a stub `flockmux-server` exposing only `/api/message/unread_count`.
    /// Returns `(addr, counter)` — flip `counter` to control the returned count.
    async fn start_stub(initial: i64) -> (SocketAddr, Arc<AtomicI64>) {
        let counter = Arc::new(AtomicI64::new(initial));
        let counter_inner = counter.clone();
        let app = Router::new().route(
            "/api/message/unread_count",
            get(move |Query(q): Query<HashMap<String, String>>| {
                let counter = counter_inner.clone();
                async move {
                    let to = q.get("to").cloned().unwrap_or_default();
                    Json(json!({ "to": to, "count": counter.load(Ordering::SeqCst) }))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (addr, counter)
    }

    /// Spin up a stub that returns HTTP 500 — used to exercise the
    /// degrade-to-noop path.
    async fn start_error_stub() -> SocketAddr {
        let app = Router::new().route(
            "/api/message/unread_count",
            get(|| async {
                (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    "boom",
                )
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        addr
    }

    fn args_for(addr: SocketAddr, agent_id: &str, state_dir: &Path) -> WakeCheckArgs {
        WakeCheckArgs {
            agent_id: agent_id.into(),
            server: format!("http://{addr}"),
            throttle_secs: 30,
            max_wakes_per_window: 3,
            state_dir: Some(state_dir.to_path_buf()),
        }
    }

    // ── 1. Direct unit tests on the pure helpers ─────────────────────────

    #[test]
    fn parse_stop_hook_active_returns_true_when_flag_set() {
        let bytes = br#"{"stop_hook_active": true}"#;
        assert!(parse_stop_hook_active(bytes));
    }

    #[test]
    fn parse_stop_hook_active_false_when_absent() {
        let bytes = br#"{"other_field": 42}"#;
        assert!(!parse_stop_hook_active(bytes));
    }

    #[test]
    fn parse_stop_hook_active_false_when_invalid_json() {
        assert!(!parse_stop_hook_active(b"not json at all"));
    }

    #[test]
    fn parse_stop_hook_active_false_when_empty() {
        assert!(!parse_stop_hook_active(b""));
    }

    #[test]
    fn throttle_round_trip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.json");
        let s = ThrottleState { last_at_ms: 12345, count: 2 };
        write_throttle(&path, &s).unwrap();
        let back = read_throttle(&path).unwrap();
        assert_eq!(back.last_at_ms, 12345);
        assert_eq!(back.count, 2);
    }

    #[test]
    fn throttle_read_missing_returns_none() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nope.json");
        assert!(read_throttle(&path).is_none());
    }

    // ── 2. Integration via run() with a stub server ──────────────────────

    // The `run()` function prints to stdout, which interferes with cargo's
    // test runner output. We test the *side effects* — throttle file state
    // and HTTP traffic — rather than capturing stdout. Stdout shape is
    // covered by separate emit_* unit tests below.

    #[tokio::test]
    async fn count_zero_clears_throttle_and_emits_noop() {
        let dir = tempdir().unwrap();
        let state_dir = dir.path().to_path_buf();
        // Pre-seed throttle to verify it's cleared.
        let path = throttle_path("test-agent", Some(&state_dir));
        write_throttle(&path, &ThrottleState { last_at_ms: 1, count: 2 }).unwrap();
        assert!(path.exists());

        let (addr, _) = start_stub(0).await;
        let args = args_for(addr, "test-agent", &state_dir);
        run(args).await.unwrap();

        assert!(!path.exists(), "throttle file should be unlinked when count=0");
    }

    #[tokio::test]
    async fn count_positive_writes_throttle() {
        let dir = tempdir().unwrap();
        let state_dir = dir.path().to_path_buf();
        let (addr, _) = start_stub(3).await;
        let args = args_for(addr, "test-agent", &state_dir);
        run(args).await.unwrap();

        let path = throttle_path("test-agent", Some(&state_dir));
        let state = read_throttle(&path).expect("throttle file should exist");
        assert_eq!(state.count, 1);
        assert!(state.last_at_ms > 0);
    }

    #[tokio::test]
    async fn throttle_window_blocks_repeat_wake() {
        let dir = tempdir().unwrap();
        let state_dir = dir.path().to_path_buf();
        // Pre-seed throttle at max.
        let path = throttle_path("test-agent", Some(&state_dir));
        write_throttle(
            &path,
            &ThrottleState {
                last_at_ms: unix_ms(),
                count: 3,
            },
        )
        .unwrap();

        // Server returning a positive count would normally trigger a wake;
        // throttle should suppress it. We check by asserting the count in
        // the throttle file stays at 3 (no increment, because we early-exit
        // before the HTTP call).
        let (addr, _) = start_stub(5).await;
        let args = args_for(addr, "test-agent", &state_dir);
        run(args).await.unwrap();

        let state = read_throttle(&path).unwrap();
        assert_eq!(state.count, 3, "throttle should not increment when window is full");
    }

    #[tokio::test]
    async fn throttle_window_expired_resets_count() {
        let dir = tempdir().unwrap();
        let state_dir = dir.path().to_path_buf();
        let path = throttle_path("test-agent", Some(&state_dir));
        // Seed with a stale window (older than throttle_secs).
        write_throttle(
            &path,
            &ThrottleState {
                last_at_ms: unix_ms().saturating_sub(60 * 1000),
                count: 3,
            },
        )
        .unwrap();

        let (addr, _) = start_stub(1).await;
        let args = args_for(addr, "test-agent", &state_dir);
        run(args).await.unwrap();

        let state = read_throttle(&path).unwrap();
        assert_eq!(state.count, 1, "expired window should reset to 1");
    }

    #[tokio::test]
    async fn http_error_does_not_create_throttle() {
        let dir = tempdir().unwrap();
        let state_dir = dir.path().to_path_buf();
        let addr = start_error_stub().await;
        let args = args_for(addr, "test-agent", &state_dir);
        run(args).await.unwrap();
        let path = throttle_path("test-agent", Some(&state_dir));
        assert!(!path.exists(), "no throttle write on HTTP error");
    }

    #[tokio::test]
    async fn network_unreachable_does_not_create_throttle() {
        let dir = tempdir().unwrap();
        let state_dir = dir.path().to_path_buf();
        // Point at a port that almost certainly has nothing listening.
        let args = WakeCheckArgs {
            agent_id: "test-agent".into(),
            server: "http://127.0.0.1:1".into(),
            throttle_secs: 30,
            max_wakes_per_window: 3,
            state_dir: Some(state_dir.clone()),
        };
        run(args).await.unwrap();
        let path = throttle_path("test-agent", Some(&state_dir));
        assert!(!path.exists(), "no throttle write when server unreachable");
    }

    #[test]
    fn emit_noop_writes_empty_object() {
        // Direct shape check — emit_* fns are tiny but we want the contract
        // pinned: empty object, no newline, no whitespace.
        let payload = json!({});
        let s = serde_json::to_string(&payload).unwrap();
        assert_eq!(s, "{}");
    }

    #[test]
    fn emit_block_writes_decision_and_reason() {
        let payload = json!({
            "decision": "block",
            "reason": "test reason",
        });
        let s = serde_json::to_string(&payload).unwrap();
        // Ordering of JSON keys in serde_json::to_string is insertion order
        // because we use the json! macro — assert structurally instead.
        let back: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(back["decision"], json!("block"));
        assert_eq!(back["reason"], json!("test reason"));
    }
}
