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
//! Stage-1 note: classification leans on lifecycle signals only — accurate for
//! claude (it ships a "Not logged in" needle) and reasonix (no key = serve
//! exits non-zero). codex/opencode ship no auth needle yet, so a logged-out
//! codex/opencode can still read as `Usable` here; tightening that (a real
//! one-line turn check + auth needles) is a later stage. The cache schema and
//! API already carry everything those refinements need.

use crate::plugins::CliPlugin;
use crate::registry::LifecycleEvent;
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
        let r = probe_one(p, shim_path, mcp_bin, server_url).await;
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
