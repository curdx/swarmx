//! Per-CLI patches applied to the user's home directory *before* we spawn a
//! child CLI under the shim. They exist to suppress interactive prompts that
//! would otherwise block the headless PTY (we have no good way to deliver the
//! single keystroke they want):
//!
//!   - `mark_claude_workspace_trusted`  — `~/.claude.json`
//!         projects[<workspace>].hasTrustDialogAccepted = true
//!     Skips claude's "Do you trust this folder?" gate. Safe because flockmux
//!     only points claude at workspaces it created itself under
//!     `~/.flockmux/workspaces/`.
//!
//!   - `mark_codex_workspace_trusted`   — `~/.codex/config.toml`
//!         [projects."<workspace>"] trust_level = "trusted"
//!     Skips codex's "Do you trust the contents of this directory?" gate.
//!     Appended as a fresh TOML section so user's existing comments / format
//!     are preserved (no round-trip through serde).
//!
//!   - `mark_codex_update_dismissed`    — `~/.codex/version.json`
//!         dismissed_version = latest_version
//!     Skips codex's "Update available! Press enter to continue" prompt that
//!     blocks the first prompt line. codex still surfaces updates outside
//!     flockmux.
//!
//! Each function is a no-op if the target file doesn't exist or already has
//! the desired state. JSON writes are atomic (write-temp + rename); the codex
//! TOML patch appends in place because we don't want to rewrite the whole
//! file (and lose user comments) for one new section.

use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

fn home_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

fn write_json_atomic(target: &Path, root: &Value) -> Result<()> {
    // `with_extension` REPLACES the old extension, so for `version.json` we'd
    // get `version.flockmux-tmp` (losing `.json` in the sibling-name). That's
    // still fine — the tmp lives in the same directory and we rename to the
    // exact `target` path anyway — but we keep the ".json" by appending.
    let tmp = match target.extension().and_then(|s| s.to_str()) {
        Some(ext) => target.with_extension(format!("{ext}.flockmux-tmp")),
        None => target.with_extension("flockmux-tmp"),
    };
    {
        let mut f = fs::File::create(&tmp)
            .with_context(|| format!("create {}", tmp.display()))?;
        f.write_all(&serde_json::to_vec_pretty(root)?)?;
        f.sync_all().ok();
    }
    fs::rename(&tmp, target).with_context(|| format!("rename to {}", target.display()))?;
    Ok(())
}

/// Mark `workspace` as trusted in `~/.claude.json`. No-op if the file doesn't
/// exist (claude hasn't run yet) or already has the flag set.
pub fn mark_claude_workspace_trusted(workspace: &Path) -> Result<()> {
    let cfg = match home_path().map(|h| h.join(".claude.json")) {
        Some(p) if p.is_file() => p,
        _ => return Ok(()),
    };
    patch_claude_trust_at(&cfg, workspace)
}

fn patch_claude_trust_at(cfg: &Path, workspace: &Path) -> Result<()> {
    let bytes = fs::read(cfg).with_context(|| format!("read {}", cfg.display()))?;
    let mut root: Value =
        serde_json::from_slice(&bytes).with_context(|| format!("parse {}", cfg.display()))?;

    let key = workspace.to_string_lossy().into_owned();
    let projects = root
        .as_object_mut()
        .context(".claude.json root is not an object")?
        .entry("projects")
        .or_insert_with(|| json!({}));
    let projects = projects
        .as_object_mut()
        .context(".claude.json projects is not an object")?;

    let entry = projects.entry(key).or_insert_with(|| json!({}));
    let entry = entry
        .as_object_mut()
        .context(".claude.json project entry is not an object")?;

    if entry
        .get("hasTrustDialogAccepted")
        .and_then(|v| v.as_bool())
        == Some(true)
    {
        return Ok(());
    }
    entry.insert("hasTrustDialogAccepted".into(), Value::Bool(true));

    write_json_atomic(cfg, &root)
}

/// Set `dismissed_version = latest_version` in `~/.codex/version.json` so
/// codex won't print "Update available! Press enter to continue" — that
/// prompt blocks our headless PTY waiting on a key we have no good way to
/// deliver.
///
/// No-op if the file doesn't exist, `latest_version` is missing, or
/// `dismissed_version` already matches.
pub fn mark_codex_update_dismissed() -> Result<()> {
    let cfg = match home_path().map(|h| h.join(".codex").join("version.json")) {
        Some(p) if p.is_file() => p,
        _ => return Ok(()),
    };
    patch_codex_dismiss_at(&cfg)
}

fn patch_codex_dismiss_at(cfg: &Path) -> Result<()> {
    let bytes = fs::read(cfg).with_context(|| format!("read {}", cfg.display()))?;
    let mut root: Value =
        serde_json::from_slice(&bytes).with_context(|| format!("parse {}", cfg.display()))?;

    let obj = root
        .as_object_mut()
        .context("version.json root is not an object")?;

    let latest = match obj.get("latest_version").and_then(|v| v.as_str()) {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => return Ok(()),
    };
    if obj.get("dismissed_version").and_then(|v| v.as_str()) == Some(latest.as_str()) {
        return Ok(());
    }
    obj.insert("dismissed_version".into(), Value::String(latest));

    write_json_atomic(cfg, &root)
}

/// Mark `workspace` as trusted in `~/.codex/config.toml`. Appends a fresh
/// `[projects."<workspace>"] trust_level = "trusted"` section if missing,
/// otherwise no-op. We don't round-trip the TOML through serde because the
/// user's config almost certainly contains comments / hand-arranged sections
/// we should preserve verbatim.
pub fn mark_codex_workspace_trusted(workspace: &Path) -> Result<()> {
    let cfg = match home_path().map(|h| h.join(".codex").join("config.toml")) {
        Some(p) if p.is_file() => p,
        _ => return Ok(()),
    };
    patch_codex_trust_at(&cfg, workspace)
}

