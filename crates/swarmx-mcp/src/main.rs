//! swarmx-mcp binary entry. Two modes:
//!
//!   - **default** (no subcommand): tokio stdio loop driving the MCP
//!     dispatcher. Stdin = newline-delimited JSON-RPC, stdout = same,
//!     stderr = tracing. EOF on stdin shuts the process down gracefully.
//!   - **`wake-check`**: invoked by Claude Code / Codex CLI as a Stop hook.
//!     POSTs `/api/message/consume_wakes` (atomically claims pending wakes;
//!     superseded the old `unread_count` GET, M6f) and emits a single JSON line
//!     on stdout that tells the CLI whether to keep the agent's turn going.
//!     Always exit 0; see `wake_check.rs` for the wire protocol.
//!
//! Identity (default mode):
//!   - `--agent-id` (or env `SWARMX_AGENT_ID`): which agent we speak for.
//!   - `--server-url` (or env `SWARMX_SERVER_URL`): REST base URL.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use swarmx_mcp::handlers::dispatch;
use swarmx_mcp::protocol::{JsonRpcRequest, JsonRpcResponse, PARSE_ERROR};
use swarmx_mcp::tools::ToolContext;
use swarmx_mcp::wake_check::{self, WakeCheckArgs};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tracing::{debug, warn};

#[derive(Debug, Parser)]
#[command(name = "swarmx-mcp", about = "swarmx swarm MCP stdio server")]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,

    // The default (stdio JSON-RPC) flags live on the root so existing
    // invocations `swarmx-mcp --agent-id <id>` keep working without a
    // subcommand prefix.
    /// Which agent we speak for (default mode only). Required when no
    /// subcommand is given.
    #[arg(long, env = "SWARMX_AGENT_ID")]
    agent_id: Option<String>,

    /// Base URL of the swarmx-server REST API (default mode only).
    #[arg(
        long,
        env = "SWARMX_SERVER_URL",
        default_value = "http://127.0.0.1:7777"
    )]
    server_url: String,
}

#[derive(Debug, Subcommand)]
enum Cmd {
    /// Stop-hook helper: probe unread count and emit a continuation hint
    /// for Claude Code / Codex when the agent has unread swarm messages.
    WakeCheck(WakeCheckArgs),
}

#[tokio::main]
async fn main() -> Result<()> {
    // Tracing must NEVER write to stdout — stdout is reserved for the wire
    // protocol of whichever mode we're in.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("SWARMX_MCP_LOG")
                .unwrap_or_else(|_| "warn,swarmx_mcp=info".into()),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    match cli.cmd {
        Some(Cmd::WakeCheck(args)) => {
            wake_check::run(args).await?;
            Ok(())
        }
        None => {
            // A swarmx-spawned worker always gets SWARMX_AGENT_ID. When the
            // binary is instead mounted by an EXTERNAL client (a developer's own
            // Claude Code / IDE pointing SWARMX_SERVER_URL at a running
            // swarmx), there's no agent identity — default to "external" so the
            // swarm_* tools (read blackboard, list agents, send message, …) work
            // out of the box. This is what makes swarmx usable as an OUTWARD
            // MCP server, not just the inward per-worker one.
            let agent_id = cli.agent_id.unwrap_or_else(|| "external".to_string());
            let ctx = ToolContext::new(agent_id.clone(), cli.server_url.clone())
                .context("build ToolContext (reqwest client)")?;
            debug!(agent_id = %agent_id, server = %cli.server_url, "swarmx-mcp starting");
            run_stdio(ctx).await
        }
    }
}

async fn run_stdio(ctx: ToolContext) -> Result<()> {
    let stdin = tokio::io::stdin();
    let mut reader = BufReader::new(stdin).lines();
    let mut stdout = tokio::io::stdout();

    while let Some(line) = reader
        .next_line()
        .await
        .context("read line from stdin")?
    {
        if line.trim().is_empty() {
            continue;
        }
        let response = handle_line(&ctx, &line).await;
        if let Some(resp) = response {
            let serialized = serde_json::to_string(&resp)
                .context("serialize JSON-RPC response")?;
            stdout.write_all(serialized.as_bytes()).await?;
            stdout.write_all(b"\n").await?;
            stdout.flush().await?;
        }
    }
    debug!("stdin EOF — shutting down");
    Ok(())
}

/// Returns `None` for notifications (no response per JSON-RPC 2.0).
async fn handle_line(ctx: &ToolContext, line: &str) -> Option<JsonRpcResponse> {
    let req: JsonRpcRequest = match serde_json::from_str(line) {
        Ok(req) => req,
        Err(err) => {
            warn!(?err, "failed to parse JSON-RPC request — emitting parse error");
            return Some(JsonRpcResponse::err(
                Value::Null,
                PARSE_ERROR,
                format!("parse error: {err}"),
            ));
        }
    };

    if req.is_notification() {
        debug!(method = %req.method, "notification received");
        return None;
    }

    let id = req.id.clone().unwrap_or(json!(null));
    Some(dispatch(ctx, id, &req.method, req.params).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_ctx() -> ToolContext {
        ToolContext::new("claude-test".into(), "http://127.0.0.1:1".into()).unwrap()
    }

    #[tokio::test]
    async fn parse_error_yields_response_with_null_id() {
        let ctx = dummy_ctx();
        let resp = handle_line(&ctx, "this is not json").await.unwrap();
        assert_eq!(resp.id, Value::Null);
        let err = resp.error.unwrap();
        assert_eq!(err.code, PARSE_ERROR);
    }

    #[tokio::test]
    async fn notification_yields_no_response() {
        let ctx = dummy_ctx();
        let resp = handle_line(
            &ctx,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        )
        .await;
        assert!(resp.is_none());
    }

    #[tokio::test]
    async fn initialize_round_trip() {
        let ctx = dummy_ctx();
        let resp = handle_line(
            &ctx,
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
        )
        .await
        .unwrap();
        assert_eq!(resp.id, json!(1));
        let result = resp.result.unwrap();
        assert_eq!(result["serverInfo"]["name"], "swarmx-swarm");
    }
}
