//! Patch the per-workspace `hasTrustDialogAccepted` flag in `~/.claude.json`
//! so spawned `claude` instances skip the "Do you trust this folder?" prompt.
//!
//! Safe because flockmux only ever asks about workspaces it created itself
//! under `~/.flockmux/workspaces/`. Atomic via write-temp + rename.

use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

fn claude_config_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".claude.json"))
}

/// Mark `workspace` as trusted in `~/.claude.json`. No-op if the file
/// doesn't exist (claude hasn't run yet) or already has the flag set.
pub fn mark_claude_workspace_trusted(workspace: &Path) -> Result<()> {
    let cfg = match claude_config_path() {
        Some(p) if p.is_file() => p,
        _ => return Ok(()),
    };

    let bytes = fs::read(&cfg).with_context(|| format!("read {}", cfg.display()))?;
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

    let entry = projects
        .entry(key.clone())
        .or_insert_with(|| json!({}));
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

    let tmp = cfg.with_extension("json.flockmux-tmp");
    {
        let mut f = fs::File::create(&tmp)
            .with_context(|| format!("create {}", tmp.display()))?;
        f.write_all(&serde_json::to_vec_pretty(&root)?)?;
        f.sync_all().ok();
    }
    fs::rename(&tmp, &cfg).with_context(|| format!("rename to {}", cfg.display()))?;
    Ok(())
}
