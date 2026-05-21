//! Wire `flockmux-shim <real-cli> <args...>` together for a single agent.

use crate::plugins::CliPlugin;
use crate::pty_stream::PtyStream;
use crate::registry::{AgentSlot, Lifecycle, LifecycleEvent};
use anyhow::{Context, Result};
use flockmux_pty::{PtyBridge, PtyHandles, SpawnOpts};
use flockmux_recorder::RecorderHandle;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use uuid::Uuid;

// OSC markers the shim emits — kept identical to flockmux-shim/src/main.rs.
const OSC_READY: &[u8] = b"\x1b]633;A\x07";
const OSC_EXIT_PREFIX: &[u8] = b"\x1b]633;D;";
const OSC_TERMINATOR: u8 = 0x07;

pub struct AgentSpawn {
    pub agent_id: String,
    pub slot: AgentSlot,
}

/// `shim_path` is the absolute path to `flockmux-shim`. Caller normally
/// derives it from `std::env::current_exe()` parent + "flockmux-shim".
///
/// `mcp_bin` is the absolute path to `flockmux-mcp` (the swarm MCP stdio
/// server). It's baked into per-spawn MCP config entries written under
/// pre-spawn patches so claude / codex can launch it on first tool call.
///
/// `server_url` is the base URL of the flockmux-server REST API that the
/// MCP subprocess will speak to. Loopback today.
///
/// `recorder` is an optional asciicast v2 sink. When set, the PTY pump
/// mirrors every chunk (including OSC lifecycle markers) into the
/// recorder; when unset, the recording layer is bypassed.
pub fn spawn_agent(
    plugin: &CliPlugin,
    role: Option<String>,
    workspace_root: &Path,
    shim_path: &Path,
    mcp_bin: &Path,
    server_url: &str,
    recorder: Option<RecorderHandle>,
) -> Result<AgentSpawn> {
    let agent_id = format!("{}-{}", plugin.id, &Uuid::new_v4().to_string()[..8]);
    let workspace = ensure_workspace(workspace_root, &agent_id)?;

    // Suppress per-CLI interactive prompts that would block a headless PTY
    // (claude's "trust folder", codex's "update available"). Each patch is a
    // no-op when not configured / not applicable. The MCP-inject patch is
    // also routed here since it shares the "pre-spawn home dir mutation"
    // shape.
    let pre_ctx = crate::pre_spawn::PreSpawnCtx {
        agent_id: agent_id.clone(),
        mcp_bin: mcp_bin.to_path_buf(),
        server_url: server_url.to_string(),
    };
    crate::pre_spawn::run_patches(plugin, &workspace, &pre_ctx);

    let mut argv = Vec::with_capacity(2 + plugin.default_args.len() + 1);
    argv.push(shim_path.to_string_lossy().into_owned());
    argv.push(plugin.binary.clone());
    argv.extend(plugin.default_args.iter().cloned());

    // codex 0.130 gates non-managed Stop hooks behind an in-app /hooks
    // trust-review prompt — workspace-local hooks.json gets installed but
    // never executes until the user manually approves it. PR #21768 ships
    // `--dangerously-bypass-hook-trust` to skip the review for automation
    // hosts like us. The flag isn't in 0.130 yet (codex aborts spawn on
    // unknown argv), so probe `<binary> --help` once per process and only
    // inject the flag if it's already supported. Net effect:
    //   - codex 0.130: probe -> false, argv unchanged, hooks.json stays
    //     dormant (known constraint, documented in auto-memory).
    //   - codex >=0.131 (future): probe -> true, flag injected, our
    //     existing hooks.json install becomes immediately effective with
    //     zero config change on flockmux's side.
    if plugin.id == "codex"
        && binary_supports_flag(&plugin.binary, "--dangerously-bypass-hook-trust")
    {
        argv.push("--dangerously-bypass-hook-trust".into());
        tracing::info!(
            agent = %agent_id,
            "codex --dangerously-bypass-hook-trust supported; injecting"
        );
    }

    // Env: pass through HOME so the CLI finds its OAuth credentials
    // (~/.claude or ~/.codex). Drop everything else from the parent
    // process — the CLI shouldn't inherit ad-hoc shell vars.
    let mut env = HashMap::new();
    let home_var = if plugin.home_env.is_empty() {
        "HOME"
    } else {
        &plugin.home_env
    };
    if let Ok(home) = std::env::var(home_var) {
        env.insert("HOME".into(), home);
    }
    // Useful unicode default — many CLIs probe LANG.
    if let Ok(lang) = std::env::var("LANG") {
        env.insert("LANG".into(), lang);
    } else {
        env.insert("LANG".into(), "en_US.UTF-8".into());
    }
    // PATH: keep the parent's so the inner CLI can resolve its own subcommands
    // (e.g. `claude doctor` may exec `node`).
    if let Ok(path) = std::env::var("PATH") {
        env.insert("PATH".into(), path);
    }
    // Identity env passed to the CLI. codex picks `FLOCKMUX_AGENT_ID` /
    // `FLOCKMUX_SERVER_URL` up via the `env_vars` whitelist in
    // ~/.codex/config.toml and forwards them to the MCP subprocess. claude
    // also forwards them by spec (any vars present in the spawn env that
    // match the MCP entry's `env` block) — and the local-scope MCP entry
    // we write already lists them explicitly, so this is belt + braces.
    env.insert("FLOCKMUX_AGENT_ID".into(), agent_id.clone());
    env.insert("FLOCKMUX_SERVER_URL".into(), server_url.to_string());

    let argv_strings: Vec<String> = argv;

    let PtyHandles { bridge, output_rx } = PtyBridge::spawn(SpawnOpts {
        argv: &argv_strings,
        cwd: Some(&workspace),
        env,
        cols: 120,
        rows: 32,
    })
    .with_context(|| format!("PtyBridge::spawn for {}", plugin.id))?;

    let input_tx = bridge.input_sender();
    let bridge = Arc::new(bridge);

    // Drain the PTY's output mpsc into the shared resume buffer. The pump
    // owns the receiver for the agent's whole lifetime — WS subscribers
    // read from the buffer, never the mpsc. EOF closes the buffer so any
    // WS writer task that's parked on `wait_changed` wakes up and exits.
    //
    // The pump is also where OSC lifecycle markers are scanned — exactly
    // once per agent, so multi-attach subscribers don't each redundantly
    // re-detect ShimReady/ShimExit, and resume attaches (whose cursor may
    // skip past the original OSC) can be told the current lifecycle via
    // Hello + the broadcast.
    let stream = Arc::new(PtyStream::new());
    let lifecycle = Arc::new(Mutex::new(Lifecycle::default()));
    // capacity=16 holds ShimReady + ShimExit plus headroom even if both
    // events fire faster than the slowest WS subscriber drains them.
    let (lifecycle_tx, _lifecycle_rx) = tokio::sync::broadcast::channel(16);
    {
        let stream = stream.clone();
        let lifecycle = lifecycle.clone();
        let lifecycle_tx = lifecycle_tx.clone();
        let agent_id_for_log = agent_id.clone();
        let recorder = recorder.clone();
        tokio::spawn(async move {
            let mut output_rx = output_rx;
            let mut osc_buf: Vec<u8> = Vec::new();
            while let Some(chunk) = output_rx.recv().await {
                scan_osc(&mut osc_buf, &chunk, &lifecycle, &lifecycle_tx);
                if let Some(rec) = &recorder {
                    rec.write_chunk(chunk.clone());
                }
                stream.append(chunk);
            }
            tracing::debug!(agent = %agent_id_for_log, "pty output drained, closing stream");
            stream.close();
            // Dropping `recorder` here is what signals EOF to the writer
            // task — every clone of the handle (including this pump's) is
            // gone, the mpsc closes, and `wait_finalize` resolves.
            drop(recorder);
        });
    }

    let slot = AgentSlot {
        bridge,
        stream,
        lifecycle,
        lifecycle_tx,
        input_tx,
        cli: plugin.id.clone(),
        role: role.unwrap_or_else(|| plugin.id.clone()),
        workspace: workspace.to_string_lossy().into_owned(),
    };

    Ok(AgentSpawn { agent_id, slot })
}

