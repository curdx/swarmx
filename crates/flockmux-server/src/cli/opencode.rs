//! OpenCode adapter — everything opencode needs that no other CLI does, in one
//! place. opencode has no "trust this folder?" gate and no blocking Stop hook,
//! so its whole integration is ONE per-agent config file at
//! `~/.flockmux/opencode/<agent_id>.json`: the flockmux-swarm MCP server (with
//! this agent's identity), `permission = "allow"`, `autoupdate = false`, and the
//! wake plugin merged into `plugin[]`. `contribute_env` points opencode at it via
//! `OPENCODE_CONFIG` (verified live: it DEEP-MERGES on top of the user's config,
//! so the user's provider/model survives and flockmux's keys win).
//!
//! opencode runs as a full-screen TUI over the PTY like claude/codex, but its
//! bootstrap/wakes are delivered over its `/tui/*` HTTP control API rather than
//! keystrokes (`input_delivery = "opencode-tui-http"`; see `crate::opencode_tui`
//! + `spawn.rs`). Selected by [`super::adapter_for`] for
//! `mcp_format = "opencode-json"`.

use super::shared::{home_path, write_json_atomic};
use super::{CliAdapter, PreSpawnCtx};
use crate::plugins::CliPlugin;
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Zero-sized behavior object for the OpenCode family.
pub struct OpencodeAdapter;

impl CliAdapter for OpencodeAdapter {
    fn name(&self) -> &'static str {
        "opencode"
    }

    fn pre_spawn(&self, plugin: &CliPlugin, _workspace: &Path, ctx: &PreSpawnCtx) {
        // 1. MCP: one per-agent config file carries mcp + permission +
        //    autoupdate. `contribute_env` points opencode at it via
        //    OPENCODE_CONFIG. The wake plugin is merged into the SAME file by
        //    the stop-hook step below — order matters (MCP writer first).
        if plugin.auto_inject_mcp {
            if let Err(err) =
                write_opencode_per_agent_config(&ctx.agent_id, &ctx.mcp_bin, &ctx.server_url)
            {
                tracing::warn!(?err, "opencode: per-agent config write failed");
            }
        }
        // 2. Wake: opencode has NO blocking Stop hook, so "wake" is delivered as
        //    an opencode PLUGIN appended into the per-agent config's plugin[].
        if plugin.auto_inject_stop_hook {
            if let Err(err) = install_opencode_wake_plugin(&ctx.agent_id) {
                tracing::warn!(?err, "opencode: wake plugin install failed");
            }
        }
        // opencode has no trust gate and suppresses updates via
        // `autoupdate = false` in the per-agent config, so there is no trust /
        // update-dismiss step.
    }

    fn contribute_env(
        &self,
        plugin: &CliPlugin,
        agent_id: &str,
        env: &mut HashMap<String, String>,
    ) {
        // Point the worker at its per-agent OPENCODE_CONFIG (written by
        // pre_spawn). VERIFIED LIVE: OPENCODE_CONFIG deep-MERGES on top of the
        // user's config (it does NOT replace it) — flockmux's keys (swarm MCP w/
        // this agent's identity, allow-all permission, autoupdate off, wake
        // plugin) win on conflict, while the user's provider/model config is
        // preserved so the worker can run a model. Per-agent identity stays
        // collision-free across PerAgent and Shared layouts (each process has
        // its own file). Gated on the file existing.
        if !plugin.auto_inject_mcp {
            return;
        }
        if let Some(cfg) = opencode_per_agent_config_path(agent_id) {
            if cfg.is_file() {
                env.insert("OPENCODE_CONFIG".into(), cfg.to_string_lossy().into_owned());
                tracing::info!(
                    agent = %agent_id,
                    opencode_config = %cfg.display(),
                    "opencode per-agent OPENCODE_CONFIG injected (isolated config: swarm MCP + allow + wake plugin)"
                );
            }
        }
    }
}

