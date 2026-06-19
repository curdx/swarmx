//! Engine real-usability probe.
//!
//! The honest answer to "which engines can I actually use right now" is NOT
//! "which binaries are installed" (that's what `cli_plugin_info`'s `installed`
//! reports) nor "is there a credential file" (a file existing ≠ the token being
//! valid). It's: **really start the engine over PTY and see if it can run.**
//! That's what this module does — it reuses the production [`spawn_agent`] path
//! to launch the CLI once into a throwaway temp workspace, reads the same
//! `LifecycleEvent` signals the live agents use (ShimReady / ShimExit /
//! HealthFail), classifies the result into three honest states, then kills the
//! process and wipes its scratch.
//!
//! Three states:
//!   - `Usable`       — started, no auth/quota banner, didn't exit early.
//!   - `NeedsLogin`   — alive but a `kind="auth"` health needle fired
//!                      (claude's "Not logged in"); the user must log in.
//!   - `NotInstalled` — binary not on the runtime PATH (no spawn attempted).
//!   - `NotUsable`    — exited early (reasonix with no DEEPSEEK_API_KEY立退s
//!                      code 1), spawn failed, or timed out with no readiness.
//!
//! Two layers of evidence:
//!   - **launch-only** (`observe`) — ShimReady + no auth banner + no early exit.
//!     Token-free; authoritative for claude (ships a "Not logged in" needle) and
//!     reasonix (no key ⇒ `serve` exits non-zero at launch).
//!   - **verified one-turn** — for an engine that passes launch, actually send
//!     one trivial turn and confirm the model responds. This catches a logged-out
//!     codex/opencode: it comes up fine and only 401s when a turn is attempted,
//!     so launch-only would wrongly read it `Usable`. Per channel:
//!       * keystroke engines (claude/codex) → `pty_one_turn_check`: ask a random
//!         arithmetic question, scan PTY output for the answer (not in the prompt,
//!         so the TUI echo can't false-match).
//!       * opencode (TUI HTTP control) → `opencode_one_turn_check`: drive `/tui`
//!         and confirm the model produced output via the session token counts.
//!     Spends a tiny amount of model tokens — the price of certainty. reasonix
//!     (serve) keeps launch-only: no key ⇒ `serve` exits at launch, so it's
//!     already authoritative.
//!
//! Real-usage write-back (`record_live_verdict`) also keeps the cache fresh from
//! live agents for free, so the manual probe is a fallback.

use crate::plugins::CliPlugin;
use crate::pty_stream::FetchResult;
use crate::registry::{AgentSlot, LifecycleEvent};
use crate::spawn::{spawn_agent, WorkspaceLayout};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeState {
    Usable,
    NeedsLogin,
    NotInstalled,
    NotUsable,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeResult {
    pub engine: String,
    pub state: ProbeState,
    /// Human-facing detail (why it can't run), `None` when usable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Coarse failure class for the UI to branch on: "auth" / "fatal" /
    /// "timeout" / "exit". `None` when usable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Unix-ms when this verdict was reached (for SWR staleness).
    pub probed_at: i64,
    /// How the verdict was reached, for diagnostics:
    /// "ready" | "health-needle" | "exit" | "timeout" | "not-installed" |
    /// "spawn-error".
    pub method: String,
}

/// Partial verdict from [`observe`], before the caller stamps engine/time.
struct Verdict {
    state: ProbeState,
    reason: Option<String>,
    kind: Option<String>,
    method: &'static str,
}

/// Persisted cache, mirrored on `~/.flockmux/engine-probe.json` (same atomic
/// temp→rename pattern as `models_config`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProbeCache {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub engines: HashMap<String, ProbeResult>,
}

const CACHE_VERSION: u32 = 1;

/// ShimReady means "the shim wrapper came up"; it does NOT mean the model is
/// reachable. After ready we keep watching this long for a late auth banner or
/// an early exit before declaring `Usable`. claude prints "Not logged in"
/// within ~1-2s of launch, so this window catches it comfortably.
const READY_SETTLE: Duration = Duration::from_secs(8);

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Per-engine overall deadline. opencode's TUI cold-start + first model call can
/// take 60-90s (see `opencode_tui::deliver_bootstrap`), so it gets a longer
/// window; the others come up in a few seconds.
fn probe_timeout(engine: &str) -> Duration {
    match engine {
        "opencode" => Duration::from_secs(100),
        "reasonix" => Duration::from_secs(50),
        _ => Duration::from_secs(35),
    }
}

