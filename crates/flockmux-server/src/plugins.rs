//! CLI plugin registry. Each plugin is a `cli-plugins/<id>.toml` describing
//! how to spawn one kind of CLI under our shim. M1 ships claude + codex
//! only; others live in §13 Backlog of the plan.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// How this CLI's per-workspace *trust* state is recorded so the headless PTY
/// isn't blocked on a "do you trust this folder?" prompt. Dispatch is keyed on
/// this **format**, not on `plugin.id` — a new CLI that reuses an existing
/// config format needs only the right value here, zero Rust changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum TrustFormat {
    /// No trust patch (the CLI has no trust gate, or we don't manage it).
    #[default]
    None,
    /// `~/.claude.json projects.<ws>.hasTrustDialogAccepted = true`.
    ClaudeJson,
    /// Appended `[projects."<ws>"] trust_level = "trusted"` in `~/.codex/config.toml`.
    CodexToml,
}

/// How this CLI is told to load the `flockmux-swarm` MCP server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum McpFormat {
    #[default]
    None,
    /// claude: per-project local scope in `~/.claude.json` + a per-agent
    /// `--mcp-config <file> --strict-mcp-config` (injected in spawn.rs) to
    /// dodge the shared-cwd `mcpServers` collision (M6b).
    ClaudeLocalScope,
    /// codex: a single global `[mcp_servers.flockmux-swarm]` section in
    /// `~/.codex/config.toml`; per-spawn identity rides in via env.
    CodexGlobalToml,
}

/// Where/how the wake Stop-hook is materialized (the timeout-unit divergence —
/// claude ms vs codex s — lives inside the per-format writer for now; a future
/// step lifts it into a `[stop_hook]` table).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum StopHookFormat {
    #[default]
    None,
    /// `<ws>/.claude/settings.local.json` (`hooks.Stop[]`, timeout in ms).
    ClaudeSettingsLocal,
    /// `<ws>/.codex/hooks.json` (`hooks.Stop[]`, timeout in seconds).
    CodexHooksJson,
}

/// Kind of a [`ReadyStep`]. Today only `answer_dialog` is implemented; the
/// golutra-style sequential kinds (`wait_for`, `input`, `extract_session_id`)
/// are the next increment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ReadyStepKind {
    /// Watch PTY output for `needle`; inject `response` once when it appears.
    #[default]
    AnswerDialog,
}

/// One step of a CLI's post-spawn readiness automation — host-side handling
/// of first-spawn TUI dialogs that would otherwise block a headless PTY. This
/// is the **data-driven replacement** for the hard-coded `spawn::DialogAutoAnswer`:
/// any CLI lists its own dialogs in `cli-plugins/<id>.toml`, no Rust change.
///
/// ```toml
/// [[ready_plan]]
/// kind = "answer_dialog"
/// needle = "Hooks need review"
/// response = "2\r"        # TOML honors \r (Enter); injected verbatim
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadyStep {
    #[serde(default)]
    pub kind: ReadyStepKind,
    /// `answer_dialog`: substring to match in PTY output (matched on raw bytes,
    /// stitched across chunk boundaries).
    #[serde(default)]
    pub needle: String,
    /// `answer_dialog`: bytes to inject once `needle` appears. TOML escapes
    /// like `\r` are honored.
    #[serde(default)]
    pub response: String,
    /// `answer_dialog`: stop watching after this many ms (default 30s) so we
    /// don't pattern-match routine agent output forever.
    #[serde(default = "default_answer_window_ms")]
    pub window_ms: u64,
}