/// Per-agent opencode config file. Written under `~/.flockmux/opencode/` keyed
/// by `agent_id`; `contribute_env` sets `OPENCODE_CONFIG=<this path>`.
///
/// VERIFIED LIVE (opencode 1.17.7): `OPENCODE_CONFIG` deep-MERGES on top of the
/// user's config — it does NOT replace it. flockmux's keys win on conflict, so
/// this file authoritatively sets the flockmux-swarm MCP server (with per-agent
/// identity in `environment`), `permission = "allow"` (headless: no approval
/// prompts), and `autoupdate = false`. The user's own `provider`/model config is
/// PRESERVED, so the worker can authenticate and run a model. Tradeoff: the
/// user's personal MCP servers also merge in, but opencode times bad ones out
/// (5s default) rather than hard-blocking.
///
/// Per-agent identity is collision-free even in Shared-workspace spells: each
/// process has its OWN OPENCODE_CONFIG file with its OWN agent_id, and flockmux
/// writes no project-local `<ws>/opencode.json` for them to clobber. The wake
/// `plugin[]` is appended separately by [`install_opencode_wake_plugin`].
fn write_opencode_per_agent_config(
    agent_id: &str,
    mcp_bin: &Path,
    server_url: &str,
) -> Result<PathBuf> {
    let path = opencode_per_agent_config_path(agent_id)
        .context("home not found; cannot write per-agent opencode config")?;
    write_opencode_config_at(&path, agent_id, mcp_bin, server_url)?;
    Ok(path)
}

/// Path-explicit core of [`write_opencode_per_agent_config`] (testable without
/// touching `$HOME`).
fn write_opencode_config_at(
    path: &Path,
    agent_id: &str,
    mcp_bin: &Path,
    server_url: &str,
) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let body = json!({
        "$schema": "https://opencode.ai/config.json",
        "autoupdate": false,
        // Top-level bare-string permission normalizes to {"*":"allow"} in
        // opencode (src/v1/config/permission.ts), i.e. full auto-approve.
        "permission": "allow",
        "mcp": {
            "flockmux-swarm": {
                "type": "local",
                "command": [mcp_bin.to_string_lossy(), "--agent-id", agent_id],
                "environment": {
                    "FLOCKMUX_AGENT_ID": agent_id,
                    "FLOCKMUX_SERVER_URL": server_url,
                },
                "enabled": true,
            }
        }
    });
    write_json_atomic(path, &body)
}

/// Computes the path [`write_opencode_per_agent_config`] writes to without
/// touching disk. `contribute_env` uses this to set `OPENCODE_CONFIG`. Returns
/// `None` if `$HOME` is not set.
fn opencode_per_agent_config_path(agent_id: &str) -> Option<PathBuf> {
    home_path().map(|h| {
        h.join(".flockmux")
            .join("opencode")
            .join(format!("{agent_id}.json"))
    })
}

/// Merge the flockmux wake plugin into the per-agent opencode config's
/// `plugin[]`. opencode has no blocking Stop hook, so the wake loop runs as a
/// plugin: on `session.idle` it calls flockmux's `consume_wakes` and re-prompts
/// the session when wakes are pending (see `cli-plugins/opencode/flockmux-wake.js`
/// — the opencode equivalent of `flockmux-mcp wake-check`).
///
/// Read-modify-write of the file [`write_opencode_per_agent_config`] just wrote
/// (pre_spawn calls the MCP writer first). Idempotent: the plugin entry is
/// appended once. No-op + warn if the bundled plugin JS can't be located — the
/// worker still has swarm_* tools but won't be auto-rewoken.
fn install_opencode_wake_plugin(agent_id: &str) -> Result<()> {
    let path = opencode_per_agent_config_path(agent_id)
        .context("home not found; cannot install opencode wake plugin")?;
    let plugin = match opencode_wake_plugin_path() {
        Some(p) => p,
        None => {
            tracing::warn!(
                agent = %agent_id,
                "opencode wake plugin JS not found; worker will have swarm_* tools but no \
                 auto-wake (set FLOCKMUX_OPENCODE_PLUGIN, or ship cli-plugins/opencode/flockmux-wake.js)"
            );
            return Ok(());
        }
    };
    merge_opencode_plugin_at(&path, &plugin)
}

/// Path-explicit core of [`install_opencode_wake_plugin`]: idempotently add
/// `plugin_path` to the config file's `plugin[]`. Testable without `$HOME` or
/// the bundled-plugin lookup.
fn merge_opencode_plugin_at(path: &Path, plugin_path: &Path) -> Result<()> {
    let mut root: Value = if path.is_file() {
        let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
        serde_json::from_slice(&bytes).unwrap_or_else(|_| json!({}))
    } else {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        json!({})
    };
    let entry = Value::String(plugin_path.to_string_lossy().into_owned());
    match root.get_mut("plugin").and_then(|v| v.as_array_mut()) {
        Some(arr) => {
            if !arr.iter().any(|v| v == &entry) {
                arr.push(entry);
            }
        }
        None => {
            root["plugin"] = Value::Array(vec![entry]);
        }
    }
    write_json_atomic(path, &root)
}

