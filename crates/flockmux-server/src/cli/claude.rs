//! Claude Code adapter — everything claude needs that no other CLI does, in one
//! place. Pre-spawn home/workspace patches target `~/.claude.json` (trust + MCP
//! local scope) and `<ws>/.claude/settings.local.json` (wake Stop-hook, ms
//! timeout). At spawn it injects a per-agent `--mcp-config … --strict-mcp-config`
//! (to dodge the shared-cwd `~/.claude.json` collision) and pins `--session-id`
//! so the transcript tailer locates the exact JSONL.
//!
//! Selected by [`super::adapter_for`] for any plugin whose config formats are
//! claude-shaped (`mcp_format = "claude-local-scope"`); the literal id is never
//! matched, so a claude-compatible CLI reusing these formats gets this behavior
//! for free.

use super::shared::{
    home_path, lock_config_patch, merge_stop_hook, render_wake_command, write_json_atomic,
};
use super::{CliAdapter, PreSpawnCtx};
use crate::plugins::CliPlugin;
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// Zero-sized behavior object for the Claude family.
pub struct ClaudeAdapter;

impl CliAdapter for ClaudeAdapter {
    fn name(&self) -> &'static str {
        "claude"
    }

    fn pre_spawn(&self, plugin: &CliPlugin, workspace: &Path, ctx: &PreSpawnCtx) {
        // 1. Trust: pre-accept "Do you trust this folder?".
        if plugin.auto_trust_workspace {
            if let Err(err) = mark_workspace_trusted(workspace) {
                tracing::warn!(?err, cli = %plugin.id, "claude auto-trust patch failed");
            }
        }
        // 2. MCP: a local-scope entry in ~/.claude.json (no "trust this MCP
        //    server?" prompt) PLUS a per-agent file that `contribute_argv`
        //    passes as `--mcp-config <file> --strict-mcp-config` to dodge the
        //    shared-cwd ~/.claude.json mcpServers collision (M6b).
        if plugin.auto_inject_mcp {
            if let Err(err) =
                mark_mcp_local(workspace, &ctx.agent_id, &ctx.mcp_bin, &ctx.server_url)
            {
                tracing::warn!(?err, "claude: mcp-inject patch failed");
            }
            if let Err(err) =
                write_per_agent_mcp_config(&ctx.agent_id, &ctx.mcp_bin, &ctx.server_url)
            {
                tracing::warn!(?err, "claude: per-agent mcp file write failed");
            }
        }
        // 3. Wake: workspace-local Stop hook (timeout in ms).
        if plugin.auto_inject_stop_hook {
            if let Err(err) = install_stop_hook(
                workspace,
                &ctx.mcp_bin,
                &ctx.server_url,
                plugin.stop_hook_timeout,
            ) {
                tracing::warn!(?err, cli = %plugin.id, "claude stop-hook install failed");
            }
        }
    }

    fn contribute_argv(&self, plugin: &CliPlugin, agent_id: &str, argv: &mut Vec<String>) {
        // Point claude at the per-agent MCP config pre_spawn wrote.
        // `--strict-mcp-config` makes claude ignore `~/.claude.json` entirely so
        // a sibling spawn that overwrote the workspace's mcpServers section (the
        // shared_workspace collision that hung M6b run #4) can no longer leak
        // someone else's agent_id into our MCP server. Skipped if the file
        // wasn't written (no $HOME) — fall back to legacy ~/.claude.json path.
        if !plugin.auto_inject_mcp {
            return;
        }
        if let Some(path) = per_agent_mcp_config_path(agent_id) {
            if path.is_file() {
                argv.push("--mcp-config".into());
                argv.push(path.to_string_lossy().into_owned());
                argv.push("--strict-mcp-config".into());
                tracing::info!(
                    agent = %agent_id,
                    mcp_config = %path.display(),
                    "claude per-agent MCP config injected (bypasses ~/.claude.json collision)"
                );
            } else {
                tracing::warn!(
                    agent = %agent_id,
                    mcp_config = %path.display(),
                    "claude per-agent MCP config missing on disk; falling back to ~/.claude.json"
                );
            }
        }
    }

    fn transcript_session_id(
        &self,
        _plugin: &CliPlugin,
        agent_id: &str,
        argv: &mut Vec<String>,
    ) -> Option<String> {
        // Force a known session id so the transcript tailer locates the exact
        // session JSONL (`<uuid>.jsonl`) instead of guessing the newest file in
        // the project dir — a stale prior session in the same workspace would
        // otherwise win.
        let sid = Uuid::new_v4().to_string();
        argv.push("--session-id".into());
        argv.push(sid.clone());
        tracing::info!(agent = %agent_id, session_id = %sid, "claude --session-id forced for transcript location");
        Some(sid)
    }
}

