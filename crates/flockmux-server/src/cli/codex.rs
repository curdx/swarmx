//! Codex CLI adapter — everything codex needs that no other CLI does, in one
//! place. Pre-spawn patches target `~/.codex/config.toml` (trust + global MCP),
//! `~/.codex/version.json` (dismiss the update nag), a per-agent `CODEX_HOME`
//! (a config that INHERITS the user's MCP servers plus a per-agent
//! flockmux-swarm — codex's 10s startup_timeout skips any that stall), and
//! `<ws>/.codex/hooks.json` (wake Stop-hook, seconds timeout). At spawn it
//! injects `--dangerously-bypass-hook-trust` (when the binary supports it) and
//! points the child at its per-agent `CODEX_HOME`.
//!
//! Selected by [`super::adapter_for`] for any plugin whose config formats are
//! codex-shaped (`mcp_format = "codex-global-toml"`); the literal id is never
//! matched.

use super::shared::{
    home_path, lock_config_patch, merge_stop_hook, render_wake_command, unique_tmp_path,
    write_json_atomic,
};
use super::{CliAdapter, PreSpawnCtx};
use crate::plugins::CliPlugin;
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Zero-sized behavior object for the Codex family.
pub struct CodexAdapter;

impl CliAdapter for CodexAdapter {
    fn name(&self) -> &'static str {
        "codex"
    }

    fn pre_spawn(&self, plugin: &CliPlugin, workspace: &Path, ctx: &PreSpawnCtx) {
        // 1. Trust: append `[projects."<ws>"] trust_level = "trusted"`.
        if plugin.auto_trust_workspace {
            if let Err(err) = mark_workspace_trusted(workspace) {
                tracing::warn!(?err, cli = %plugin.id, "codex auto-trust patch failed");
            }
        }
        // 2. Suppress codex's "update available" prompt (blocks the headless PTY).
        if plugin.auto_dismiss_update {
            if let Err(err) = mark_update_dismissed() {
                tracing::warn!(?err, cli = %plugin.id, "codex auto-dismiss-update patch failed");
            }
        }
        // 3. MCP: preferred path is a per-agent CODEX_HOME that INHERITS the
        //    user's ~/.codex MCP servers (context7, …) plus a per-agent
        //    flockmux-swarm; codex's default 10s startup_timeout_sec skips any
        //    inherited server that stalls, so the worker can't hang on a heavy
        //    one. `contribute_env` sets CODEX_HOME when the per-agent config
        //    exists. The global block is a fallback for a worker that (for any
        //    reason) falls back to ~/.codex.
        if plugin.auto_inject_mcp {
            if let Err(err) = write_codex_per_agent_home(&ctx.agent_id, workspace, &ctx.mcp_bin) {
                tracing::warn!(?err, "codex: per-agent CODEX_HOME write failed");
            }
            if let Err(err) = ensure_codex_mcp_global(&ctx.mcp_bin) {
                tracing::warn!(?err, "codex: mcp-inject patch failed");
            }
        }
        // 4. Wake: workspace-local Stop hook (timeout in seconds).
        if plugin.auto_inject_stop_hook {
            if let Err(err) = install_stop_hook(
                workspace,
                &ctx.mcp_bin,
                &ctx.server_url,
                plugin.stop_hook_timeout,
            ) {
                tracing::warn!(?err, cli = %plugin.id, "codex stop-hook install failed");
            }
        }
    }

    fn contribute_argv(&self, plugin: &CliPlugin, agent_id: &str, argv: &mut Vec<String>) {
        // codex 0.130 gates non-managed Stop hooks behind an in-app /hooks
        // trust-review prompt — workspace-local hooks.json gets installed but
        // never executes until the user manually approves it. PR #21768 ships
        // `--dangerously-bypass-hook-trust` to skip the review for automation
        // hosts like us. The flag isn't in 0.130 yet (codex aborts spawn on
        // unknown argv), so probe `<binary> --help` once per process and only
        // inject the flag if it's already supported. Net effect:
        //   - codex 0.130: probe -> false, argv unchanged, hooks.json stays
        //     dormant (known constraint, documented in auto-memory).
        //   - codex >=0.131 (future): probe -> true, flag injected, our
        //     existing hooks.json install becomes immediately effective with
        //     zero config change on flockmux's side.
        if super::binary_supports_flag(&plugin.binary, "--dangerously-bypass-hook-trust") {
            argv.push("--dangerously-bypass-hook-trust".into());
            tracing::info!(
                agent = %agent_id,
                "--dangerously-bypass-hook-trust supported; injecting"
            );
        }
    }

    fn contribute_env(
        &self,
        plugin: &CliPlugin,
        agent_id: &str,
        env: &mut HashMap<String, String>,
    ) {
        // Point the worker at its per-agent CODEX_HOME (written by pre_spawn) so
        // it loads a per-agent config: the user's ~/.codex MCP servers inherited
        // (context7, …) plus this agent's own flockmux-swarm. codex's 10s
        // startup_timeout skips any inherited server that stalls. Gated on the
        // per-agent config.toml existing; otherwise codex falls back to the
        // global ~/.codex (still has the block).
        if !plugin.auto_inject_mcp {
            return;
        }
        if let Some(home) = codex_per_agent_home_path(agent_id) {
            if home.join("config.toml").is_file() {
                env.insert("CODEX_HOME".into(), home.to_string_lossy().into_owned());
                tracing::info!(
                    agent = %agent_id,
                    codex_home = %home.display(),
                    "codex per-agent CODEX_HOME injected (isolates MCP from user's global ~/.codex)"
                );
            }
        }
    }
}

