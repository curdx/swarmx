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

/// Run all configured pre-spawn patches for `plugin`. Failures are logged at
/// `warn!` but never propagated — at worst the user sees the prompt we tried
/// to suppress, which is annoying but not fatal.
pub fn run_patches(plugin: &crate::plugins::CliPlugin, workspace: &Path) {
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
}