/// Mark `workspace` as trusted in `~/.claude.json`. No-op if the file doesn't
/// exist (claude hasn't run yet) or already has the flag set.
fn mark_workspace_trusted(workspace: &Path) -> Result<()> {
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

/// The `flockmux-swarm` MCP entry for one agent. Shared by the local-scope
/// patch and the per-agent config so the two channels never drift.
fn swarm_mcp_entry(agent_id: &str, mcp_bin: &Path, server_url: &str) -> Value {
    json!({
        "type": "stdio",
        "command": mcp_bin.to_string_lossy(),
        "args": ["--agent-id", agent_id],
        "env": {
            "FLOCKMUX_AGENT_ID": agent_id,
            "FLOCKMUX_SERVER_URL": server_url,
        }
    })
}

/// Mark `flockmux-swarm` as a local-scope MCP server in
/// `~/.claude.json projects.<workspace>.mcpServers`. We use *local* scope (per
/// project, baked into `~/.claude.json`) rather than *project* scope
/// (`.mcp.json` in the repo) so claude never shows the "do you trust this MCP
/// server?" prompt — local scope is implicitly trusted because the user owns
/// the file.
///
/// Each spawn writes its own `args` carrying `--agent-id <id>` plus an env
/// passthrough block. Two channels for the same data: claude's
/// `args ["--agent-id", "..."]` and `env { FLOCKMUX_AGENT_ID: "..." }`. If the
/// user later runs `flockmux-mcp` by hand the env wins; if claude clears the
/// env block, the args still identify the agent.
///
/// No-op if the file doesn't exist (claude hasn't run yet) or the entry is
/// already up-to-date.
fn mark_mcp_local(
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

    let mcp_servers = project.entry("mcpServers").or_insert_with(|| json!({}));
    let mcp_servers = mcp_servers
        .as_object_mut()
        .context(".claude.json mcpServers is not an object")?;

    let desired = swarm_mcp_entry(agent_id, mcp_bin, server_url);

    if mcp_servers.get("flockmux-swarm") == Some(&desired) {
        return Ok(());
    }
    mcp_servers.insert("flockmux-swarm".into(), desired);

    write_json_atomic(cfg, &root)
}

/// Write a workspace-local `.claude/settings.local.json` carrying a Stop
/// hook that calls `flockmux-mcp wake-check`.
///
/// Project-local (workspace-scoped) on purpose: we don't want to pollute
/// the user's `~/.claude/settings.json`, and the hook is only meaningful
/// inside the flockmux-managed workspace anyway.
fn install_stop_hook(
    workspace: &Path,
    mcp_bin: &Path,
    server_url: &str,
    timeout: i64,
) -> Result<()> {
    let cfg_dir = workspace.join(".claude");
    fs::create_dir_all(&cfg_dir).with_context(|| format!("mkdir {}", cfg_dir.display()))?;
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
        serde_json::from_slice(&bytes).with_context(|| format!("parse {}", cfg.display()))?
    } else {
        json!({})
    };

    let command = render_wake_command(mcp_bin, server_url);
    merge_stop_hook(&mut root, &command, timeout);
    write_json_atomic(cfg, &root)
}