/// Mark `workspace` as trusted in `~/.codex/config.toml`. Appends a fresh
/// `[projects."<workspace>"] trust_level = "trusted"` section if missing,
/// otherwise no-op. We don't round-trip the TOML through serde because the
/// user's config almost certainly contains comments / hand-arranged sections
/// we should preserve verbatim.
fn mark_workspace_trusted(workspace: &Path) -> Result<()> {
    let cfg = match home_path().map(|h| h.join(".codex").join("config.toml")) {
        Some(p) if p.is_file() => p,
        _ => return Ok(()),
    };
    patch_codex_trust_at(&cfg, workspace)
}

fn patch_codex_trust_at(cfg: &Path, workspace: &Path) -> Result<()> {
    let _guard = lock_config_patch();
    let existing = fs::read_to_string(cfg).with_context(|| format!("read {}", cfg.display()))?;

    let key = workspace.to_string_lossy();
    // codex emits exactly this header style; matching it on its own line is
    // enough — flockmux paths never need TOML literal-key escaping.
    let header = format!("[projects.\"{key}\"]");
    let already = existing.lines().any(|line| line.trim() == header);
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
        let mut f = fs::File::create(&tmp).with_context(|| format!("create {}", tmp.display()))?;
        f.write_all(out.as_bytes())?;
        f.sync_all().ok();
    }
    fs::rename(&tmp, cfg).with_context(|| format!("rename to {}", cfg.display()))?;
    Ok(())
}