fn patch_codex_trust_at(cfg: &Path, workspace: &Path) -> Result<()> {
    let existing = fs::read_to_string(cfg)
        .with_context(|| format!("read {}", cfg.display()))?;

    let key = workspace.to_string_lossy();
    // codex emits exactly this header style; matching it on its own line is
    // enough — flockmux paths never need TOML literal-key escaping.
    let header = format!("[projects.\"{key}\"]");
    let already = existing
        .lines()
        .any(|line| line.trim() == header);
    if already {
        return Ok(());
    }

    let mut out = existing;
    if !out.ends_with('\n') {
        out.push('\n');
    }
    if !out.ends_with("\n\n") {
        out.push('\n');
    }
    out.push_str(&header);
    out.push('\n');
    out.push_str("trust_level = \"trusted\"\n");

    // Atomic write — codex itself opens & rewrites this file on session
    // start, so a half-written file would be poison.
    let tmp = cfg.with_extension("toml.flockmux-tmp");
    {
        let mut f = fs::File::create(&tmp)
            .with_context(|| format!("create {}", tmp.display()))?;
        f.write_all(out.as_bytes())?;
        f.sync_all().ok();
    }
    fs::rename(&tmp, cfg).with_context(|| format!("rename to {}", cfg.display()))?;
    Ok(())
}

/// Mark `flockmux-swarm` as a local-scope MCP server in
/// `~/.claude.json projects.<workspace>.mcpServers`. We use *local* scope (per
/// project, baked into `~/.claude.json`) rather than *project* scope
/// (`.mcp.json` in the repo) so claude never shows the "do you trust this MCP
/// server?" prompt — local scope is implicitly trusted because the user
/// owns the file.
///
/// Each spawn writes its own `args` carrying `--agent-id <id>` plus an env
/// passthrough block. Two channels for the same data: claude's
/// `args ["--agent-id", "..."]` and `env { FLOCKMUX_AGENT_ID: "..." }`. If the
/// user later runs `flockmux-mcp` by hand the env wins; if claude clears the
/// env block, the args still identify the agent.
///
/// No-op if the file doesn't exist (claude hasn't run yet) or the entry is
/// already up-to-date.
pub fn mark_claude_mcp_local(
    workspace: &Path,
    agent_id: &str,
    mcp_bin: &Path,
    server_url: &str,
) -> Result<()> {
    let cfg = match home_path().map(|h| h.join(".claude.json")) {
        Some(p) if p.is_file() => p,
        _ => return Ok(()),
    };
    patch_claude_mcp_at(&cfg, workspace, agent_id, mcp_bin, server_url)
}

fn patch_claude_mcp_at(
    cfg: &Path,
    workspace: &Path,
    agent_id: &str,
    mcp_bin: &Path,
    server_url: &str,
) -> Result<()> {
    let bytes = fs::read(cfg).with_context(|| format!("read {}", cfg.display()))?;
    let mut root: Value =
        serde_json::from_slice(&bytes).with_context(|| format!("parse {}", cfg.display()))?;

    let ws_key = workspace.to_string_lossy().into_owned();
    let projects = root
        .as_object_mut()
        .context(".claude.json root is not an object")?
        .entry("projects")
        .or_insert_with(|| json!({}));
    let projects = projects
        .as_object_mut()
        .context(".claude.json projects is not an object")?;
    let project = projects.entry(ws_key).or_insert_with(|| json!({}));
    let project = project
        .as_object_mut()
        .context(".claude.json project entry is not an object")?;

    let mcp_servers = project
        .entry("mcpServers")
        .or_insert_with(|| json!({}));
    let mcp_servers = mcp_servers
        .as_object_mut()
        .context(".claude.json mcpServers is not an object")?;

    let desired = json!({
        "type": "stdio",
        "command": mcp_bin.to_string_lossy(),
        "args": ["--agent-id", agent_id],
        "env": {
            "FLOCKMUX_AGENT_ID": agent_id,
            "FLOCKMUX_SERVER_URL": server_url,
        }
    });

    if mcp_servers.get("flockmux-swarm") == Some(&desired) {
        return Ok(());
    }
    mcp_servers.insert("flockmux-swarm".into(), desired);

    write_json_atomic(cfg, &root)
}

/// Append a global `[mcp_servers.flockmux-swarm]` section to
/// `~/.codex/config.toml` if it's missing or its `command =` line differs.
/// Per-spawn data (which agent) does NOT live in this section — codex's MCP
/// config is global. Instead we whitelist the env vars and codex passes them
/// through to the subprocess; flockmux-server already sets
/// `FLOCKMUX_AGENT_ID` on each spawn.
///
/// `default_tools_approval_mode = "auto"` skips codex's per-call approval
/// gate (our tools are loopback-only, idempotent or undoable).
///
/// Rewrites the section in place if `command =` no longer points at the
/// current binary (handles `cargo build` moving the path between runs).
pub fn ensure_codex_mcp_global(mcp_bin: &Path) -> Result<()> {
    let cfg = match home_path().map(|h| h.join(".codex").join("config.toml")) {
        Some(p) => p,
        None => return Ok(()),
    };
    // Codex's MCP block is independent of trust — we should be able to write
    // it even if the user has never run codex yet (the dir might not exist).
    if let Some(parent) = cfg.parent() {
        fs::create_dir_all(parent).ok();
    }
    let existing = if cfg.is_file() {
        fs::read_to_string(&cfg).with_context(|| format!("read {}", cfg.display()))?
    } else {
        String::new()
    };
    let desired_section = render_codex_mcp_section(mcp_bin);

    let updated = match find_section_range(&existing, "[mcp_servers.flockmux-swarm]") {
        Some((start, end)) => {
            // Strip trailing blank line(s) on either side so we don't grow
            // the file every time we rewrite.
            let mut new_body = String::with_capacity(existing.len());
            new_body.push_str(&existing[..start]);
            new_body.push_str(&desired_section);
            new_body.push_str(&existing[end..]);
            if new_body == existing {
                return Ok(());
            }
            new_body
        }
        None => {
            let mut out = existing;
            if !out.is_empty() {
                if !out.ends_with('\n') {
                    out.push('\n');
                }
                if !out.ends_with("\n\n") {
                    out.push('\n');
                }
            }
            out.push_str(&desired_section);
            out
        }
    };

    let tmp = cfg.with_extension("toml.flockmux-tmp");
    {
        let mut f = fs::File::create(&tmp)
            .with_context(|| format!("create {}", tmp.display()))?;
        f.write_all(updated.as_bytes())?;
        f.sync_all().ok();
    }
    fs::rename(&tmp, &cfg).with_context(|| format!("rename to {}", cfg.display()))?;
    Ok(())
}