/// Per-agent claude MCP config file. Written under `~/.flockmux/mcp/` keyed
/// by `agent_id`, intended to be passed to `claude --mcp-config <path>
/// --strict-mcp-config` so claude completely ignores the shared
/// `~/.claude.json` config and uses ONLY this file.
///
/// Why this exists: `~/.claude.json` keys MCP servers by project (cwd) path.
/// In shared_workspace spells (M6a fullstack-feature) all 3 agents have the
/// same cwd, so each `mark_mcp_local()` overwrites the previous agent's entry —
/// when claude lazy-launches its MCP server the file now holds the LAST spawn's
/// identity, leaving the other agents impersonating each other. Confirmed in
/// M6b run #4: FE's MCP server reported its id as the test agent's id, FE
/// concluded "there's a separate FE agent" and stopped to ask for
/// clarification, never wrote code. This per-agent override sidesteps the
/// collision entirely.
fn write_per_agent_mcp_config(agent_id: &str, mcp_bin: &Path, server_url: &str) -> Result<PathBuf> {
    let path = per_agent_mcp_config_path(agent_id)
        .context("home not found; cannot write per-agent claude MCP config")?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    // `--strict-mcp-config` makes claude read ONLY this file, ignoring
    // `~/.claude.json` entirely. So to let a swarm worker actually USE the
    // user's own MCP servers (context7, …) — the ones they enabled in the
    // dashboard's MCP panel, written to user scope — we copy that user-scope
    // `mcpServers` block in here, then force-overwrite flockmux-swarm with THIS
    // agent's entry (never inherit a stale/foreign swarm id — that was the M6b
    // shared-cwd collision). claude tolerates an MCP server that fails to
    // connect, so inheriting a broken one degrades that one tool, not the agent.
    let body = per_agent_mcp_body(read_user_scope_mcp_servers(), agent_id, mcp_bin, server_url);
    write_json_atomic(&path, &body)?;
    Ok(path)
}

/// User-scope MCP servers from `~/.claude.json`'s top-level `mcpServers` (where
/// `claude mcp add --scope user` writes). Empty map if the file is absent /
/// unparseable / has no such block — inheriting nothing is the safe floor.
fn read_user_scope_mcp_servers() -> serde_json::Map<String, Value> {
    match home_path().map(|h| h.join(".claude.json")) {
        Some(cfg) => read_user_scope_mcp_servers_at(&cfg),
        None => Default::default(),
    }
}

/// Pure read of the TOP-LEVEL `mcpServers` (user scope) from a given config
/// path. Deliberately NOT `projects.<ws>.mcpServers` (local scope) — only the
/// user-scope block, which is what `claude mcp add --scope user` and the
/// dashboard MCP panel write, should be inherited by every worker.
fn read_user_scope_mcp_servers_at(cfg: &Path) -> serde_json::Map<String, Value> {
    let Ok(bytes) = fs::read(cfg) else {
        return Default::default();
    };
    serde_json::from_slice::<Value>(&bytes)
        .ok()
        .as_ref()
        .and_then(|v| v.get("mcpServers"))
        .and_then(|m| m.as_object())
        .cloned()
        .unwrap_or_default()
}

/// Build the per-agent `--mcp-config` body: the inherited user-scope servers
/// plus a guaranteed-correct per-agent flockmux-swarm. Pure (no IO) so the merge
/// rule is unit-tested. Any `flockmux-swarm` from the user scope is dropped first
/// so ONLY this agent's entry survives (no cross-agent identity leak).
fn per_agent_mcp_body(
    mut servers: serde_json::Map<String, Value>,
    agent_id: &str,
    mcp_bin: &Path,
    server_url: &str,
) -> Value {
    servers.remove("flockmux-swarm");
    servers.insert(
        "flockmux-swarm".into(),
        swarm_mcp_entry(agent_id, mcp_bin, server_url),
    );
    json!({ "mcpServers": Value::Object(servers) })
}

