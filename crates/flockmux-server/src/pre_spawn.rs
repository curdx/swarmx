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

/// Run all configured pre-spawn patches for `plugin`. Failures are logged at
/// `warn!` but never propagated — at worst the user sees the prompt we tried
/// to suppress (or the agent is missing the swarm tool block), which is
/// annoying but not fatal.
pub fn run_patches(
    plugin: &crate::plugins::CliPlugin,
    workspace: &Path,
    ctx: &PreSpawnCtx,
) {
    if plugin.auto_trust_workspace {
        match plugin.id.as_str() {
            "claude" => {
                if let Err(err) = mark_claude_workspace_trusted(workspace) {
                    tracing::warn!(?err, cli = %plugin.id, "auto-trust patch failed");
                }
            }
            "codex" => {
                if let Err(err) = mark_codex_workspace_trusted(workspace) {
                    tracing::warn!(?err, cli = %plugin.id, "auto-trust patch failed");
                }
            }
            other => {
                tracing::debug!(
                    cli = %other,
                    "auto_trust_workspace set but no handler — ignored"
                );
            }
        }
    }
    if plugin.auto_dismiss_update {
        match plugin.id.as_str() {
            "codex" => {
                if let Err(err) = mark_codex_update_dismissed() {
                    tracing::warn!(?err, cli = %plugin.id, "auto-dismiss-update patch failed");
                }
            }
            other => {
                tracing::debug!(
                    cli = %other,
                    "auto_dismiss_update set but no handler — ignored"
                );
            }
        }
    }
    if plugin.auto_inject_mcp {
        match plugin.id.as_str() {
            "claude" => {
                if let Err(err) = mark_claude_mcp_local(
                    workspace,
                    &ctx.agent_id,
                    &ctx.mcp_bin,
                    &ctx.server_url,
                ) {
                    tracing::warn!(?err, cli = %plugin.id, "mcp-inject patch failed");
                }
            }
            "codex" => {
                if let Err(err) = ensure_codex_mcp_global(&ctx.mcp_bin) {
                    tracing::warn!(?err, cli = %plugin.id, "mcp-inject patch failed");
                }
            }
            other => {
                tracing::debug!(
                    cli = %other,
                    "auto_inject_mcp set but no handler — ignored"
                );
            }
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
}