pub fn probe_cache_path() -> PathBuf {
    let base = std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."));
    base.join(".flockmux").join("engine-probe.json")
}

pub fn load_cache() -> ProbeCache {
    let path = probe_cache_path();
    match std::fs::read_to_string(&path) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
        Err(_) => ProbeCache::default(),
    }
}

/// Atomic persist (temp → rename) so a probe task and a write-back hook never
/// corrupt the file mid-write.
pub fn save_cache(cache: &ProbeCache) -> anyhow::Result<()> {
    let path = probe_cache_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    let body = serde_json::to_string_pretty(cache)?;
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// Upsert one engine's result into the on-disk cache (load → merge → save).
pub fn cache_upsert(result: &ProbeResult) {
    let mut cache = load_cache();
    cache.version = CACHE_VERSION;
    cache.engines.insert(result.engine.clone(), result.clone());
    if let Err(e) = save_cache(&cache) {
        tracing::warn!(?e, engine = %result.engine, "engine-probe: cache save failed");
    }
}

/// Write-back from a LIVE (non-probe) agent's lifecycle. A real user spawning
/// engine X and watching it come up healthy is the same evidence an explicit
/// probe gathers — for free — so we fold it into the same cache. This keeps the
/// verdicts self-maintaining: normal use fills the cache, and the manual
/// "检测可用性" button becomes a rarely-needed fallback. `method` is tagged
/// `live-*` so diagnostics can tell a real-usage verdict from a probed one.
///
/// Fidelity note: this mirrors the launch-only signal (ShimReady ⇒ Usable),
/// the SAME bar the probe uses — so a logged-out codex/opencode (no auth needle)
/// can still write `Usable` here. The verified one-turn check is what tightens
/// that; this is the cheap always-on layer.
pub fn record_live_verdict(
    engine: &str,
    state: ProbeState,
    reason: Option<String>,
    kind: Option<String>,
    method: &'static str,
) {
    cache_upsert(&ProbeResult {
        engine: engine.to_string(),
        state,
        reason,
        kind,
        probed_at: now_ms(),
        method: method.to_string(),
    });
}

/// Set while a `probe_all` sweep is running. The API reads it so the UI can show
/// a spinner, and `probe_all` uses it to refuse a duplicate concurrent sweep
/// (each engine spawns a real cold-starting CLI — running two sweeps at once
/// would thrash the machine).
static PROBE_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

/// True while a real-usability sweep is in flight.
pub fn is_probing() -> bool {
    PROBE_IN_FLIGHT.load(Ordering::SeqCst)
}

/// Atomically claim the in-flight flag; `false` means a sweep already owns it.
fn try_begin() -> bool {
    PROBE_IN_FLIGHT
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
}

/// RAII release so the flag clears even if a probe panics mid-sweep.
struct ProbeGuard;
impl Drop for ProbeGuard {
    fn drop(&mut self) {
        PROBE_IN_FLIGHT.store(false, Ordering::SeqCst);
    }
}

/// All cached verdicts (engine → result), sorted by engine id for a stable UI
/// order. Empty until the first sweep (or, later, real-usage write-back) runs.
pub fn cached_results() -> Vec<ProbeResult> {
    let mut v: Vec<ProbeResult> = load_cache().engines.into_values().collect();
    v.sort_by(|a, b| a.engine.cmp(&b.engine));
    v
}

/// Probe every plugin in the registry, SERIALLY (each spawns a real CLI; four
/// cold starts at once is heavy and opencode is memory-hungry). Results are
/// written to the cache as each completes, so a reader sees them trickle in.
/// No-ops (returns empty) if another sweep is already running.
pub async fn probe_all(
    plugins: &[CliPlugin],
    shim_path: &Path,
    mcp_bin: &Path,
    server_url: &str,
) -> Vec<ProbeResult> {
    if !try_begin() {
        tracing::info!("engine-probe: a sweep is already in flight, skipping duplicate");
        return Vec::new();
    }
    let _guard = ProbeGuard;
    let mut out = Vec::with_capacity(plugins.len());
    for p in plugins {
        // Isolate each engine: a probe spawns a real CLI and parses its raw
        // output, so a panic (a bad byte boundary, an adapter bug) must NOT take
        // down the rest of the sweep. Run it as its own task and turn a JoinError
        // (panic) into an honest NotUsable verdict.
        let plugin = p.clone();
        let shim = shim_path.to_path_buf();
        let mcp = mcp_bin.to_path_buf();
        let url = server_url.to_string();
        let r = match tokio::spawn(
            async move { probe_one(&plugin, &shim, &mcp, &url).await },
        )
        .await
        {
            Ok(r) => r,
            Err(join_err) => {
                tracing::error!(engine = %p.id, ?join_err, "engine-probe: probe task panicked — isolated, sweep continues");
                ProbeResult {
                    engine: p.id.clone(),
                    state: ProbeState::NotUsable,
                    reason: Some("探测过程内部错误（已隔离，不影响其他引擎）".into()),
                    kind: Some("fatal".into()),
                    probed_at: now_ms(),
                    method: "panic".into(),
                }
            }
        };
        tracing::info!(
            engine = %r.engine, state = ?r.state, method = %r.method,
            reason = ?r.reason, "engine-probe: result"
        );
        cache_upsert(&r);
        out.push(r);
    }
    out
}

/// Probe a single engine: install check → real spawn into a temp workspace →
/// read lifecycle signals → classify → kill + wipe scratch. Never panics; every
/// failure path returns a `ProbeResult`.
pub async fn probe_one(
    plugin: &CliPlugin,
    shim_path: &Path,
    mcp_bin: &Path,
    server_url: &str,
) -> ProbeResult {
    let engine = plugin.id.clone();

    // 1. Cheapest signal: is the binary even on the PATH? No spawn if not.
    if crate::runtime_path::resolve_executable(&plugin.binary).is_none() {
        return ProbeResult {
            engine,
            state: ProbeState::NotInstalled,
            reason: Some(format!("`{}` 未安装或不在运行时 PATH 上", plugin.binary)),
            kind: None,
            probed_at: now_ms(),
            method: "not-installed".into(),
        };
    }

    // 2. Throwaway workspace under the OS temp dir — workspace-local artifacts
    //    (.mcp.json, .codex/, .claude/) die with it on cleanup, and it never
    //    enters the SQLite `workspaces` table so it can't show up in the UI.
    let tmp = std::env::temp_dir().join(format!(
        "flockmux-probe-{}-{}",
        plugin.id,
        &Uuid::new_v4().to_string()[..8]
    ));
    let layout = WorkspaceLayout::Shared { dir: tmp.clone() };

    // 3. Real spawn over the production path. recorder=None → no .cast file.
    let spawn = match spawn_agent(
        plugin, None, None, None, &layout, shim_path, mcp_bin, server_url, None,
    ) {
        Ok(s) => s,
        Err(e) => {
            cleanup(&tmp, None);
            return ProbeResult {
                engine,
                state: ProbeState::NotUsable,
                reason: Some(format!("启动失败：{e}")),
                kind: Some("fatal".into()),
                probed_at: now_ms(),
                method: "spawn-error".into(),
            };
        }
    };
    let agent_id = spawn.agent_id.clone();
    let slot = spawn.slot;
    let mut rx = slot.lifecycle_tx.subscribe();

    // 4. Watch the same lifecycle channel live agents use.
    let verdict = observe(&mut rx, probe_timeout(&engine)).await;

    // 4b. Verified one-turn check. Launch-only can't tell a logged-out
    //     codex/opencode (comes up fine, 401s only when a turn is attempted)
    //     from a working one, so for a launch-Usable engine we actually complete
    //     one trivial turn — over the PTY for keystroke engines (claude/codex),
    //     over the /tui HTTP control API for opencode. reasonix (serve) keeps its
    //     launch verdict: no key → its `serve` exits at launch, so launch-only is
    //     already authoritative and a turn adds nothing.
    let verdict = if verdict.state == ProbeState::Usable {
        if let Some(port) = slot.tui_http_port() {
            opencode_one_turn_check(port, &slot.workspace).await
        } else if slot.serve_http_port().is_some() {
            verdict
        } else {
            pty_one_turn_check(&slot).await
        }
    } else {
        verdict
    };

    // 5. Tear down: kill the PTY process group, drop the slot (pump drains),
    //    wipe the temp workspace + any per-agent scratch the adapters wrote.
    slot.kill();
    drop(slot);
    cleanup(&tmp, Some(&agent_id));

    ProbeResult {
        engine,
        state: verdict.state,
        reason: verdict.reason,
        kind: verdict.kind,
        probed_at: now_ms(),
        method: verdict.method.into(),
    }
}

/// Read lifecycle events until a decisive signal or the deadline.
async fn observe(
    rx: &mut tokio::sync::broadcast::Receiver<LifecycleEvent>,
    total: Duration,
) -> Verdict {
    let start = Instant::now();
    let mut ready_at: Option<Instant> = None;

    loop {
        // Ready + survived the settle window with no failure → Usable.
        if let Some(r) = ready_at {
            if r.elapsed() >= READY_SETTLE {
                return Verdict {
                    state: ProbeState::Usable,
                    reason: None,
                    kind: None,
                    method: "ready",
                };
            }
        }

        let elapsed = start.elapsed();
        if elapsed >= total {
            return ready_or_timeout(ready_at.is_some());
        }
        // Wake up at whichever comes first: overall deadline, or (once ready)
        // the end of the settle window.
        let until_total = total - elapsed;
        let recv_to = match ready_at {
            Some(r) => READY_SETTLE.saturating_sub(r.elapsed()).min(until_total),
            None => until_total,
        };

        match tokio::time::timeout(recv_to, rx.recv()).await {
            Ok(Ok(LifecycleEvent::HealthFail { reason, kind })) => {
                // Decisive: the CLI itself says it can't work. "auth" → login.
                let state = if kind == "auth" {
                    ProbeState::NeedsLogin
                } else {
                    ProbeState::NotUsable
                };
                return Verdict {
                    state,
                    reason: Some(reason),
                    kind: Some(kind),
                    method: "health-needle",
                };
            }
            Ok(Ok(LifecycleEvent::ShimExit(code))) => {
                if code == 0 && ready_at.is_some() {
                    // Clean exit after coming up — treat as ran-ok.
                    return Verdict {
                        state: ProbeState::Usable,
                        reason: None,
                        kind: None,
                        method: "exit-ok",
                    };
                }
                // Early/non-zero exit = 立退. reasonix with no DEEPSEEK_API_KEY
                // lands here (serve refuses to run → exit 1).
                return Verdict {
                    state: ProbeState::NotUsable,
                    reason: Some(format!(
                        "进程退出 code={code}（可能未登录、缺 key 或配置不全）"
                    )),
                    kind: Some("exit".into()),
                    method: "exit",
                };
            }
            Ok(Ok(LifecycleEvent::ShimReady)) => {
                ready_at.get_or_insert_with(Instant::now);
                // Keep watching through the settle window for a late banner.
            }
            // Broadcast lag — just keep reading.
            Ok(Err(_)) => continue,
            // recv timed out: either the settle window closed (→ usable) or the
            // overall deadline hit (→ timeout). The loop top re-checks settle;
            // fall through to re-evaluate.
            Err(_) => continue,
        }
    }
}

fn ready_or_timeout(ready: bool) -> Verdict {
    if ready {
        Verdict {
            state: ProbeState::Usable,
            reason: None,
            kind: None,
            method: "ready",
        }
    } else {
        Verdict {
            state: ProbeState::NotUsable,
            reason: Some("启动后在超时窗口内无响应".into()),
            kind: Some("timeout".into()),
            method: "timeout",
        }
    }
}

/// How long to wait for the engine to actually answer the one-turn prompt. A
/// trivial arithmetic turn lands in a few seconds on a warm CLI; a cold model
/// call or a retrying auth failure can take longer, hence a generous window.
const TURN_TIMEOUT: Duration = Duration::from_secs(35);

/// Verified one-turn check for keystroke (PTY) engines — the honest tightening
/// of launch-only classification.
///
/// `ShimReady` only proves the CLI's wrapper came up; codex/opencode ship no
/// auth needle, so a logged-out one launches fine and only fails when a turn is
/// actually attempted (it 401s on the model call). So we send ONE trivial
/// arithmetic prompt and watch the PTY output for the *answer*. The answer is
/// the sum of two pseudo-random addends and is NOT present in the prompt, so the
/// TUI's echo of our pasted prompt can't satisfy the scan — only a real model
/// turn produces it. Auth/quota banners in the turn output → NeedsLogin; no
/// answer before the deadline → NotUsable. This DOES spend a (tiny) amount of
/// model tokens, unlike the launch-only path — it's the price of certainty.
async fn pty_one_turn_check(slot: &AgentSlot) -> Verdict {
    let (Some(input), Some(stream)) = (slot.pty_input(), slot.pty_stream()) else {
        // Not a keystroke channel (caller gates on this) — keep launch verdict.
        return Verdict {
            state: ProbeState::Usable,
            reason: None,
            kind: None,
            method: "ready",
        };
    };

    // Two pseudo-random 4-digit addends from a uuid; their SUM is the token to
    // scan for, and it never appears in the prompt text.
    let id = Uuid::new_v4();
    let b = id.as_bytes();
    let a = 1000 + (u16::from_le_bytes([b[0], b[1]]) % 9000) as u32;
    let c = 1000 + (u16::from_le_bytes([b[2], b[3]]) % 9000) as u32;
    let sum = (a + c).to_string();
    let prompt = format!(
        "What is {a} plus {c}? Reply with ONLY the result as plain digits — \
         no commas, no spaces, no words, nothing else."
    );

    // Cursor at the head: read only output produced AFTER we ask.
    let mut cursor = stream.snapshot().next_seq.saturating_sub(1);

    // Deliver like the bootstrap path: paste body, settle (scaled to size), a
    // standalone \r to submit, then a safety \r once the paste has closed.
    let body = crate::spells::sanitize_pty_inject(&prompt).into_bytes();
    let body_len = body.len() as u64;
    if input.send(bytes::Bytes::from(body)).await.is_err() {
        return Verdict {
            state: ProbeState::NotUsable,
            reason: Some("无法向 PTY 投递验证提示".into()),
            kind: Some("fatal".into()),
            method: "turn-deliver-fail",
        };
    }
    tokio::time::sleep(Duration::from_millis(150 + body_len / 100)).await;
    let _ = input.send(bytes::Bytes::from_static(b"\r")).await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    let _ = input.send(bytes::Bytes::from_static(b"\r")).await;

    // Scan the PTY output for the answer (→ verified) or an auth banner. Keep
    // RAW BYTES and lossy-decode for matching: a PTY chunk can split a UTF-8
    // char or ANSI sequence at any boundary, so byte-truncating the buffer is
    // panic-free (unlike `String::split_off`, which panics on a non-char
    // boundary — a real crash codex's multibyte TUI output triggers constantly).
    const SCAN_CAP: usize = 64 * 1024;
    let start = Instant::now();
    let mut buf: Vec<u8> = Vec::new();
    while start.elapsed() < TURN_TIMEOUT {
        if let FetchResult::Ok(entries) = stream.fetch_since(cursor) {
            for (seq, bytes) in entries {
                cursor = cursor.max(seq);
                buf.extend_from_slice(&bytes);
            }
        }
        // Bound memory: the answer lands near the tail; keep the last 64 KiB.
        // `drain` at an arbitrary byte index never panics.
        if buf.len() > SCAN_CAP {
            buf.drain(..buf.len() - SCAN_CAP);
        }
        if let Some(v) = classify_turn(&String::from_utf8_lossy(&buf), &sum) {
            return v;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    // Deadline with no answer: NeedsLogin if a banner showed, else launched-but-
    // can't-complete-a-turn.
    classify_turn(&String::from_utf8_lossy(&buf), &sum).unwrap_or(Verdict {
        state: ProbeState::NotUsable,
        reason: Some("启动正常但在超时内没完成一次对话（可能未登录 / 额度耗尽 / 配置不全）".into()),
        kind: Some("turn-timeout".into()),
        method: "turn-timeout",
    })
}

/// Classify accumulated one-turn output: an auth/quota banner → NeedsLogin; the
/// expected answer present → verified Usable; otherwise keep waiting (`None`).
/// Auth is checked first because a logged-out engine prints the banner *instead*
/// of an answer.
fn classify_turn(buf: &str, sum: &str) -> Option<Verdict> {
    let low = buf.to_lowercase();
    const AUTH: &[&str] = &[
        "not logged in",
        "not authenticated",
        "unauthorized",
        "missing bearer",
        "please log in",
        "please login",
        "/login",
        "log in to",
    ];
    if AUTH.iter().any(|n| low.contains(n)) {
        return Some(Verdict {
            state: ProbeState::NeedsLogin,
            reason: Some("启动正常，但发起一次对话时返回未登录 / 未授权".into()),
            kind: Some("auth".into()),
            method: "turn-auth",
        });
    }
    if buf.contains(sum) {
        return Some(Verdict {
            state: ProbeState::Usable,
            reason: None,
            kind: None,
            method: "turn-ok",
        });
    }
    None
}

/// Overall budget for opencode's verified turn. Its TUI cold-start + first model
/// call is slow (the bootstrap path alone allows ~90s), so it gets a generous
/// window before we call it can't-complete-a-turn.
const OPENCODE_TURN_TIMEOUT: Duration = Duration::from_secs(75);

/// Verified one-turn check for opencode. opencode is driven over its `/tui` HTTP
/// control API, not keystrokes, so the PTY answer-scan doesn't apply — instead we
/// submit a trivial turn and confirm the model produced output via the session
/// token counts (see `opencode_tui::verify_one_turn`). That's actually cleaner
/// than the PTY path: reading structured token counts needs no echo
/// disambiguation. Detecting an auth banner (→ NeedsLogin) would need reading the
/// session's message text; for now a logged-out opencode lands on NotUsable.
async fn opencode_one_turn_check(port: u16, workspace_dir: &str) -> Verdict {
    let prompt = "What is 318 plus 921? Reply with only the number.";
    match crate::opencode_tui::verify_one_turn(port, workspace_dir, prompt, OPENCODE_TURN_TIMEOUT)
        .await
    {
        Ok(true) => Verdict {
            state: ProbeState::Usable,
            reason: None,
            kind: None,
            method: "turn-ok",
        },
        Ok(false) => Verdict {
            state: ProbeState::NotUsable,
            reason: Some("启动正常但在超时内没完成一次对话（可能未登录 / 额度耗尽 / 配置不全）".into()),
            kind: Some("turn-timeout".into()),
            method: "turn-timeout",
        },
        Err(e) => Verdict {
            state: ProbeState::NotUsable,
            reason: Some(format!("opencode 验证回合失败：{e}")),
            kind: Some("fatal".into()),
            method: "turn-error",
        },
    }
}

/// Remove the probe's scratch: temp workspace + per-agent config dirs the
/// adapters write under `~/.flockmux/` (reasonix HOME, opencode/codex config,
/// the swarm MCP entry, the wake throttle file). Best-effort; missing is fine.
fn cleanup(tmp: &Path, agent_id: Option<&str>) {
    let _ = std::fs::remove_dir_all(tmp);
    let (Some(id), Ok(home)) = (agent_id, std::env::var("HOME")) else {
        return;
    };
    let fm = PathBuf::from(home).join(".flockmux");
    let _ = std::fs::remove_dir_all(fm.join("reasonix-home").join(id));
    let _ = std::fs::remove_dir_all(fm.join("codex-home").join(id));
    let _ = std::fs::remove_file(fm.join("opencode").join(format!("{id}.json")));
    let _ = std::fs::remove_file(fm.join("mcp").join(format!("{id}.json")));
    let _ = std::fs::remove_file(fm.join("wake").join(format!("{id}.json")));
}