/// Computes the path `write_per_agent_mcp_config()` writes to without touching
/// disk. `contribute_argv` uses this to find the `--mcp-config` value at launch
/// time. Returns `None` if `$HOME` is not set (then claude has no home anyway
/// and would have failed earlier).
fn per_agent_mcp_config_path(agent_id: &str) -> Option<PathBuf> {
    home_path().map(|h| {
        h.join(".flockmux")
            .join("mcp")
            .join(format!("{agent_id}.json"))
    })
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
        fs::write(
            &cfg,
            serde_json::to_vec(&json!({ "projects": {} })).unwrap(),
        )
        .unwrap();

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

    // ── per-agent MCP body: inherit user-scope + force-correct swarm ─────

    #[test]
    fn per_agent_body_inherits_user_servers_and_overrides_swarm() {
        let mcp_bin = std::path::Path::new("/opt/flockmux-mcp");
        let user: serde_json::Map<String, Value> = serde_json::from_value(json!({
            "context7": {
                "type": "stdio",
                "command": "npx",
                "args": ["-y", "@upstash/context7-mcp"],
            },
            // A STALE swarm entry pointing at someone else's agent: it must be
            // dropped, not leak into this agent's config (the M6b collision).
            "flockmux-swarm": {
                "type": "stdio",
                "command": "/old/flockmux-mcp",
                "args": ["--agent-id", "OTHER-AGENT"],
            },
        }))
        .unwrap();

        let body = per_agent_mcp_body(user, "agent-A", mcp_bin, "http://127.0.0.1:7777");
        let servers = body["mcpServers"].as_object().unwrap();

        // The user's own server is inherited verbatim (worker can now use it).
        assert_eq!(servers["context7"]["command"], json!("npx"));
        assert_eq!(
            servers["context7"]["args"],
            json!(["-y", "@upstash/context7-mcp"])
        );
        // flockmux-swarm is force-overwritten with THIS agent + THIS binary,
        // never the stale OTHER-AGENT entry.
        assert_eq!(
            servers["flockmux-swarm"]["command"],
            json!("/opt/flockmux-mcp")
        );
        assert_eq!(
            servers["flockmux-swarm"]["args"],
            json!(["--agent-id", "agent-A"])
        );
        assert_eq!(
            servers["flockmux-swarm"]["env"]["FLOCKMUX_AGENT_ID"],
            json!("agent-A")
        );
    }

    #[test]
    fn per_agent_body_with_no_user_servers_has_only_swarm() {
        let body = per_agent_mcp_body(
            serde_json::Map::new(),
            "solo",
            std::path::Path::new("/m"),
            "http://x",
        );
        let servers = body["mcpServers"].as_object().unwrap();
        assert_eq!(servers.len(), 1);
        assert!(servers.contains_key("flockmux-swarm"));
    }

    #[test]
    fn read_user_scope_takes_top_level_not_project_scope() {
        let dir = tempdir().unwrap();
        let cfg = dir.path().join(".claude.json");
        fs::write(
            &cfg,
            serde_json::to_vec(&json!({
                // user scope (top-level) — THIS is what every worker inherits.
                "mcpServers": {
                    "context7": { "type": "stdio", "command": "npx" },
                    "chrome-devtools": { "type": "stdio", "command": "npx" },
                },
                // local/project scope — must NOT be inherited: it carries the
                // per-workspace flockmux-swarm with a possibly-foreign agent id.
                "projects": {
                    "/some/ws": {
                        "mcpServers": {
                            "flockmux-swarm": { "command": "/x", "args": ["--agent-id", "OTHER"] }
                        }
                    }
                }
            }))
            .unwrap(),
        )
        .unwrap();

        let got = read_user_scope_mcp_servers_at(&cfg);
        assert!(got.contains_key("context7"));
        assert!(got.contains_key("chrome-devtools"));
        // Project-scope entries are invisible here (we read only top level).
        assert_eq!(got.len(), 2);
    }

    #[test]
    fn read_user_scope_empty_when_absent_or_no_block() {
        let dir = tempdir().unwrap();
        // Missing file -> empty (inherit nothing; safe floor).
        assert!(read_user_scope_mcp_servers_at(&dir.path().join("nope.json")).is_empty());
        // Present but no mcpServers block -> empty.
        let cfg = dir.path().join(".claude.json");
        fs::write(
            &cfg,
            serde_json::to_vec(&json!({ "projects": {} })).unwrap(),
        )
        .unwrap();
        assert!(read_user_scope_mcp_servers_at(&cfg).is_empty());
    }

    // ── claude MCP local-scope patch ─────────────────────────────────────

    #[test]
    fn claude_mcp_local_writes_new_entry() {
        let dir = tempdir().unwrap();
        let cfg = dir.path().join("claude.json");
        fs::write(
            &cfg,
            serde_json::to_vec(&json!({ "projects": {} })).unwrap(),
        )
        .unwrap();
        let ws = dir.path().join("ws-A");
        let bin = dir.path().join("flockmux-mcp");

        patch_claude_mcp_at(&cfg, &ws, "claude-aaa", &bin, "http://127.0.0.1:7777").unwrap();

        let written: Value = serde_json::from_slice(&fs::read(&cfg).unwrap()).unwrap();
        let entry =
            &written["projects"][ws.to_string_lossy().as_ref()]["mcpServers"]["flockmux-swarm"];
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
        fs::write(
            &cfg,
            serde_json::to_vec(&json!({ "projects": {} })).unwrap(),
        )
        .unwrap();
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
        assert_eq!(
            mcp["flockmux-swarm"]["env"]["FLOCKMUX_AGENT_ID"],
            json!("claude-ccc")
        );
    }

    // ── M5b Stop-hook install patches ─────────────────────────────────────

    #[test]
    fn claude_stop_hook_creates_settings_local() {
        let dir = tempdir().unwrap();
        let cfg = dir.path().join("settings.local.json");
        let bin = dir.path().join("flockmux-mcp");
        install_claude_stop_hook_at(&cfg, &bin, "http://127.0.0.1:7777", 10_000).unwrap();

        let root: Value = serde_json::from_slice(&fs::read(&cfg).unwrap()).unwrap();
        let stop = root["hooks"]["Stop"]
            .as_array()
            .expect("hooks.Stop is array");
        assert_eq!(stop.len(), 1);
        let inner = stop[0]["hooks"][0].clone();
        assert_eq!(inner["type"], json!("command"));
        assert_eq!(inner["timeout"], json!(10_000), "claude timeout in ms");
        let cmd = inner["command"].as_str().unwrap();
        assert!(cmd.contains("wake-check"), "got: {cmd}");
        assert!(cmd.contains("--server http://127.0.0.1:7777"), "got: {cmd}");
        assert!(
            cmd.contains(bin.to_string_lossy().as_ref()),
            "absolute bin path: {cmd}"
        );
        // Trust-stability invariant: command must NOT carry per-spawn identity,
        // otherwise codex 0.130+ would re-prompt /hooks on every new agent.
        assert!(
            !cmd.contains("--agent-id"),
            "agent_id must NOT be in command: {cmd}"
        );
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
        assert_eq!(
            pre[0]["hooks"][0]["command"],
            json!("/usr/local/bin/user-lint")
        );
        // Stop now has TWO entries: the user's (first) and flockmux (last).
        let stop = after["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(
            stop.len(),
            2,
            "user hook should be preserved + wake-check appended"
        );
        assert_eq!(
            stop[0]["hooks"][0]["command"],
            json!("/usr/local/bin/user-stop"),
            "user hook stays first",
        );
        let cmd = stop[1]["hooks"][0]["command"].as_str().unwrap();
        assert!(
            cmd.contains("wake-check"),
            "flockmux entry appended at end: {cmd}"
        );
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
