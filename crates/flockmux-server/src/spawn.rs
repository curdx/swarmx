//! Wire `flockmux-shim <real-cli> <args...>` together for a single agent.

use crate::plugins::CliPlugin;
use crate::registry::AgentSlot;
use anyhow::{Context, Result};
use flockmux_pty::{PtyBridge, PtyHandles, SpawnOpts};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use uuid::Uuid;

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

    // Skip claude's "Do you trust this folder?" prompt for workspaces we
    // created ourselves under ~/.flockmux/.
    if plugin.auto_trust_workspace && plugin.id == "claude" {
        if let Err(err) = crate::trust::mark_claude_workspace_trusted(&workspace) {
            tracing::warn!(?err, "auto-trust patch failed; user will see the prompt");
        }
    }

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

    let slot = AgentSlot {
        bridge,
        output_rx: Some(output_rx),
        input_tx,
        cli: plugin.id.clone(),
        role: role.unwrap_or_else(|| plugin.id.clone()),
        workspace: workspace.to_string_lossy().into_owned(),
    };

    Ok(AgentSpawn { agent_id, slot })
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
