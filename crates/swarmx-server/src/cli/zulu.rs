//! zulu (Comate) CLI adapter — writes the swarm MCP config where zulu reads it.
//!
//! Unlike reasonix (root `<ws>/.mcp.json`), zulu reads its MCP servers from
//! `<ws>/.comate/mcp.json` (verified: the Comate kernel's `MCP_SETTINGS_DIR=
//! ".comate" / mcp.json` — `zulu inspect` picks up `swarmx-swarm` from there).
//! Same standard `mcpServers` schema (`{"mcpServers":{...}}`). Per-agent
//! identity rides in the entry's `args` + `env`. Selected by
//! [`super::adapter_for`] for `mcp_format = "zulu-mcp-json"`.

use super::shared::write_json_atomic;
use super::{CliAdapter, PreSpawnCtx};
use crate::plugins::CliPlugin;
use anyhow::{Context, Result};
use serde_json::json;
use std::path::Path;

/// Zero-sized behavior object for the zulu (Comate) family.
pub struct ZuluAdapter;

impl CliAdapter for ZuluAdapter {
    fn name(&self) -> &'static str {
        "zulu"
    }

    fn pre_spawn(&self, plugin: &CliPlugin, workspace: &Path, ctx: &PreSpawnCtx) {
        if plugin.auto_inject_mcp {
            if let Err(err) =
                write_comate_mcp_json(workspace, &ctx.agent_id, &ctx.mcp_bin, &ctx.server_url)
            {
                tracing::warn!(?err, cli = %plugin.id, "zulu: .comate/mcp.json write failed (agent will lack swarm tools)");
            }
        }
        // Keep our managed .comate/mcp.json out of git's dirty accounting.
        crate::worktree::ignore_managed_artifacts(workspace);
    }
}

/// Write `<workspace>/.comate/mcp.json` carrying the swarmx-swarm MCP server in
/// the standard `mcpServers` schema (which the Comate kernel reads). Per-spawn
/// identity in both `args` (`--agent-id <id>`) and the `env` block.
fn write_comate_mcp_json(
    workspace: &Path,
    agent_id: &str,
    mcp_bin: &Path,
    server_url: &str,
) -> Result<()> {
    let dir = workspace.join(".comate");
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    let path = dir.join("mcp.json");
    let body = json!({
        "mcpServers": {
            "swarmx-swarm": {
                "command": mcp_bin.to_string_lossy(),
                "args": ["--agent-id", agent_id],
                "env": {
                    "SWARMX_AGENT_ID": agent_id,
                    "SWARMX_SERVER_URL": server_url,
                }
            }
        }
    });
    write_json_atomic(&path, &body).with_context(|| format!("write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn writes_comate_mcp_json_with_agent_identity() {
        let dir = tempdir().unwrap();
        let ws = dir.path();
        let bin = dir.path().join("swarmx-mcp");
        write_comate_mcp_json(ws, "zulu-abc12345", &bin, "http://127.0.0.1:7777").unwrap();

        // Written under .comate/, NOT the root .mcp.json.
        assert!(!ws.join(".mcp.json").exists());
        let written: Value =
            serde_json::from_slice(&fs::read(ws.join(".comate").join("mcp.json")).unwrap()).unwrap();
        let entry = &written["mcpServers"]["swarmx-swarm"];
        assert_eq!(entry["command"], json!(bin.to_string_lossy().as_ref()));
        assert_eq!(entry["args"], json!(["--agent-id", "zulu-abc12345"]));
        assert_eq!(entry["env"]["SWARMX_AGENT_ID"], json!("zulu-abc12345"));
        assert_eq!(
            entry["env"]["SWARMX_SERVER_URL"],
            json!("http://127.0.0.1:7777")
        );
    }
}