/// Locate the bundled flockmux opencode wake plugin (a small JS file). Env
/// override first (the Tauri sidecar sets `FLOCKMUX_OPENCODE_PLUGIN` to the
/// packaged resource path), then a `CARGO_MANIFEST_DIR`-relative repo path for
/// dev. Returns `None` if neither resolves (caller warns + degrades). Mirrors
/// the `locate_*` helpers in `spawn.rs`.
fn opencode_wake_plugin_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("FLOCKMUX_OPENCODE_PLUGIN") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    // dev fallback: <workspace-root>/cli-plugins/opencode/flockmux-wake.js
    // (CARGO_MANIFEST_DIR is crates/flockmux-server; step up two levels).
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    if let Some(ws) = manifest.parent().and_then(|p| p.parent()) {
        let cand = ws
            .join("cli-plugins")
            .join("opencode")
            .join("flockmux-wake.js");
        if cand.is_file() {
            return Some(cand);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;

    #[test]
    fn opencode_config_writes_mcp_permission_and_autoupdate() {
        let dir = tempdir().unwrap();
        let cfg = dir.path().join("agent.json");
        let bin = dir.path().join("flockmux-mcp");
        write_opencode_config_at(&cfg, "opencode-abc123", &bin, "http://127.0.0.1:7777").unwrap();
        let v: Value = serde_json::from_slice(&fs::read(&cfg).unwrap()).unwrap();
        // Headless posture: no startup update, full auto-approve.
        assert_eq!(v["autoupdate"], json!(false));
        assert_eq!(v["permission"], json!("allow"));
        // swarm MCP server: local stdio, per-agent identity in command + environment.
        let mcp = &v["mcp"]["flockmux-swarm"];
        assert_eq!(mcp["type"], json!("local"));
        assert_eq!(mcp["enabled"], json!(true));
        assert_eq!(
            mcp["command"],
            json!([bin.to_string_lossy(), "--agent-id", "opencode-abc123"])
        );
        assert_eq!(
            mcp["environment"]["FLOCKMUX_AGENT_ID"],
            json!("opencode-abc123")
        );
        assert_eq!(
            mcp["environment"]["FLOCKMUX_SERVER_URL"],
            json!("http://127.0.0.1:7777")
        );
    }

    #[test]
    fn opencode_plugin_merge_is_idempotent_and_preserves_mcp() {
        let dir = tempdir().unwrap();
        let cfg = dir.path().join("agent.json");
        let bin = dir.path().join("flockmux-mcp");
        write_opencode_config_at(&cfg, "opencode-xyz", &bin, "http://127.0.0.1:7777").unwrap();
        let plugin = dir.path().join("flockmux-wake.js");
        // Merge twice → exactly one entry (pre_spawn may re-run on re-spawn).
        merge_opencode_plugin_at(&cfg, &plugin).unwrap();
        merge_opencode_plugin_at(&cfg, &plugin).unwrap();
        let v: Value = serde_json::from_slice(&fs::read(&cfg).unwrap()).unwrap();
        assert_eq!(
            v["plugin"],
            json!([plugin.to_string_lossy()]),
            "plugin appended exactly once (idempotent)"
        );
        // The mcp + permission the MCP writer wrote must survive the
        // read-modify-write of the plugin merge.
        assert_eq!(v["permission"], json!("allow"));
        assert_eq!(v["mcp"]["flockmux-swarm"]["type"], json!("local"));
    }

    #[test]
    fn opencode_plugin_merge_creates_file_when_config_absent() {
        let dir = tempdir().unwrap();
        let cfg = dir.path().join("nested").join("agent.json");
        let plugin = dir.path().join("flockmux-wake.js");
        // No prior config (e.g. the MCP write was skipped) → still mints a valid
        // file carrying just the plugin, so wake degrades gracefully.
        merge_opencode_plugin_at(&cfg, &plugin).unwrap();
        let v: Value = serde_json::from_slice(&fs::read(&cfg).unwrap()).unwrap();
        assert_eq!(v["plugin"], json!([plugin.to_string_lossy()]));
    }
}
