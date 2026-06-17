//! Low-level primitives shared by more than one [`super::CliAdapter`].
//!
//! Everything here is CLI-agnostic plumbing: atomic file writes, the
//! read-modify-write lock that serializes patches to the user's shared CLI
//! config files, and the wake Stop-hook building blocks that claude and codex
//! both speak. A primitive only lands here if at least two adapters need it; a
//! detail that belongs to a single CLI lives in that CLI's module instead, so
//! the adapters never reach across into one another.

use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

pub(super) fn home_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Serializes read-modify-write of the *shared* CLI config files
/// (`~/.claude.json`, `~/.codex/config.toml`, `~/.codex/version.json`). Each
/// spawn patches these; run in parallel they otherwise (a) collide on the temp
/// sibling -> `rename ... No such file or directory`, and (b) lost-update each
/// other (both read v0, both write v0+self, last writer wins). Held only across
/// a few ms of local file IO, never across `.await`. Poison-tolerant so one
/// panicked patch can't wedge every future spawn.
static CONFIG_PATCH_LOCK: Mutex<()> = Mutex::new(());

pub(super) fn lock_config_patch() -> std::sync::MutexGuard<'static, ()> {
    CONFIG_PATCH_LOCK
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

/// Temp sibling for an atomic write, unique per process-and-call so concurrent
/// writers never share it (the old fixed `.flockmux-tmp` suffix raced under
/// parallel spawn). Stays in `target`'s dir so the final `rename` is one-fs.
pub(super) fn unique_tmp_path(target: &Path) -> PathBuf {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    // Preserve the original extension in the sibling name purely for legible
    // debris (`config.toml` -> `config.toml.flockmux-tmp.<pid>.<n>`).
    let suffix = match target.extension().and_then(|s| s.to_str()) {
        Some(ext) => format!("{ext}.flockmux-tmp.{pid}.{n}"),
        None => format!("flockmux-tmp.{pid}.{n}"),
    };
    target.with_extension(suffix)
}

pub(super) fn write_json_atomic(target: &Path, root: &Value) -> Result<()> {
    let tmp = unique_tmp_path(target);
    {
        let mut f = fs::File::create(&tmp).with_context(|| format!("create {}", tmp.display()))?;
        f.write_all(&serde_json::to_vec_pretty(root)?)?;
        f.sync_all().ok();
    }
    fs::rename(&tmp, target).with_context(|| format!("rename to {}", target.display()))?;
    Ok(())
}

// ── Stop-hook building blocks (M5b wake) ─────────────────────────────────────
//
// Each CLI ships a Stop event hook system; we use it to push a synthetic
// continuation prompt whenever the agent has unread swarm mail. Claude and codex
// agree on the wire protocol:
//   stdout JSON `{}`                                  → no-op
//   stdout JSON `{"decision":"block", "reason":...}`  → continue another turn
// but they DIFFER on the config schema's `timeout` unit:
//   - Claude  (~/.claude/settings.local.json): timeout in **milliseconds**.
//   - Codex   (~/.codex/hooks.json):           timeout in **seconds**.
// Mixing them silently truncates / explodes the cap, so each adapter passes its
// own `stop_hook_timeout` (from the manifest, in its native unit) into
// `merge_stop_hook`. The command string itself is identical across every spawn:
//
//   <mcp_bin> wake-check --server <server_url>
//
// We deliberately do NOT embed agent_id here. Codex 0.130+ keys hook trust by
// config hash (incl. command string); a per-spawn agent_id in the command would
// make every new agent count as a "new or changed" hook and re-prompt /hooks.
// Instead wake_check reads agent_id from the `cwd` field of the stdin JSON the
// CLI feeds it — flockmux workspaces are always created at `<root>/<agent_id>`,
// so the basename IS the agent_id.
//
// `mcp_bin` is an absolute path (PreSpawnCtx already resolves it), so the hook is
// immune to PATH drift between user shell and CLI subprocess. These two helpers
// live here (not in a single adapter) precisely because both claude and codex
// emit the same hook shape into different files.

pub(super) fn render_wake_command(mcp_bin: &Path, server_url: &str) -> String {
    format!(
        "{} wake-check --server {}",
        // Note: we don't shell-quote because spawn pipelines invoke the
        // string via the CLI's shell-out path (claude/codex both use
        // sh -c). server_url is an http/https URL — no shell metachars
        // in practice.
        mcp_bin.to_string_lossy(),
        server_url,
    )
}

/// Merge a flockmux wake-check entry into `root.hooks.Stop`. Idempotent on
/// the command string: re-installing collapses to one row. Since the
/// command no longer encodes agent_id, ALL spawns share the same hash,
/// which is exactly what we want for trust persistence.
pub(super) fn merge_stop_hook(root: &mut Value, command: &str, timeout: i64) {
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
                inner
                    .iter()
                    .any(|h| h.get("command").and_then(|v| v.as_str()) == Some(command))
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