fn render_codex_mcp_section(mcp_bin: &Path) -> String {
    format!(
        "[mcp_servers.flockmux-swarm]\n\
         command = \"{}\"\n\
         env_vars = [\"FLOCKMUX_AGENT_ID\", \"FLOCKMUX_SERVER_URL\"]\n\
         default_tools_approval_mode = \"auto\"\n\
         startup_timeout_sec = 10\n",
        mcp_bin.to_string_lossy(),
    )
}

/// Locate `header` (matched against `line.trim()`) and return the half-open
/// byte range `[start, end)` covering the entire section: header line through
/// the line just before the next `[...]` header (or EOF).
fn find_section_range(haystack: &str, header: &str) -> Option<(usize, usize)> {
    let mut start: Option<usize> = None;
    let mut cursor = 0usize;
    for line in haystack.split_inclusive('\n') {
        let line_start = cursor;
        cursor += line.len();
        let stripped = line.trim_end_matches('\n').trim_end_matches('\r').trim();
        if let Some(s) = start {
            // We're inside our section — stop at the next TOML section header.
            if stripped.starts_with('[') && stripped.ends_with(']') {
                return Some((s, line_start));
            }
            continue;
        }
        if stripped == header {
            start = Some(line_start);
        }
    }
    start.map(|s| (s, haystack.len()))
}

// ── Stop-hook patches (M5b wake) ─────────────────────────────────────────
//
// Each CLI ships a Stop event hook system; we use it to push a synthetic
// continuation prompt whenever the agent has unread swarm mail. Both CLIs
// agree on the wire protocol:
//   stdout JSON `{}`                                  → no-op
//   stdout JSON `{"decision":"block", "reason":...}`  → continue another turn
// but they DIFFER on the config schema's `timeout` unit:
//   - Claude  (~/.claude/settings.local.json): timeout in **milliseconds**.
//   - Codex   (~/.codex/hooks.json):           timeout in **seconds**.
// Mixing them silently truncates / explodes the cap. Read carefully.
//
// The hook command line is built once per spawn and embedded into the JSON:
//
//   <mcp_bin> wake-check --agent-id <agent_id> --server <server_url>
//
// `mcp_bin` is an absolute path (PreSpawnCtx already resolves it), so the
// hook is immune to PATH drift between user shell and CLI subprocess.

const WAKE_HOOK_TIMEOUT_MS: i64 = 10_000; // claude wants ms
const WAKE_HOOK_TIMEOUT_S: i64 = 10; // codex wants s

fn render_wake_command(mcp_bin: &Path, agent_id: &str, server_url: &str) -> String {
    format!(
        "{} wake-check --agent-id {} --server {}",
        // Note: we don't shell-quote because spawn pipelines invoke the
        // string via the CLI's shell-out path (claude/codex both use sh
        // -c). agent_id is alnum + dash (server-allocated), server_url is
        // an http/https URL — neither contains shell metachars in practice.
        mcp_bin.to_string_lossy(),
        agent_id,
        server_url,
    )
}

/// Merge a flockmux wake-check entry into `root.hooks.Stop`. Idempotent on
/// the agent_id-bearing command string: re-installing for the same agent
/// drops the prior row and re-appends, so repeat spawns don't grow the
/// array. Different agent_ids coexist as separate rows.
fn merge_stop_hook(root: &mut Value, command: &str, timeout: i64) {
    // Ensure `hooks` exists and is an object.
    if !root.is_object() {
        *root = json!({});
    }
    let obj = root.as_object_mut().unwrap();
    let hooks = obj.entry("hooks").or_insert_with(|| json!({}));
    if !hooks.is_object() {
        *hooks = json!({});
    }
    let hooks_obj = hooks.as_object_mut().unwrap();
    let stop = hooks_obj.entry("Stop").or_insert_with(|| json!([]));
    if !stop.is_array() {
        *stop = json!([]);
    }
    let stop_arr = stop.as_array_mut().unwrap();

    // Drop any prior entry carrying the exact same command (same agent_id).
    stop_arr.retain(|entry| {
        let matches = entry
            .get("hooks")
            .and_then(|v| v.as_array())
            .map(|inner| {
                inner.iter().any(|h| {
                    h.get("command").and_then(|v| v.as_str()) == Some(command)
                })
            })
            .unwrap_or(false);
        !matches
    });

    // Append at the END so user-declared Stop hooks fire first — friendly
    // behavior: their lint / test gating isn't bypassed by our wake noise.
    // Claude requires `matcher: ""`; codex tolerates its absence but we set
    // it for uniformity.
    stop_arr.push(json!({
        "matcher": "",
        "hooks": [{
            "type": "command",
            "command": command,
            "timeout": timeout,
        }]
    }));
}

/// Write a workspace-local `.claude/settings.local.json` carrying a Stop
/// hook that calls `flockmux-mcp wake-check`.
///
/// Project-local (workspace-scoped) on purpose: we don't want to pollute
/// the user's `~/.claude/settings.json`, and the hook is only meaningful
/// inside the flockmux-managed workspace anyway.
pub fn install_claude_stop_hook(
    workspace: &Path,
    agent_id: &str,
    mcp_bin: &Path,
    server_url: &str,
) -> Result<()> {
    let cfg_dir = workspace.join(".claude");
    fs::create_dir_all(&cfg_dir)
        .with_context(|| format!("mkdir {}", cfg_dir.display()))?;
    let cfg = cfg_dir.join("settings.local.json");
    install_claude_stop_hook_at(&cfg, agent_id, mcp_bin, server_url)
}