/// Set `dismissed_version = latest_version` in `~/.codex/version.json` so
/// codex won't print "Update available! Press enter to continue" — that
/// prompt blocks our headless PTY waiting on a key we have no good way to
/// deliver.
///
/// No-op if the file doesn't exist, `latest_version` is missing, or
/// `dismissed_version` already matches.
fn mark_update_dismissed() -> Result<()> {
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
fn ensure_codex_mcp_global(mcp_bin: &Path) -> Result<()> {
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
        let mut f = fs::File::create(&tmp).with_context(|| format!("create {}", tmp.display()))?;
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

/// Per-agent `CODEX_HOME` directory. `contribute_env` points the codex worker at
/// this via the `CODEX_HOME` env so it loads a PER-AGENT config (the user's
/// `~/.codex` MCP servers inherited, plus this agent's own flockmux-swarm)
/// instead of sharing the global `~/.codex` — sharing it would leak one agent's
/// swarm identity into another (the per-agent swarm entry must be unique). Also
/// read by the transcript tailer to find this worker's rollout JSONL.
pub fn codex_per_agent_home_path(agent_id: &str) -> Option<PathBuf> {
    home_path().map(|h| h.join(".flockmux").join("codex-home").join(agent_id))
}

/// Sanitize the user's `~/.codex/config.toml` for a worker's isolated
/// `CODEX_HOME`: KEEP their personal `[mcp_servers.*]` (so the worker inherits
/// context7 etc.) and all model/provider settings, but strip the two section
/// kinds that must NOT be inherited verbatim:
///
/// - `[mcp_servers.flockmux-swarm]`: re-appended per-agent with THIS agent's id
///   (never inherit a stale/foreign one — the M6b collision), and keeping it
///   would `duplicate key` crash against the re-append.
/// - `[projects...]`: per-dir trust entries. A worker only needs its OWN
///   workspace trusted (re-appended by the caller); inheriting them also
///   `duplicate key` crashes against the trust patch.
///
/// Inherited heavy servers can't hang the worker: codex's default
/// `startup_timeout_sec = 10` skips any server that doesn't come up in time, so
/// a slow/broken server degrades to "unavailable", not a stalled startup.
///
/// String-based so the user's formatting and other config survive verbatim.
fn prune_codex_config_for_inherit(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut skipping = false;
    for line in text.split_inclusive('\n') {
        let t = line.trim_end_matches(['\n', '\r']).trim();
        if t.starts_with('[') && t.ends_with(']') {
            // New section header: drop only our own swarm entry + trust tables;
            // every other [mcp_servers.*] is KEPT so the worker inherits it.
            skipping = t == "[mcp_servers.flockmux-swarm]"
                || t.starts_with("[mcp_servers.flockmux-swarm.")
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
/// config with their `mcp_servers` INHERITED — our own stale swarm entry + trust
/// tables pruned — plus this agent's flockmux-swarm + this workspace's trust), a
/// symlink to the shared `auth.json` (so token refreshes stay shared), and a
/// copy of the already-dismissed `version.json`. Idempotent.
fn write_codex_per_agent_home(agent_id: &str, workspace: &Path, mcp_bin: &Path) -> Result<()> {
    let home = codex_per_agent_home_path(agent_id)
        .context("home not found; cannot write per-agent codex home")?;
    fs::create_dir_all(&home).with_context(|| format!("create {}", home.display()))?;

    let user_codex = home_path().map(|h| h.join(".codex"));
    let user_cfg_text = match user_codex.as_ref().map(|d| d.join("config.toml")) {
        Some(p) if p.is_file() => fs::read_to_string(&p).unwrap_or_default(),
        _ => String::new(),
    };

    // Base = user's config with their MCP servers INHERITED (minus our own
    // swarm entry + trust tables); then append our per-agent swarm + trust.
    let mut cfg = prune_codex_config_for_inherit(&user_cfg_text)
        .trim_end()
        .to_string();
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

/// Write a workspace-local `.codex/hooks.json` carrying a Stop hook that
/// calls `flockmux-mcp wake-check`. Same structural shape as claude's
/// settings.local.json but `timeout` is in **seconds**, not ms.
fn install_stop_hook(
    workspace: &Path,
    mcp_bin: &Path,
    server_url: &str,
    timeout: i64,
) -> Result<()> {
    let cfg_dir = workspace.join(".codex");
    fs::create_dir_all(&cfg_dir).with_context(|| format!("mkdir {}", cfg_dir.display()))?;
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
        serde_json::from_slice(&bytes).with_context(|| format!("parse {}", cfg.display()))?
    } else {
        json!({})
    };

    let command = render_wake_command(mcp_bin, server_url);
    merge_stop_hook(&mut root, &command, timeout);
    write_json_atomic(cfg, &root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;

    #[test]
    fn prune_keeps_user_mcp_inherits_but_drops_swarm_and_projects() {
        let cfg = "\
model = \"gpt-5.5\"\n\
model_provider = \"custom\"\n\
\n\
[model_providers.custom]\n\
base_url = \"https://nowcoding.ai/v1\"\n\
\n\
[mcp_servers.context7]\n\
command = \"npx\"\n\
args = [\"-y\", \"@upstash/context7-mcp\"]\n\
\n\
[mcp_servers.flockmux-swarm]\n\
command = \"/old/path\"\n\
\n\
[projects.\"/some/dir\"]\n\
trust_level = \"trusted\"\n";
        let out = prune_codex_config_for_inherit(cfg);
        // model + custom provider survive (worker still reaches the model)
        assert!(out.contains("model = \"gpt-5.5\""));
        assert!(out.contains("[model_providers.custom]"));
        assert!(out.contains("nowcoding.ai"));
        // The user's OWN mcp server is INHERITED — the worker can now use it.
        assert!(out.contains("[mcp_servers.context7]"));
        assert!(out.contains("@upstash/context7-mcp"));
        // Our own stale swarm entry AND all projects are dropped — the latter
        // prevents the duplicate-key crash on re-append.
        assert!(!out.contains("[mcp_servers.flockmux-swarm]"));
        assert!(!out.contains("/old/path"));
        assert!(!out.contains("[projects"));
        assert!(!out.contains("/some/dir"));
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
        assert!(
            after.contains("# user comment that must survive"),
            "comments preserved"
        );
        assert!(
            after.contains("[projects.\"/some/other\"]"),
            "existing section kept"
        );
        let expected_header = format!("[projects.\"{}\"]", ws.to_string_lossy());
        assert!(after.contains(&expected_header), "new header appended");
        assert!(
            after
                .lines()
                .rev()
                .take(3)
                .any(|l| l == "trust_level = \"trusted\""),
            "trust_level set in new section",
        );
    }

    #[test]
    fn codex_trust_noop_when_section_already_present() {
        let dir = tempdir().unwrap();
        let cfg = dir.path().join("config.toml");
        let ws = dir.path().join("ws-Y");
        let header = format!("[projects.\"{}\"]", ws.to_string_lossy());
        let original = format!("model = \"gpt-5.5\"\n\n{header}\ntrust_level = \"trusted\"\n");
        fs::write(&cfg, &original).unwrap();
        let before = fs::read(&cfg).unwrap();

        patch_codex_trust_at(&cfg, &ws).unwrap();

        assert_eq!(
            fs::read(&cfg).unwrap(),
            before,
            "no-op when already present"
        );
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
        let (start, end) = find_section_range(body, "[mcp_servers.flockmux-swarm]").unwrap();
        let section = &body[start..end];
        assert!(section.contains("command = \"foo\""));
        assert!(!section.contains("[c]"), "section bled past next header");
    }

    #[test]
    fn find_section_range_matches_until_eof_when_last_section() {
        let body = "[mcp_servers.flockmux-swarm]\ncommand = \"foo\"\n";
        let (start, end) = find_section_range(body, "[mcp_servers.flockmux-swarm]").unwrap();
        assert_eq!(end, body.len());
        let section = &body[start..end];
        assert!(section.contains("command = \"foo\""));
    }

    // ── M5b Stop-hook install patches ─────────────────────────────────────

    #[test]
    fn codex_stop_hook_creates_hooks_json() {
        let dir = tempdir().unwrap();
        let cfg = dir.path().join("hooks.json");
        let bin = dir.path().join("flockmux-mcp");
        install_codex_stop_hook_at(&cfg, &bin, "http://127.0.0.1:7777", 10).unwrap();

        let root: Value = serde_json::from_slice(&fs::read(&cfg).unwrap()).unwrap();
        let stop = root["hooks"]["Stop"]
            .as_array()
            .expect("hooks.Stop is array");
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
        assert!(
            !cmd.contains("--agent-id"),
            "agent_id must NOT be in command: {cmd}"
        );
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
        assert_eq!(
            stop.len(),
            1,
            "repeated install must not accumulate entries"
        );
    }
}