fn default_answer_window_ms() -> u64 {
    30_000
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliPlugin {
    pub id: String,
    pub display_name: String,
    pub binary: String,
    #[serde(default)]
    pub default_args: Vec<String>,
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
    /// Post-spawn readiness automation: auto-answer first-spawn TUI dialogs
    /// that would block a headless PTY (e.g. codex 0.130+'s "Hooks need
    /// review" menu). Data-driven — replaces the old `auto_answer_hooks_dialog`
    /// bool + the hard-coded needle/response in Rust. See [`ReadyStep`] and
    /// `spawn::ReadyPlanRunner`. Empty for CLIs with no blocking dialogs.
    #[serde(default)]
    pub ready_plan: Vec<ReadyStep>,

    /// Which trust-config format to write when `auto_trust_workspace` is set.
    /// Dispatch is keyed on this, NOT on `plugin.id` (see `TrustFormat`).
    #[serde(default)]
    pub trust_format: TrustFormat,
    /// Which MCP-injection format to use when `auto_inject_mcp` is set.
    #[serde(default)]
    pub mcp_format: McpFormat,
    /// Which Stop-hook format to install when `auto_inject_stop_hook` is set.
    #[serde(default)]
    pub stop_hook_format: StopHookFormat,
}

fn default_home_env() -> String { "HOME".into() }

#[derive(Debug, Clone, Default)]
pub struct PluginRegistry {
    plugins: HashMap<String, CliPlugin>,
}

impl PluginRegistry {
    /// Load every `cli-plugins/*.toml`. **One malformed file is skipped with a
    /// warning, not fatal** — a single bad/partial plugin must not take down
    /// ALL CLIs (and thus server boot). This mirrors `roles::RoleRegistry`'s
    /// tolerant policy; a missing dir is also non-fatal (empty registry).
    pub fn load_dir(dir: &Path) -> Result<Self> {
        let mut plugins: HashMap<String, CliPlugin> = HashMap::new();
        if !dir.is_dir() {
            tracing::warn!(dir = %dir.display(), "cli-plugins dir missing; no CLIs loaded");
            return Ok(Self { plugins });
        }
        let read = std::fs::read_dir(dir)
            .with_context(|| format!("read_dir({})", dir.display()))?;
        for entry in read {
            let path = match entry {
                Ok(e) => e.path(),
                Err(err) => {
                    tracing::warn!(?err, "skip unreadable cli-plugins dir entry");
                    continue;
                }
            };
            if path.extension().and_then(|s| s.to_str()) != Some("toml") {
                continue;
            }
            let bytes = match std::fs::read_to_string(&path) {
                Ok(b) => b,
                Err(err) => {
                    tracing::warn!(path = %path.display(), ?err, "skip cli-plugin: read failed");
                    continue;
                }
            };
            let plugin: CliPlugin = match toml::from_str(&bytes) {
                Ok(p) => p,
                Err(err) => {
                    tracing::warn!(path = %path.display(), error = %err, "skip cli-plugin: parse failed");
                    continue;
                }
            };
            if let Some(prev) = plugins.insert(plugin.id.clone(), plugin) {
                tracing::warn!(id = %prev.id, "duplicate cli-plugin id; later file wins");
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The shipped manifests must declare the format enums pre_spawn.rs / spawn.rs
    /// dispatch on. Guards the L2 backfill against an accidental drop/typo that
    /// would silently turn an agent into a coordination-dead island.
    #[test]
    fn shipped_manifests_declare_formats() {
        let reg = PluginRegistry::load_dir(&default_plugins_dir()).expect("load shipped plugins");

        let claude = reg.get("claude").expect("claude plugin present");
        assert_eq!(claude.trust_format, TrustFormat::ClaudeJson);
        assert_eq!(claude.mcp_format, McpFormat::ClaudeLocalScope);
        assert_eq!(claude.stop_hook_format, StopHookFormat::ClaudeSettingsLocal);

        let codex = reg.get("codex").expect("codex plugin present");
        assert_eq!(codex.trust_format, TrustFormat::CodexToml);
        assert_eq!(codex.mcp_format, McpFormat::CodexGlobalToml);
        assert_eq!(codex.stop_hook_format, StopHookFormat::CodexHooksJson);

        // The codex "Hooks need review" auto-answer migrated from the old
        // auto_answer_hooks_dialog bool into a data-driven ready_plan step.
        assert!(
            codex.ready_plan.iter().any(|s| {
                s.kind == ReadyStepKind::AnswerDialog
                    && s.needle == "Hooks need review"
                    && s.response == "2\r"
            }),
            "codex.toml must carry the Hooks-need-review answer_dialog in ready_plan",
        );
        // claude has no first-spawn blocking dialog → empty plan.
        assert!(claude.ready_plan.is_empty(), "claude ships no ready_plan");
    }

    /// A new field with a kebab-case typo must FAIL parse (→ warn-skip at load),
    /// never silently deserialize to `None` and strand the agent.
    #[test]
    fn typoed_format_value_fails_parse() {
        let good = r#"id="x"
display_name="X"
binary="x"
mcp_format="claude-local-scope"
"#;
        assert!(toml::from_str::<CliPlugin>(good).is_ok());

        let typo = r#"id="x"
display_name="X"
binary="x"
mcp_format="claude-locl-scope"
"#;
        assert!(
            toml::from_str::<CliPlugin>(typo).is_err(),
            "a typo'd mcp_format must be rejected, not defaulted to None"
        );
    }

    /// Formats default to `None` when omitted, so an unconfigured CLI degrades
    /// loudly (run_patches warns) rather than mis-dispatching.
    #[test]
    fn formats_default_to_none() {
        let minimal = r#"id="x"
display_name="X"
binary="x"
"#;
        let p: CliPlugin = toml::from_str(minimal).unwrap();
        assert_eq!(p.trust_format, TrustFormat::None);
        assert_eq!(p.mcp_format, McpFormat::None);
        assert_eq!(p.stop_hook_format, StopHookFormat::None);
    }
}