fn install_claude_stop_hook_at(
    cfg: &Path,
    agent_id: &str,
    mcp_bin: &Path,
    server_url: &str,
) -> Result<()> {
    let mut root: Value = if cfg.is_file() {
        let bytes = fs::read(cfg).with_context(|| format!("read {}", cfg.display()))?;
        serde_json::from_slice(&bytes)
            .with_context(|| format!("parse {}", cfg.display()))?
    } else {
        json!({})
    };

    let command = render_wake_command(mcp_bin, agent_id, server_url);
    merge_stop_hook(&mut root, &command, WAKE_HOOK_TIMEOUT_MS);
    write_json_atomic(cfg, &root)
}

/// Write a workspace-local `.codex/hooks.json` carrying a Stop hook that
/// calls `flockmux-mcp wake-check`. Same structural shape as claude's
/// settings.local.json but `timeout` is in **seconds**, not ms.
pub fn install_codex_stop_hook(
    workspace: &Path,
    agent_id: &str,
    mcp_bin: &Path,
    server_url: &str,
) -> Result<()> {
    let cfg_dir = workspace.join(".codex");
    fs::create_dir_all(&cfg_dir)
        .with_context(|| format!("mkdir {}", cfg_dir.display()))?;
    let cfg = cfg_dir.join("hooks.json");
    install_codex_stop_hook_at(&cfg, agent_id, mcp_bin, server_url)
}

fn install_codex_stop_hook_at(
    cfg: &Path,
    agent_id: &str,
    mcp_bin: &Path,
    server_url: &str,
) -> Result<()> {
    let mut root: Value = if cfg.is_file() {
        let bytes = fs::read(cfg).with_context(|| format!("read {}", cfg.display()))?;
        serde_json::from_slice(&bytes)
            .with_context(|| format!("parse {}", cfg.display()))?
    } else {
        json!({})
    };

    let command = render_wake_command(mcp_bin, agent_id, server_url);
    merge_stop_hook(&mut root, &command, WAKE_HOOK_TIMEOUT_S);
    write_json_atomic(cfg, &root)
}

/// Dispatch into per-CLI patch sequences. Each CLI has its own readable
/// top-to-bottom block listing every patch that applies to it; the host
/// never interleaves them. Failures are logged at `warn!` but never
/// propagated — at worst the user sees the prompt we tried to suppress
/// (or the agent is missing the swarm tool block), which is annoying but
/// not fatal.
///
/// Adding a new CLI is two steps:
///   1. add a `cli-plugins/<id>.toml` and set the auto-* flags you want;
///   2. add a `run_<id>_patches` fn here and route to it from the match.
pub fn run_patches(
    plugin: &crate::plugins::CliPlugin,
    workspace: &Path,
    ctx: &PreSpawnCtx,
) {
    match plugin.id.as_str() {
        "claude" => run_claude_patches(plugin, workspace, ctx),
        "codex" => run_codex_patches(plugin, workspace, ctx),
        other => {
            tracing::debug!(
                cli = %other,
                "no pre-spawn patch handler registered for this CLI"
            );
        }
    }
}

/// All `claude`-specific pre-spawn patches, in execution order. Each step
/// is gated on its `auto_*` flag in the plugin manifest, so a host that
/// only wants trust auto-accept (no MCP, no hook) can opt out cleanly.
fn run_claude_patches(
    plugin: &crate::plugins::CliPlugin,
    workspace: &Path,
    ctx: &PreSpawnCtx,
) {
    // 1. Auto-accept "Do you trust this folder?" — workspaces are flockmux-owned.
    if plugin.auto_trust_workspace {
        if let Err(err) = mark_claude_workspace_trusted(workspace) {
            tracing::warn!(?err, "claude: auto-trust patch failed");
        }
    }
    // 2. Register flockmux-swarm as a local-scope MCP server with this
    //    spawn's agent_id baked into args + env.
    if plugin.auto_inject_mcp {
        if let Err(err) = mark_claude_mcp_local(
            workspace,
            &ctx.agent_id,
            &ctx.mcp_bin,
            &ctx.server_url,
        ) {
            tracing::warn!(?err, "claude: mcp-inject patch failed");
        }
    }
    // 3. Install <workspace>/.claude/settings.local.json Stop hook (M5b
    //    wake-check). Timeout is in MILLISECONDS for claude.
    if plugin.auto_inject_stop_hook {
        if let Err(err) = install_claude_stop_hook(
            workspace,
            &ctx.agent_id,
            &ctx.mcp_bin,
            &ctx.server_url,
        ) {
            tracing::warn!(?err, "claude: stop-hook install failed");
        }
    }
}

/// All `codex`-specific pre-spawn patches, in execution order. Codex has
/// one extra step over claude (auto-dismiss the "update available" prompt
/// that blocks a headless PTY), and writes to different files / different
/// timeout units — keep them paired here so the differences are visible
/// at a glance.
fn run_codex_patches(
    plugin: &crate::plugins::CliPlugin,
    workspace: &Path,
    ctx: &PreSpawnCtx,
) {
    // 1. Auto-accept "Do you trust the contents of this directory?".
    if plugin.auto_trust_workspace {
        if let Err(err) = mark_codex_workspace_trusted(workspace) {
            tracing::warn!(?err, "codex: auto-trust patch failed");
        }
    }
    // 2. Mark the latest codex release as already-dismissed so the
    //    "Update available! Press enter to continue" prompt is skipped.
    //    (claude has no equivalent prompt.)
    if plugin.auto_dismiss_update {
        if let Err(err) = mark_codex_update_dismissed() {
            tracing::warn!(?err, "codex: auto-dismiss-update patch failed");
        }
    }
    // 3. Ensure ~/.codex/config.toml has the global flockmux-swarm MCP
    //    server block. Per-spawn identity rides in via FLOCKMUX_AGENT_ID
    //    env passthrough (whitelisted in env_vars).
    if plugin.auto_inject_mcp {
        if let Err(err) = ensure_codex_mcp_global(&ctx.mcp_bin) {
            tracing::warn!(?err, "codex: mcp-inject patch failed");
        }
    }
    // 4. Install <workspace>/.codex/hooks.json Stop hook (M5b wake-check).
    //    Timeout is in SECONDS for codex — different from claude.
    if plugin.auto_inject_stop_hook {
        if let Err(err) = install_codex_stop_hook(
            workspace,
            &ctx.agent_id,
            &ctx.mcp_bin,
            &ctx.server_url,
        ) {
            tracing::warn!(?err, "codex: stop-hook install failed");
        }
    }
}

