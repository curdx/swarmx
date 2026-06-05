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
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

fn home_path() -> Option<PathBuf> {
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

fn lock_config_patch() -> std::sync::MutexGuard<'static, ()> {
    CONFIG_PATCH_LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Temp sibling for an atomic write, unique per process-and-call so concurrent
/// writers never share it (the old fixed `.flockmux-tmp` suffix raced under
/// parallel spawn). Stays in `target`'s dir so the final `rename` is one-fs.
fn unique_tmp_path(target: &Path) -> PathBuf {
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

fn write_json_atomic(target: &Path, root: &Value) -> Result<()> {
    let tmp = unique_tmp_path(target);
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
    let _guard = lock_config_patch();
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
    let _guard = lock_config_patch();
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
    let _guard = lock_config_patch();
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
    let tmp = unique_tmp_path(cfg);
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
    let _guard = lock_config_patch();
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
    let _guard = lock_config_patch();
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

    let tmp = unique_tmp_path(&cfg);
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

/// Per-agent `CODEX_HOME` directory. spawn.rs points the codex worker at this
/// via the `CODEX_HOME` env so it loads an ISOLATED config instead of the
/// user's global `~/.codex` — which carries the user's personal MCP servers
/// (chrome-devtools, pencil, …). Those heavy/interactive servers stall a
/// headless worker at startup ("Starting MCP servers (n/4)… Reconnecting…").
/// Mirrors claude's `--strict-mcp-config` isolation.
pub fn codex_per_agent_home_path(agent_id: &str) -> Option<PathBuf> {
    home_path().map(|h| h.join(".flockmux").join("codex-home").join(agent_id))
}

/// Remove every `[mcp_servers...]` AND `[projects...]` section from a codex
/// `config.toml`, keeping the preamble and all OTHER sections (model /
/// model_providers / endpoint settings the worker still needs) intact.
///
/// - mcp_servers: the user's personal servers stall a headless worker.
/// - projects: these are per-dir trust entries. A worker only needs its OWN
///   workspace trusted (re-appended by the caller). Stripping them also avoids
///   a `duplicate key` crash when the workspace was already trusted globally
///   (run_patches step 1) in the config we copy from.
///
/// String-based so the user's formatting and other config survive verbatim.
fn strip_codex_mcp_sections(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut skipping = false;
    for line in text.split_inclusive('\n') {
        let t = line.trim_end_matches(['\n', '\r']).trim();
        if t.starts_with('[') && t.ends_with(']') {
            // New section header: skip mcp_servers + projects tables.
            skipping = t.starts_with("[mcp_servers]")
                || t.starts_with("[mcp_servers.")
                || t.starts_with("[projects]")
                || t.starts_with("[projects.");
        }
        if !skipping {
            out.push_str(line);
        }
    }
    out
}

/// Write the per-agent `CODEX_HOME`: an isolated `config.toml` (the user's
/// config minus their `mcp_servers`, plus ONLY flockmux-swarm + this
/// workspace's trust), a symlink to the shared `auth.json` (so token refreshes
/// stay shared), and a copy of the already-dismissed `version.json`. Idempotent.
pub fn write_codex_per_agent_home(agent_id: &str, workspace: &Path, mcp_bin: &Path) -> Result<()> {
    let home = codex_per_agent_home_path(agent_id)
        .context("home not found; cannot write per-agent codex home")?;
    fs::create_dir_all(&home).with_context(|| format!("create {}", home.display()))?;

    let user_codex = home_path().map(|h| h.join(".codex"));
    let user_cfg_text = match user_codex.as_ref().map(|d| d.join("config.toml")) {
        Some(p) if p.is_file() => fs::read_to_string(&p).unwrap_or_default(),
        _ => String::new(),
    };

    // Base = user's config minus their MCP servers; then append ours + trust.
    let mut cfg = strip_codex_mcp_sections(&user_cfg_text).trim_end().to_string();
    if !cfg.is_empty() {
        cfg.push_str("\n\n");
    }
    cfg.push_str(&render_codex_mcp_section(mcp_bin));
    cfg.push('\n');
    cfg.push_str(&format!(
        "[projects.\"{}\"]\ntrust_level = \"trusted\"\n",
        workspace.to_string_lossy()
    ));

    let cfg_path = home.join("config.toml");
    let tmp = cfg_path.with_extension("toml.flockmux-tmp");
    {
        let mut f = fs::File::create(&tmp).with_context(|| format!("create {}", tmp.display()))?;
        f.write_all(cfg.as_bytes())?;
        f.sync_all().ok();
    }
    fs::rename(&tmp, &cfg_path).with_context(|| format!("rename to {}", cfg_path.display()))?;

    // Carry over auth (symlink → shared token) + dismissed-update marker (copy).
    if let Some(dir) = user_codex {
        let src_auth = dir.join("auth.json");
        if src_auth.is_file() {
            let dst_auth = home.join("auth.json");
            let _ = fs::remove_file(&dst_auth);
            #[cfg(unix)]
            {
                let _ = std::os::unix::fs::symlink(&src_auth, &dst_auth);
            }
            #[cfg(not(unix))]
            {
                let _ = fs::copy(&src_auth, &dst_auth);
            }
        }
        let src_ver = dir.join("version.json");
        if src_ver.is_file() {
            let _ = fs::copy(&src_ver, home.join("version.json"));
        }
    }
    Ok(())
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
// The hook command line is identical across every spawn:
//
//   <mcp_bin> wake-check --server <server_url>
//
// We deliberately do NOT embed agent_id here. Codex 0.130+ keys hook
// trust by config hash (incl. command string); a per-spawn agent_id in
// the command would make every new agent count as a "new or changed"
// hook and re-prompt /hooks. Instead wake_check reads agent_id from the
// `cwd` field of the stdin JSON the CLI feeds it — flockmux workspaces
// are always created at `<root>/<agent_id>`, so the basename IS the
// agent_id (see `flockmux_mcp::wake_check::agent_id_from_stdin_cwd`).
//
// `mcp_bin` is an absolute path (PreSpawnCtx already resolves it), so
// the hook is immune to PATH drift between user shell and CLI subprocess.

// NOTE: the Stop-hook `timeout` value + its unit (claude=ms, codex=s) is no
// longer a Rust constant — it comes from each plugin's `stop_hook_timeout`
// manifest field (claude.toml=10000, codex.toml=10), so a new CLI declares
// its own value in its own native unit instead of inheriting a wrong constant.

fn render_wake_command(mcp_bin: &Path, server_url: &str) -> String {
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
    mcp_bin: &Path,
    server_url: &str,
    timeout: i64,
) -> Result<()> {
    let cfg_dir = workspace.join(".claude");
    fs::create_dir_all(&cfg_dir)
        .with_context(|| format!("mkdir {}", cfg_dir.display()))?;
    // Keep our managed file out of git's "dirty" accounting (sidebar dot,
    // merge-to-main base check) without touching the user's tracked .gitignore.
    crate::worktree::ignore_managed_artifacts(workspace);
    let cfg = cfg_dir.join("settings.local.json");
    install_claude_stop_hook_at(&cfg, mcp_bin, server_url, timeout)
}

fn install_claude_stop_hook_at(
    cfg: &Path,
    mcp_bin: &Path,
    server_url: &str,
    timeout: i64,
) -> Result<()> {
    let mut root: Value = if cfg.is_file() {
        let bytes = fs::read(cfg).with_context(|| format!("read {}", cfg.display()))?;
        serde_json::from_slice(&bytes)
            .with_context(|| format!("parse {}", cfg.display()))?
    } else {
        json!({})
    };

    let command = render_wake_command(mcp_bin, server_url);
    merge_stop_hook(&mut root, &command, timeout);
    write_json_atomic(cfg, &root)
}

/// Write a workspace-local `.codex/hooks.json` carrying a Stop hook that
/// calls `flockmux-mcp wake-check`. Same structural shape as claude's
/// settings.local.json but `timeout` is in **seconds**, not ms.
pub fn install_codex_stop_hook(
    workspace: &Path,
    mcp_bin: &Path,
    server_url: &str,
    timeout: i64,
) -> Result<()> {
    let cfg_dir = workspace.join(".codex");
    fs::create_dir_all(&cfg_dir)
        .with_context(|| format!("mkdir {}", cfg_dir.display()))?;
    // Keep our managed file out of git's "dirty" accounting (see claude variant).
    crate::worktree::ignore_managed_artifacts(workspace);
    let cfg = cfg_dir.join("hooks.json");
    install_codex_stop_hook_at(&cfg, mcp_bin, server_url, timeout)
}

fn install_codex_stop_hook_at(
    cfg: &Path,
    mcp_bin: &Path,
    server_url: &str,
    timeout: i64,
) -> Result<()> {
    let mut root: Value = if cfg.is_file() {
        let bytes = fs::read(cfg).with_context(|| format!("read {}", cfg.display()))?;
        serde_json::from_slice(&bytes)
            .with_context(|| format!("parse {}", cfg.display()))?
    } else {
        json!({})
    };

    let command = render_wake_command(mcp_bin, server_url);
    merge_stop_hook(&mut root, &command, timeout);
    write_json_atomic(cfg, &root)
}

/// Dispatch into per-CLI patch sequences. Each CLI has its own readable
/// top-to-bottom block listing every patch that applies to it; the host
/// never interleaves them. Failures are logged at `warn!` but never
/// propagated — at worst the user sees the prompt we tried to suppress
/// (or the agent is missing the swarm tool block), which is annoying but
/// not fatal.
///
/// Apply all pre-spawn host patches for `plugin`. Each capability is gated on
/// its `auto_*` flag, and the *writer* is chosen by the plugin's declared
/// **format** enum (`trust_format` / `mcp_format` / `stop_hook_format`) — NOT
/// by `plugin.id`. So adding a CLI that reuses an existing config format is
/// pure config (`cli-plugins/<id>.toml`); a CLI with a genuinely new format
/// adds one enum variant (in `plugins.rs`) + one writer below + one match arm.
pub fn run_patches(
    plugin: &crate::plugins::CliPlugin,
    workspace: &Path,
    ctx: &PreSpawnCtx,
) {
    use crate::plugins::{McpFormat, StopHookFormat, TrustFormat};

    // 1. Trust: pre-accept the "do you trust this folder?" gate.
    if plugin.auto_trust_workspace {
        let res = match plugin.trust_format {
            TrustFormat::ClaudeJson => mark_claude_workspace_trusted(workspace),
            TrustFormat::CodexToml => mark_codex_workspace_trusted(workspace),
            TrustFormat::None => {
                tracing::warn!(cli = %plugin.id, "auto_trust_workspace set but trust_format = none; skipping");
                Ok(())
            }
        };
        if let Err(err) = res {
            tracing::warn!(?err, cli = %plugin.id, "auto-trust patch failed");
        }
    }

    // 2. Suppress the "update available" prompt. Codex-only quirk today
    //    (claude has no equivalent), so this stays a simple flag gate rather
    //    than a format dispatch.
    if plugin.auto_dismiss_update {
        if let Err(err) = mark_codex_update_dismissed() {
            tracing::warn!(?err, cli = %plugin.id, "auto-dismiss-update patch failed");
        }
    }

    // 3. Register the flockmux-swarm MCP server so the agent gets swarm_* tools.
    if plugin.auto_inject_mcp {
        match plugin.mcp_format {
            McpFormat::ClaudeLocalScope => {
                // Local-scope entry in ~/.claude.json ...
                if let Err(err) =
                    mark_claude_mcp_local(workspace, &ctx.agent_id, &ctx.mcp_bin, &ctx.server_url)
                {
                    tracing::warn!(?err, "claude: mcp-inject patch failed");
                }
                // ... plus a per-agent file that spawn.rs passes as
                // `--mcp-config <file> --strict-mcp-config` to dodge the
                // shared-cwd ~/.claude.json mcpServers collision (M6b). Run
                // independently of the entry above (matches prior behavior).
                if let Err(err) =
                    write_claude_per_agent_mcp_config(&ctx.agent_id, &ctx.mcp_bin, &ctx.server_url)
                {
                    tracing::warn!(?err, "claude: per-agent mcp file write failed");
                }
            }
            McpFormat::CodexGlobalToml => {
                // Preferred: a per-agent CODEX_HOME with ONLY flockmux-swarm, so
                // the worker doesn't inherit the user's personal ~/.codex MCP
                // servers (which stall a headless worker at startup). spawn.rs
                // sets CODEX_HOME when the per-agent config.toml exists.
                if let Err(err) =
                    write_codex_per_agent_home(&ctx.agent_id, workspace, &ctx.mcp_bin)
                {
                    tracing::warn!(?err, "codex: per-agent CODEX_HOME write failed");
                }
                // Fallback: also ensure the global block, so a worker that (for
                // any reason) falls back to ~/.codex still gets swarm_* tools.
                if let Err(err) = ensure_codex_mcp_global(&ctx.mcp_bin) {
                    tracing::warn!(?err, "codex: mcp-inject patch failed");
                }
            }
            McpFormat::None => {
                tracing::warn!(cli = %plugin.id, "auto_inject_mcp set but mcp_format = none; agent will have NO swarm_* tools (cannot coordinate)");
            }
        }
    }

    // 4. Install the wake Stop-hook (timeout unit differs per writer:
    //    claude = ms, codex = seconds).
    if plugin.auto_inject_stop_hook {
        let res = match plugin.stop_hook_format {
            StopHookFormat::ClaudeSettingsLocal => {
                install_claude_stop_hook(workspace, &ctx.mcp_bin, &ctx.server_url, plugin.stop_hook_timeout)
            }
            StopHookFormat::CodexHooksJson => {
                install_codex_stop_hook(workspace, &ctx.mcp_bin, &ctx.server_url, plugin.stop_hook_timeout)
            }
            StopHookFormat::None => {
                tracing::warn!(cli = %plugin.id, "auto_inject_stop_hook set but stop_hook_format = none; agent will never be re-woken");
                Ok(())
            }
        };
        if let Err(err) = res {
            tracing::warn!(?err, cli = %plugin.id, "stop-hook install failed");
        }
    }
}

/// Per-agent claude MCP config file. Written under `~/.flockmux/mcp/` keyed
/// by `agent_id`, intended to be passed to `claude --mcp-config <path>
/// --strict-mcp-config` so claude completely ignores the shared
/// `~/.claude.json` config and uses ONLY this file.
///
/// Why this exists: `~/.claude.json` keys MCP servers by project (cwd) path.
/// In shared_workspace spells (M6a fullstack-feature) all 3 agents have the
/// same cwd, so each `mark_claude_mcp_local()` overwrites the previous
/// agent's entry — when claude lazy-launches its MCP server the file now
/// holds the LAST spawn's identity, leaving the other agents impersonating
/// each other. Confirmed in M6b run #4: FE's MCP server reported its id
/// as the test agent's id, FE concluded "there's a separate FE agent" and
/// stopped to ask for clarification, never wrote code. This per-agent
/// override sidesteps the collision entirely.
pub fn write_claude_per_agent_mcp_config(
    agent_id: &str,
    mcp_bin: &Path,
    server_url: &str,
) -> Result<PathBuf> {
    let path = claude_per_agent_mcp_config_path(agent_id)
        .context("home not found; cannot write per-agent claude MCP config")?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let body = json!({
        "mcpServers": {
            "flockmux-swarm": {
                "type": "stdio",
                "command": mcp_bin.to_string_lossy(),
                "args": ["--agent-id", agent_id],
                "env": {
                    "FLOCKMUX_AGENT_ID": agent_id,
                    "FLOCKMUX_SERVER_URL": server_url,
                }
            }
        }
    });
    write_json_atomic(&path, &body)?;
    Ok(path)
}

/// Computes the path `write_claude_per_agent_mcp_config()` writes to without
/// touching disk. `spawn.rs` uses this to find the `--mcp-config` value at
/// launch time. Returns `None` if `$HOME` is not set (then claude has no
/// home anyway and would have failed earlier).
pub fn claude_per_agent_mcp_config_path(agent_id: &str) -> Option<PathBuf> {
    home_path().map(|h| h.join(".flockmux").join("mcp").join(format!("{agent_id}.json")))
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
    fn strip_codex_mcp_sections_drops_mcp_and_projects_keeps_provider() {
        let cfg = "\
model = \"gpt-5.5\"\n\
model_provider = \"custom\"\n\
\n\
[model_providers.custom]\n\
base_url = \"https://nowcoding.ai/v1\"\n\
\n\
[mcp_servers.chrome-devtools]\n\
command = \"npx\"\n\
args = [\"chrome-devtools-mcp@latest\"]\n\
\n\
[mcp_servers.flockmux-swarm]\n\
command = \"/old/path\"\n\
\n\
[projects.\"/some/dir\"]\n\
trust_level = \"trusted\"\n";
        let out = strip_codex_mcp_sections(cfg);
        // model + custom provider survive (worker still reaches the model)
        assert!(out.contains("model = \"gpt-5.5\""));
        assert!(out.contains("[model_providers.custom]"));
        assert!(out.contains("nowcoding.ai"));
        // ALL mcp_servers (incl. stale flockmux-swarm) AND all projects gone —
        // the latter prevents the duplicate-key crash on re-append.
        assert!(!out.contains("[mcp_servers"));
        assert!(!out.contains("chrome-devtools"));
        assert!(!out.contains("/old/path"));
        assert!(!out.contains("[projects"));
        assert!(!out.contains("/some/dir"));
    }

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
        let tmp = unique_tmp_path(cfg);
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
        install_claude_stop_hook_at(&cfg, &bin, "http://127.0.0.1:7777", 10_000).unwrap();

        let root: Value = serde_json::from_slice(&fs::read(&cfg).unwrap()).unwrap();
        let stop = root["hooks"]["Stop"].as_array().expect("hooks.Stop is array");
        assert_eq!(stop.len(), 1);
        let inner = stop[0]["hooks"][0].clone();
        assert_eq!(inner["type"], json!("command"));
        assert_eq!(inner["timeout"], json!(10_000), "claude timeout in ms");
        let cmd = inner["command"].as_str().unwrap();
        assert!(cmd.contains("wake-check"), "got: {cmd}");
        assert!(cmd.contains("--server http://127.0.0.1:7777"), "got: {cmd}");
        assert!(cmd.contains(bin.to_string_lossy().as_ref()), "absolute bin path: {cmd}");
        // Trust-stability invariant: command must NOT carry per-spawn identity,
        // otherwise codex 0.130+ would re-prompt /hooks on every new agent.
        assert!(!cmd.contains("--agent-id"), "agent_id must NOT be in command: {cmd}");
    }

    #[test]
    fn codex_stop_hook_creates_hooks_json() {
        let dir = tempdir().unwrap();
        let cfg = dir.path().join("hooks.json");
        let bin = dir.path().join("flockmux-mcp");
        install_codex_stop_hook_at(&cfg, &bin, "http://127.0.0.1:7777", 10).unwrap();

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
        assert!(cmd.contains("wake-check"), "got: {cmd}");
        // See claude_stop_hook_creates_settings_local for the why.
        assert!(!cmd.contains("--agent-id"), "agent_id must NOT be in command: {cmd}");
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
        install_claude_stop_hook_at(&cfg, &bin, "http://127.0.0.1:7777", 10_000).unwrap();

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

        install_codex_stop_hook_at(&cfg, &bin, "http://127.0.0.1:7777", 10).unwrap();
        install_codex_stop_hook_at(&cfg, &bin, "http://127.0.0.1:7777", 10).unwrap();
        install_codex_stop_hook_at(&cfg, &bin, "http://127.0.0.1:7777", 10).unwrap();

        let root: Value = serde_json::from_slice(&fs::read(&cfg).unwrap()).unwrap();
        let stop = root["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 1, "repeated install must not accumulate entries");
    }

    /// Trust-persistence guard: every spawn must produce the EXACT same hook
    /// command, otherwise codex 0.130+ would re-prompt /hooks each time.
    /// Multiple installs (even logically representing different agents) must
    /// collapse to a single Stop hook row identical to the first install.
    #[test]
    fn claude_stop_hook_command_is_stable_across_installs() {
        let dir = tempdir().unwrap();
        let cfg = dir.path().join("settings.local.json");
        let bin = dir.path().join("flockmux-mcp");

        install_claude_stop_hook_at(&cfg, &bin, "http://127.0.0.1:7777", 10_000).unwrap();
        let first: Value = serde_json::from_slice(&fs::read(&cfg).unwrap()).unwrap();
        let first_cmd = first["hooks"]["Stop"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .to_string();

        install_claude_stop_hook_at(&cfg, &bin, "http://127.0.0.1:7777", 10_000).unwrap();
        let second: Value = serde_json::from_slice(&fs::read(&cfg).unwrap()).unwrap();
        let second_stop = second["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(second_stop.len(), 1, "second install must dedupe to 1");
        assert_eq!(
            second_stop[0]["hooks"][0]["command"].as_str().unwrap(),
            first_cmd,
            "command string must be byte-identical to keep trust hash stable",
        );
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
        install_claude_stop_hook_at(&cfg, &bin, "http://127.0.0.1:7777", 10_000).unwrap();

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
        install_claude_stop_hook_at(&cfg, &bin, "http://127.0.0.1:7777", 10_000).unwrap();
        let after: Value = serde_json::from_slice(&fs::read(&cfg).unwrap()).unwrap();
        // Unrelated fields must survive.
        assert_eq!(after["permissions"]["allow"], json!(["Bash"]));
        assert_eq!(after["userOptions"]["model"], json!("sonnet-4-6"));
        // Wake hook still got added.
        assert!(after["hooks"]["Stop"].is_array());
    }
}
