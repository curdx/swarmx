//! CLI plugin registry. Each plugin is a `cli-plugins/<id>.toml` describing
//! how to spawn one kind of CLI under our shim. M1 ships claude + codex
//! only; others live in §13 Backlog of the plan.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliPlugin {
    pub id: String,
    pub display_name: String,
    pub binary: String,
    #[serde(default)]
    pub default_args: Vec<String>,
    /// One of: `shim_osc`, `prompt_pattern`, `none`. M1 only honours
    /// `shim_osc` (the others are stubs for M2+).
    #[serde(default = "default_ready_detect")]
    pub ready_detect: String,
    /// One of: `project_mcp_json`, `codex_toml`, `none`. Unused in M1.
    #[serde(default = "default_mcp_inject")]
    pub mcp_inject: String,
    /// Env var name to pass through from the server process (so the CLI
    /// can find its OAuth credentials at `$HOME/.claude/` etc.). Default
    /// is "HOME".
    #[serde(default = "default_home_env")]
    pub home_env: String,
    /// If true, the host patches the CLI's per-workspace trust state before
    /// spawn so the CLI doesn't prompt "Do you trust this folder?" — fine
    /// for flockmux because workspaces always live under `~/.flockmux/`
    /// (created by us). Currently only honoured for `id = "claude"` (writes
    /// `~/.claude.json: projects[<ws>].hasTrustDialogAccepted = true`).
    #[serde(default)]
    pub auto_trust_workspace: bool,
    /// If true, the host suppresses the CLI's "an update is available"
    /// prompt before spawn — those prompts otherwise block the headless PTY
    /// waiting on a single keystroke we have no way to deliver. Currently
    /// only honoured for `id = "codex"` (writes
    /// `~/.codex/version.json: dismissed_version = latest_version`).
    #[serde(default)]
    pub auto_dismiss_update: bool,
    /// If true, the host writes (or refreshes) an MCP server entry pointing
    /// at the `flockmux-mcp` binary so the spawned agent can call swarm
    /// tools (send_message / blackboard / …) from inside its native toolbox.
    /// Currently honoured for:
    ///   - `id = "claude"` — writes `~/.claude.json projects.<ws>.mcpServers.flockmux-swarm`
    ///     (local scope, no approval prompt; per-spawn entry carries agent_id)
    ///   - `id = "codex"`  — appends `[mcp_servers.flockmux-swarm]` to
    ///     `~/.codex/config.toml` (global config; per-spawn identity rides
    ///     in via the `FLOCKMUX_AGENT_ID` env passthrough)
    #[serde(default)]
    pub auto_inject_mcp: bool,
    /// If true, the host installs a workspace-local Stop hook that runs
    /// `flockmux-mcp wake-check` at every turn boundary, giving the agent
    /// a synthetic continuation prompt whenever its swarm inbox has unread
    /// messages. Currently honoured for:
    ///   - `id = "claude"` — writes `<workspace>/.claude/settings.local.json`
    ///     `hooks.Stop[]` (timeout in milliseconds).
    ///   - `id = "codex"`  — writes `<workspace>/.codex/hooks.json`
    ///     `hooks.Stop[]` (timeout in seconds).
    ///
    /// Merge-not-clobber: existing user hooks are preserved; flockmux's
    /// entry is appended once (idempotent on re-spawn).
    #[serde(default)]
    pub auto_inject_stop_hook: bool,
}

fn default_ready_detect() -> String { "shim_osc".into() }
fn default_mcp_inject() -> String { "none".into() }
fn default_home_env() -> String { "HOME".into() }

#[derive(Debug, Clone, Default)]
pub struct PluginRegistry {
    plugins: HashMap<String, CliPlugin>,
}

impl PluginRegistry {
    pub fn load_dir(dir: &Path) -> Result<Self> {
        let mut plugins = HashMap::new();
        let read = std::fs::read_dir(dir)
            .with_context(|| format!("read_dir({})", dir.display()))?;
        for entry in read {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("toml") {
                continue;
            }
            let bytes = std::fs::read_to_string(&path)
                .with_context(|| format!("read {}", path.display()))?;
            let plugin: CliPlugin = toml::from_str(&bytes)
                .with_context(|| format!("parse {}", path.display()))?;
            plugins.insert(plugin.id.clone(), plugin);
        }
        Ok(Self { plugins })
    }

    pub fn get(&self, id: &str) -> Option<&CliPlugin> {
        self.plugins.get(id)
    }

    pub fn list(&self) -> Vec<&CliPlugin> {
        let mut v: Vec<_> = self.plugins.values().collect();
        v.sort_by_key(|p| p.id.clone());
        v
    }
}

/// Locate the `cli-plugins/` directory: first the path from env
/// `FLOCKMUX_CLI_PLUGINS_DIR`, otherwise `<workspace>/cli-plugins` relative
/// to the binary's manifest dir (during dev) or CWD.
pub fn default_plugins_dir() -> PathBuf {
    if let Ok(p) = std::env::var("FLOCKMUX_CLI_PLUGINS_DIR") {
        return PathBuf::from(p);
    }
    // CARGO_MANIFEST_DIR resolves to `crates/flockmux-server` at build
    // time; step up two levels to reach the workspace root.
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    if let Some(ws) = manifest.parent().and_then(|p| p.parent()) {
        let candidate = ws.join("cli-plugins");
        if candidate.is_dir() {
            return candidate;
        }
    }
    PathBuf::from("cli-plugins")
}