/// Per-spawn context that the host computes once and threads into pre-spawn
/// patches. Currently only the MCP-inject patch needs it; everything else
/// works from the plugin + workspace alone.
#[derive(Debug, Clone)]
pub struct PreSpawnCtx {
    /// The agent_id flockmux-server allocated for this spawn.
    pub agent_id: String,
    /// Absolute path to `flockmux-mcp` — baked into the per-spawn entry so
    /// the path doesn't drift with the user's CWD or PATH.
    pub mcp_bin: PathBuf,
    /// Base URL of the flockmux-server REST API the MCP subprocess will talk
    /// to. Loopback today, but the field exists so a remote pairing mode
    /// doesn't need a schema change.
    pub server_url: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;

    #[test]
    fn claude_trust_sets_flag_for_new_workspace() {
        let dir = tempdir().unwrap();
        let cfg = dir.path().join("claude.json");
        fs::write(&cfg, serde_json::to_vec(&json!({ "projects": {} })).unwrap()).unwrap();

        let workspace = dir.path().join("ws-A");
        patch_claude_trust_at(&cfg, &workspace).unwrap();

        let written: Value = serde_json::from_slice(&fs::read(&cfg).unwrap()).unwrap();
        assert_eq!(
            written["projects"][workspace.to_string_lossy().as_ref()]["hasTrustDialogAccepted"],
            json!(true)
        );
    }

    #[test]
    fn claude_trust_noop_when_already_set() {
        let dir = tempdir().unwrap();
        let cfg = dir.path().join("claude.json");
        let ws_key = dir.path().join("ws-B").to_string_lossy().into_owned();
        fs::write(
            &cfg,
            serde_json::to_vec(&json!({
                "projects": { &ws_key: { "hasTrustDialogAccepted": true, "other": 42 } }
            }))
            .unwrap(),
        )
        .unwrap();
        let before = fs::read(&cfg).unwrap();

        patch_claude_trust_at(&cfg, &dir.path().join("ws-B")).unwrap();

        let after = fs::read(&cfg).unwrap();
        // No-op path must not rewrite the file (preserves user-set fields verbatim).
        assert_eq!(before, after);
    }

    #[test]
    fn claude_trust_preserves_other_projects() {
        let dir = tempdir().unwrap();
        let cfg = dir.path().join("claude.json");
        fs::write(
            &cfg,
            serde_json::to_vec(&json!({
                "projects": { "/some/other": { "hasTrustDialogAccepted": true, "tag": "keep-me" } }
            }))
            .unwrap(),
        )
        .unwrap();

        patch_claude_trust_at(&cfg, &dir.path().join("ws-C")).unwrap();

        let after: Value = serde_json::from_slice(&fs::read(&cfg).unwrap()).unwrap();
        assert_eq!(after["projects"]["/some/other"]["tag"], json!("keep-me"));
        assert_eq!(
            after["projects"][dir.path().join("ws-C").to_string_lossy().as_ref()]
                ["hasTrustDialogAccepted"],
            json!(true)
        );
    }

    #[test]
    fn codex_dismiss_updates_to_latest() {
        let dir = tempdir().unwrap();
        let cfg = dir.path().join("version.json");
        fs::write(
            &cfg,
            serde_json::to_vec(&json!({
                "latest_version": "0.132.0",
                "last_checked_at": "2026-05-20T00:00:00Z",
                "dismissed_version": "0.65.0"
            }))
            .unwrap(),
        )
        .unwrap();

        patch_codex_dismiss_at(&cfg).unwrap();

        let after: Value = serde_json::from_slice(&fs::read(&cfg).unwrap()).unwrap();
        assert_eq!(after["dismissed_version"], json!("0.132.0"));
        // last_checked_at must be preserved — codex owns that field.
        assert_eq!(after["last_checked_at"], json!("2026-05-20T00:00:00Z"));
    }

    #[test]
    fn codex_dismiss_noop_when_already_latest() {
        let dir = tempdir().unwrap();
        let cfg = dir.path().join("version.json");
        let original = json!({
            "latest_version": "0.132.0",
            "dismissed_version": "0.132.0"
        });
        fs::write(&cfg, serde_json::to_vec(&original).unwrap()).unwrap();
        let before_bytes = fs::read(&cfg).unwrap();

        patch_codex_dismiss_at(&cfg).unwrap();

        assert_eq!(fs::read(&cfg).unwrap(), before_bytes);
    }