/// Server-side OSC scanner. Identical algorithm to the M2 client-side
/// scanner in `pty_ws.rs`, lifted here so it runs exactly once per agent
/// regardless of how many WS subscribers are attached.
///
/// Updates `lifecycle` in place and broadcasts state-change events.
/// `osc_buf` carries state across calls so OSC sequences split across
/// `read(2)` boundaries are still matched.
fn scan_osc(
    osc_buf: &mut Vec<u8>,
    chunk: &[u8],
    lifecycle: &Mutex<Lifecycle>,
    tx: &tokio::sync::broadcast::Sender<LifecycleEvent>,
) {
    osc_buf.extend_from_slice(chunk);

    loop {
        // Bound growth; OSC sequences are short, anything beyond a few KB
        // means we're holding non-OSC junk indefinitely.
        if osc_buf.len() > 4096 {
            let keep_from = osc_buf.len() - 256;
            osc_buf.drain(..keep_from);
        }

        if let Some(pos) = find(osc_buf, OSC_READY) {
            osc_buf.drain(..pos + OSC_READY.len());
            let already = {
                let mut l = lifecycle.lock();
                let prev = l.shim_ready;
                l.shim_ready = true;
                prev
            };
            if !already {
                let _ = tx.send(LifecycleEvent::ShimReady);
            }
            continue;
        }
        if let Some(pos) = find(osc_buf, OSC_EXIT_PREFIX) {
            let after = pos + OSC_EXIT_PREFIX.len();
            if let Some(end_rel) = osc_buf[after..].iter().position(|&b| b == OSC_TERMINATOR) {
                let code_bytes = &osc_buf[after..after + end_rel];
                let code = std::str::from_utf8(code_bytes)
                    .ok()
                    .and_then(|s| s.parse::<i32>().ok())
                    .unwrap_or(-1);
                osc_buf.drain(..after + end_rel + 1);
                let new_exit = {
                    let mut l = lifecycle.lock();
                    if l.shim_exit.is_some() {
                        false
                    } else {
                        l.shim_exit = Some(code);
                        true
                    }
                };
                if new_exit {
                    let _ = tx.send(LifecycleEvent::ShimExit(code));
                }
                continue;
            } else {
                // OSC exit prefix matched but terminator not in this chunk;
                // wait for more bytes.
                break;
            }
        }
        break;
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn ensure_workspace(root: &Path, agent_id: &str) -> Result<PathBuf> {
    let dir = root.join(agent_id);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("create workspace {}", dir.display()))?;
    // Resolve symlinks so the path matches what the spawned CLI sees after
    // its own canonicalize step. On macOS `/tmp` is a symlink to
    // `/private/tmp`, and claude / codex both trust by canonical path —
    // without this, our trust-patch key wouldn't match and the prompt
    // resurfaces.
    let canonical = std::fs::canonicalize(&dir)
        .with_context(|| format!("canonicalize workspace {}", dir.display()))?;
    Ok(canonical)
}

/// Find `flockmux-shim` next to the current executable. Falls back to
/// `target/debug/flockmux-shim` relative to the manifest dir during
/// `cargo run`, since `current_exe` points into `target/debug/deps/...`
/// for tests but `target/debug/` for `cargo run`.
pub fn locate_shim() -> Result<PathBuf> {
    locate_sibling_bin("flockmux-shim", "FLOCKMUX_SHIM_PATH")
}

/// Find `flockmux-mcp` next to the current executable (same heuristic as
/// `locate_shim`). The path is baked into MCP config entries so claude /
/// codex can launch it directly.
pub fn locate_mcp() -> Result<PathBuf> {
    locate_sibling_bin("flockmux-mcp", "FLOCKMUX_MCP_PATH")
}

/// Probe `<binary> --help` once and cache whether `flag` appears anywhere
/// in stdout or stderr. Used to feature-detect CLI flags whose absence
/// would crash spawn (codex 0.130 rejects unknown argv with non-zero exit
/// — adding a future-only flag unconditionally would brick every spawn on
/// the older version).
///
/// Cache key is `(binary, flag)` so different plugins probing different
/// flags don't collide. The cache is process-lifetime — a CLI upgrade
/// requires a server restart to re-probe, which is fine for the local
/// single-user model.
///
/// Errors and timeouts on the probe fall through as `false`: if we can't
/// confirm the flag is supported, we don't inject it.
fn binary_supports_flag(binary: &str, flag: &str) -> bool {
    static CACHE: OnceLock<Mutex<HashMap<(String, String), bool>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));

    let key = (binary.to_string(), flag.to_string());
    if let Some(&v) = cache.lock().get(&key) {
        return v;
    }

    let supported = std::process::Command::new(binary)
        .arg("--help")
        .output()
        .ok()
        .map(|o| {
            let stdout = String::from_utf8_lossy(&o.stdout);
            let stderr = String::from_utf8_lossy(&o.stderr);
            stdout.contains(flag) || stderr.contains(flag)
        })
        .unwrap_or(false);

    tracing::info!(
        binary, flag, supported,
        "binary flag probe result"
    );
    cache.lock().insert(key, supported);
    supported
}

fn locate_sibling_bin(name: &str, env_override: &str) -> Result<PathBuf> {
    if let Ok(p) = std::env::var(env_override) {
        return Ok(PathBuf::from(p));
    }
    let exe = std::env::current_exe().context("current_exe")?;
    if let Some(dir) = exe.parent() {
        let cand = dir.join(if cfg!(windows) {
            format!("{name}.exe")
        } else {
            name.to_string()
        });
        if cand.is_file() {
            return Ok(cand);
        }
    }
    anyhow::bail!(
        "{name} not found next to flockmux-server. Build it with \
         `cargo build -p {name}` or set {env_override}"
    )
}
