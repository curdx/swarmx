//! Wire `flockmux-shim <real-cli> <args...>` together for a single agent.

use crate::plugins::CliPlugin;
use crate::pty_stream::PtyStream;
use crate::registry::{AgentSlot, Lifecycle, LifecycleEvent};
use anyhow::{Context, Result};
use flockmux_pty::{PtyBridge, PtyHandles, SpawnOpts};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
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
pub fn spawn_agent(
    plugin: &CliPlugin,
    role: Option<String>,
    workspace_root: &Path,
    shim_path: &Path,
) -> Result<AgentSpawn> {
    let agent_id = format!("{}-{}", plugin.id, &Uuid::new_v4().to_string()[..8]);
    let workspace = ensure_workspace(workspace_root, &agent_id)?;

    // Suppress per-CLI interactive prompts that would block a headless PTY
    // (claude's "trust folder", codex's "update available"). Each patch is a
    // no-op when not configured / not applicable.
    crate::pre_spawn::run_patches(plugin, &workspace);

    let mut argv = Vec::with_capacity(2 + plugin.default_args.len());
    argv.push(shim_path.to_string_lossy().into_owned());
    argv.push(plugin.binary.clone());
    argv.extend(plugin.default_args.iter().cloned());

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
    // Telemetry / lifecycle env for future MCP injection.
    env.insert("FLOCKMUX_AGENT_ID".into(), agent_id.clone());

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
        tokio::spawn(async move {
            let mut output_rx = output_rx;
            let mut osc_buf: Vec<u8> = Vec::new();
            while let Some(chunk) = output_rx.recv().await {
                scan_osc(&mut osc_buf, &chunk, &lifecycle, &lifecycle_tx);
                stream.append(chunk);
            }
            tracing::debug!(agent = %agent_id_for_log, "pty output drained, closing stream");
            stream.close();
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
    Ok(dir)
}

/// Find `flockmux-shim` next to the current executable. Falls back to
/// `target/debug/flockmux-shim` relative to the manifest dir during
/// `cargo run`, since `current_exe` points into `target/debug/deps/...`
/// for tests but `target/debug/` for `cargo run`.
pub fn locate_shim() -> Result<PathBuf> {
    if let Ok(p) = std::env::var("FLOCKMUX_SHIM_PATH") {
        return Ok(PathBuf::from(p));
    }
    let exe = std::env::current_exe().context("current_exe")?;
    if let Some(dir) = exe.parent() {
        let cand = dir.join(if cfg!(windows) {
            "flockmux-shim.exe"
        } else {
            "flockmux-shim"
        });
        if cand.is_file() {
            return Ok(cand);
        }
    }
    anyhow::bail!(
        "flockmux-shim not found next to flockmux-server. Build it with \
         `cargo build -p flockmux-shim` or set FLOCKMUX_SHIM_PATH"
    )
}