    #[test]
    fn codex_trust_appends_section_when_missing() {
        let dir = tempdir().unwrap();
        let cfg = dir.path().join("config.toml");
        let original = "\
model = \"gpt-5.5\"

# user comment that must survive
[projects.\"/some/other\"]
trust_level = \"trusted\"
";
        fs::write(&cfg, original).unwrap();

        let ws = dir.path().join("ws-X");
        patch_codex_trust_at(&cfg, &ws).unwrap();

        let after = fs::read_to_string(&cfg).unwrap();
        assert!(after.contains("# user comment that must survive"), "comments preserved");
        assert!(after.contains("[projects.\"/some/other\"]"), "existing section kept");
        let expected_header = format!("[projects.\"{}\"]", ws.to_string_lossy());
        assert!(after.contains(&expected_header), "new header appended");
        assert!(
            after.lines().rev().take(3).any(|l| l == "trust_level = \"trusted\""),
            "trust_level set in new section",
        );
    }

    #[test]
    fn codex_trust_noop_when_section_already_present() {
        let dir = tempdir().unwrap();
        let cfg = dir.path().join("config.toml");
        let ws = dir.path().join("ws-Y");
        let header = format!("[projects.\"{}\"]", ws.to_string_lossy());
        let original = format!(
            "model = \"gpt-5.5\"\n\n{header}\ntrust_level = \"trusted\"\n"
        );
        fs::write(&cfg, &original).unwrap();
        let before = fs::read(&cfg).unwrap();

        patch_codex_trust_at(&cfg, &ws).unwrap();

        assert_eq!(fs::read(&cfg).unwrap(), before, "no-op when already present");
    }

    #[test]
    fn codex_dismiss_noop_when_latest_missing() {
        let dir = tempdir().unwrap();
        let cfg = dir.path().join("version.json");
        fs::write(
            &cfg,
            serde_json::to_vec(&json!({ "dismissed_version": "0.65.0" })).unwrap(),
        )
        .unwrap();
        let before_bytes = fs::read(&cfg).unwrap();

        patch_codex_dismiss_at(&cfg).unwrap();
        assert_eq!(fs::read(&cfg).unwrap(), before_bytes);
    }

    // ── claude MCP local-scope patch ─────────────────────────────────────

    #[test]
    fn claude_mcp_local_writes_new_entry() {
        let dir = tempdir().unwrap();
        let cfg = dir.path().join("claude.json");
        fs::write(&cfg, serde_json::to_vec(&json!({ "projects": {} })).unwrap()).unwrap();
        let ws = dir.path().join("ws-A");
        let bin = dir.path().join("flockmux-mcp");

        patch_claude_mcp_at(
            &cfg,
            &ws,
            "claude-aaa",
            &bin,
            "http://127.0.0.1:7777",
        )
        .unwrap();

        let written: Value = serde_json::from_slice(&fs::read(&cfg).unwrap()).unwrap();
        let entry = &written["projects"][ws.to_string_lossy().as_ref()]["mcpServers"]
            ["flockmux-swarm"];
        assert_eq!(entry["type"], json!("stdio"));
        assert_eq!(entry["command"], json!(bin.to_string_lossy().as_ref()));
        assert_eq!(entry["args"], json!(["--agent-id", "claude-aaa"]));
        assert_eq!(entry["env"]["FLOCKMUX_AGENT_ID"], json!("claude-aaa"));
        assert_eq!(
            entry["env"]["FLOCKMUX_SERVER_URL"],
            json!("http://127.0.0.1:7777")
        );
    }

    #[test]
    fn claude_mcp_local_noop_when_identical() {
        let dir = tempdir().unwrap();
        let cfg = dir.path().join("claude.json");
        fs::write(&cfg, serde_json::to_vec(&json!({ "projects": {} })).unwrap()).unwrap();
        let ws = dir.path().join("ws-B");
        let bin = dir.path().join("flockmux-mcp");

        patch_claude_mcp_at(&cfg, &ws, "claude-bbb", &bin, "http://127.0.0.1:7777").unwrap();
        let first = fs::read(&cfg).unwrap();
        patch_claude_mcp_at(&cfg, &ws, "claude-bbb", &bin, "http://127.0.0.1:7777").unwrap();
        let second = fs::read(&cfg).unwrap();
        assert_eq!(first, second, "second write must be a no-op");
    }

    #[test]
    fn claude_mcp_local_preserves_other_mcp_servers() {
        let dir = tempdir().unwrap();
        let cfg = dir.path().join("claude.json");
        let ws_key = dir.path().join("ws-C").to_string_lossy().into_owned();
        fs::write(
            &cfg,
            serde_json::to_vec(&json!({
                "projects": {
                    &ws_key: {
                        "mcpServers": {
                            "user-other": { "type": "stdio", "command": "/usr/bin/other" }
                        }
                    }
                }
            }))
            .unwrap(),
        )
        .unwrap();
        let ws = dir.path().join("ws-C");
        let bin = dir.path().join("flockmux-mcp");

        patch_claude_mcp_at(&cfg, &ws, "claude-ccc", &bin, "http://127.0.0.1:7777").unwrap();

        let after: Value = serde_json::from_slice(&fs::read(&cfg).unwrap()).unwrap();
        let mcp = &after["projects"][&ws_key]["mcpServers"];
        assert_eq!(mcp["user-other"]["command"], json!("/usr/bin/other"));
        assert_eq!(mcp["flockmux-swarm"]["env"]["FLOCKMUX_AGENT_ID"], json!("claude-ccc"));
    }

    // ── codex MCP global-config patch ────────────────────────────────────

    #[test]
    fn codex_mcp_global_appends_when_missing() {
        let dir = tempdir().unwrap();
        let cfg = dir.path().join("config.toml");
        let original = "model = \"gpt-5.5\"\n\n# user comment\n";
        fs::write(&cfg, original).unwrap();
        let bin = dir.path().join("flockmux-mcp");

        // Re-exercise the in-place logic via the same function by overriding
        // path through a local helper that mirrors the public one.
        ensure_codex_mcp_at(&cfg, &bin).unwrap();

        let after = fs::read_to_string(&cfg).unwrap();
        assert!(after.contains("# user comment"), "comments preserved");
        assert!(after.contains("[mcp_servers.flockmux-swarm]"));
        assert!(after.contains("default_tools_approval_mode = \"auto\""));
        assert!(after.contains("env_vars = [\"FLOCKMUX_AGENT_ID\", \"FLOCKMUX_SERVER_URL\"]"));
        assert!(after.contains(bin.to_string_lossy().as_ref()));
    }

    #[test]
    fn codex_mcp_global_noop_when_section_already_matches() {
        let dir = tempdir().unwrap();
        let cfg = dir.path().join("config.toml");
        let bin = dir.path().join("flockmux-mcp");
        ensure_codex_mcp_at(&cfg, &bin).unwrap();
        let first = fs::read(&cfg).unwrap();
        ensure_codex_mcp_at(&cfg, &bin).unwrap();
        let second = fs::read(&cfg).unwrap();
        assert_eq!(first, second, "second write must be a no-op");
    }

    #[test]
    fn codex_mcp_global_rewrites_when_command_differs() {
        let dir = tempdir().unwrap();
        let cfg = dir.path().join("config.toml");
        let bin_a = dir.path().join("flockmux-mcp-a");
        let bin_b = dir.path().join("flockmux-mcp-b");

        ensure_codex_mcp_at(&cfg, &bin_a).unwrap();
        let after_a = fs::read_to_string(&cfg).unwrap();
        assert!(after_a.contains(bin_a.to_string_lossy().as_ref()));

        ensure_codex_mcp_at(&cfg, &bin_b).unwrap();
        let after_b = fs::read_to_string(&cfg).unwrap();
        assert!(
            after_b.contains(bin_b.to_string_lossy().as_ref()),
            "new path must appear"
        );
        assert!(
            !after_b.contains(bin_a.to_string_lossy().as_ref()),
            "old path must be gone"
        );
        // Section must appear exactly once.
        let count = after_b.matches("[mcp_servers.flockmux-swarm]").count();
        assert_eq!(count, 1, "section duplicated: {after_b}");
    }

    #[test]
    fn codex_mcp_global_preserves_other_sections() {
        let dir = tempdir().unwrap();
        let cfg = dir.path().join("config.toml");
        let bin = dir.path().join("flockmux-mcp");
        let original = "\
[mcp_servers.user-other]\n\
command = \"/usr/bin/other\"\n\
env_vars = [\"X\"]\n\
\n\
[projects.\"/some/ws\"]\n\
trust_level = \"trusted\"\n";
        fs::write(&cfg, original).unwrap();

        ensure_codex_mcp_at(&cfg, &bin).unwrap();

        let after = fs::read_to_string(&cfg).unwrap();
        assert!(after.contains("[mcp_servers.user-other]"));
        assert!(after.contains("[projects.\"/some/ws\"]"));
        assert!(after.contains("[mcp_servers.flockmux-swarm]"));

        // Run again; user-other untouched.
        ensure_codex_mcp_at(&cfg, &bin).unwrap();
        let after2 = fs::read_to_string(&cfg).unwrap();
        assert!(after2.contains("[mcp_servers.user-other]"));
    }

    /// Mirror of `ensure_codex_mcp_global` operating on an explicit path so
    /// tests don't touch `~/.codex/config.toml`. The production function
    /// resolves the path via `home_path()` then defers to the same logic.
    fn ensure_codex_mcp_at(cfg: &Path, mcp_bin: &Path) -> Result<()> {
        if let Some(parent) = cfg.parent() {
            fs::create_dir_all(parent).ok();
        }
        let existing = if cfg.is_file() {
            fs::read_to_string(cfg)?
        } else {
            String::new()
        };
        let desired_section = render_codex_mcp_section(mcp_bin);
        let updated = match find_section_range(&existing, "[mcp_servers.flockmux-swarm]") {
            Some((start, end)) => {
                let mut new_body = String::with_capacity(existing.len());
                new_body.push_str(&existing[..start]);
                new_body.push_str(&desired_section);
                new_body.push_str(&existing[end..]);
                if new_body == existing {
                    return Ok(());
                }
                new_body
            }
            None => {
                let mut out = existing;
                if !out.is_empty() {
                    if !out.ends_with('\n') {
                        out.push('\n');
                    }
                    if !out.ends_with("\n\n") {
                        out.push('\n');
                    }
                }
                out.push_str(&desired_section);
                out
            }
        };
        let tmp = cfg.with_extension("toml.flockmux-tmp");
        {
            let mut f = fs::File::create(&tmp)?;
            f.write_all(updated.as_bytes())?;
            f.sync_all().ok();
        }
        fs::rename(&tmp, cfg)?;
        Ok(())
    }

    #[test]
    fn find_section_range_matches_until_next_header() {
        let body = "\
[a]\nx = 1\n\n[mcp_servers.flockmux-swarm]\ncommand = \"foo\"\nenv_vars = []\n\n[c]\ny = 2\n";
        let (start, end) =
            find_section_range(body, "[mcp_servers.flockmux-swarm]").unwrap();
        let section = &body[start..end];
        assert!(section.contains("command = \"foo\""));
        assert!(!section.contains("[c]"), "section bled past next header");
    }

    #[test]
    fn find_section_range_matches_until_eof_when_last_section() {
        let body = "[mcp_servers.flockmux-swarm]\ncommand = \"foo\"\n";
        let (start, end) =
            find_section_range(body, "[mcp_servers.flockmux-swarm]").unwrap();
        assert_eq!(end, body.len());
        let section = &body[start..end];
        assert!(section.contains("command = \"foo\""));
    }

    // ── M5b Stop-hook install patches ─────────────────────────────────────

    #[test]
    fn claude_stop_hook_creates_settings_local() {
        let dir = tempdir().unwrap();
        let cfg = dir.path().join("settings.local.json");
        let bin = dir.path().join("flockmux-mcp");
        install_claude_stop_hook_at(&cfg, "claude-aaa", &bin, "http://127.0.0.1:7777").unwrap();

        let root: Value = serde_json::from_slice(&fs::read(&cfg).unwrap()).unwrap();
        let stop = root["hooks"]["Stop"].as_array().expect("hooks.Stop is array");
        assert_eq!(stop.len(), 1);
        let inner = stop[0]["hooks"][0].clone();
        assert_eq!(inner["type"], json!("command"));
        assert_eq!(inner["timeout"], json!(10_000), "claude timeout in ms");
        let cmd = inner["command"].as_str().unwrap();
        assert!(cmd.contains("wake-check --agent-id claude-aaa"), "got: {cmd}");
        assert!(cmd.contains("--server http://127.0.0.1:7777"), "got: {cmd}");
        assert!(cmd.contains(bin.to_string_lossy().as_ref()), "absolute bin path: {cmd}");
    }

    #[test]
    fn codex_stop_hook_creates_hooks_json() {
        let dir = tempdir().unwrap();
        let cfg = dir.path().join("hooks.json");
        let bin = dir.path().join("flockmux-mcp");
        install_codex_stop_hook_at(&cfg, "codex-bbb", &bin, "http://127.0.0.1:7777").unwrap();

        let root: Value = serde_json::from_slice(&fs::read(&cfg).unwrap()).unwrap();
        let stop = root["hooks"]["Stop"].as_array().expect("hooks.Stop is array");
        assert_eq!(stop.len(), 1);
        let inner = stop[0]["hooks"][0].clone();
        assert_eq!(inner["type"], json!("command"));
        assert_eq!(
            inner["timeout"],
            json!(10),
            "codex timeout in SECONDS — ms would be 2.7h timeout",
        );
        let cmd = inner["command"].as_str().unwrap();
        assert!(cmd.contains("wake-check --agent-id codex-bbb"), "got: {cmd}");
    }

    #[test]
    fn claude_stop_hook_merges_existing_user_hooks() {
        let dir = tempdir().unwrap();
        let cfg = dir.path().join("settings.local.json");
        // Pre-seed with a user-defined PreToolUse hook + a user-defined Stop
        // hook. Both must survive verbatim.
        let original = json!({
            "hooks": {
                "PreToolUse": [{
                    "matcher": "Bash",
                    "hooks": [{ "type": "command", "command": "/usr/local/bin/user-lint", "timeout": 5000 }]
                }],
                "Stop": [{
                    "matcher": "",
                    "hooks": [{ "type": "command", "command": "/usr/local/bin/user-stop", "timeout": 5000 }]
                }]
            }
        });
        fs::write(&cfg, serde_json::to_vec_pretty(&original).unwrap()).unwrap();

        let bin = dir.path().join("flockmux-mcp");
        install_claude_stop_hook_at(&cfg, "claude-aaa", &bin, "http://127.0.0.1:7777").unwrap();

        let after: Value = serde_json::from_slice(&fs::read(&cfg).unwrap()).unwrap();
        // PreToolUse must be untouched.
        let pre = after["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(pre.len(), 1);
        assert_eq!(pre[0]["hooks"][0]["command"], json!("/usr/local/bin/user-lint"));
        // Stop now has TWO entries: the user's (first) and flockmux (last).
        let stop = after["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 2, "user hook should be preserved + wake-check appended");
        assert_eq!(
            stop[0]["hooks"][0]["command"],
            json!("/usr/local/bin/user-stop"),
            "user hook stays first",
        );
        let cmd = stop[1]["hooks"][0]["command"].as_str().unwrap();
        assert!(cmd.contains("wake-check"), "flockmux entry appended at end: {cmd}");
    }

    #[test]
    fn codex_stop_hook_idempotent_on_repeat_install() {
        let dir = tempdir().unwrap();
        let cfg = dir.path().join("hooks.json");
        let bin = dir.path().join("flockmux-mcp");

        install_codex_stop_hook_at(&cfg, "codex-xxx", &bin, "http://127.0.0.1:7777").unwrap();
        install_codex_stop_hook_at(&cfg, "codex-xxx", &bin, "http://127.0.0.1:7777").unwrap();
        install_codex_stop_hook_at(&cfg, "codex-xxx", &bin, "http://127.0.0.1:7777").unwrap();

        let root: Value = serde_json::from_slice(&fs::read(&cfg).unwrap()).unwrap();
        let stop = root["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 1, "re-running the same agent_id must not accumulate entries");
    }

    #[test]
    fn claude_stop_hook_distinct_agent_ids_coexist() {
        let dir = tempdir().unwrap();
        let cfg = dir.path().join("settings.local.json");
        let bin = dir.path().join("flockmux-mcp");

        install_claude_stop_hook_at(&cfg, "claude-A", &bin, "http://127.0.0.1:7777").unwrap();
        install_claude_stop_hook_at(&cfg, "claude-B", &bin, "http://127.0.0.1:7777").unwrap();

        let root: Value = serde_json::from_slice(&fs::read(&cfg).unwrap()).unwrap();
        let stop = root["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 2, "two different agent_ids should coexist");
        // Both rows present, agent_ids differ.
        let cmds: Vec<&str> = stop
            .iter()
            .map(|e| e["hooks"][0]["command"].as_str().unwrap())
            .collect();
        assert!(cmds.iter().any(|c| c.contains("claude-A")));
        assert!(cmds.iter().any(|c| c.contains("claude-B")));
    }

    #[test]
    fn stop_hook_json_shape_validates_required_fields() {
        // Reference-project lesson (openclaw zod-schema): every hook entry
        // must carry `type`, `command`, `timeout` — otherwise the CLI
        // silently skips the hook with no error, which would be invisible
        // in production.
        let dir = tempdir().unwrap();
        let cfg = dir.path().join("settings.local.json");
        let bin = dir.path().join("flockmux-mcp");
        install_claude_stop_hook_at(&cfg, "claude-aaa", &bin, "http://127.0.0.1:7777").unwrap();

        let root: Value = serde_json::from_slice(&fs::read(&cfg).unwrap()).unwrap();
        for entry in root["hooks"]["Stop"].as_array().unwrap() {
            assert!(entry["matcher"].is_string(), "matcher field present");
            for h in entry["hooks"].as_array().unwrap() {
                assert_eq!(h["type"], json!("command"), "every hook is type=command");
                assert!(h["command"].is_string(), "command is a string");
                assert!(h["timeout"].is_i64(), "timeout is an integer");
            }
        }
    }

    #[test]
    fn stop_hook_preserves_unrelated_top_level_keys() {
        let dir = tempdir().unwrap();
        let cfg = dir.path().join("settings.local.json");
        let original = json!({
            "permissions": { "allow": ["Bash"] },
            "userOptions": { "model": "sonnet-4-6" }
        });
        fs::write(&cfg, serde_json::to_vec_pretty(&original).unwrap()).unwrap();
        let bin = dir.path().join("flockmux-mcp");
        install_claude_stop_hook_at(&cfg, "claude-keep", &bin, "http://127.0.0.1:7777").unwrap();
        let after: Value = serde_json::from_slice(&fs::read(&cfg).unwrap()).unwrap();
        // Unrelated fields must survive.
        assert_eq!(after["permissions"]["allow"], json!(["Bash"]));
        assert_eq!(after["userOptions"]["model"], json!("sonnet-4-6"));
        // Wake hook still got added.
        assert!(after["hooks"]["Stop"].is_array());
    }
}
