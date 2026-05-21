//! flockmux-mcp binary: tokio stdio loop driving the MCP dispatcher.
//!
//! Operationally:
//!   - stdin  → newline-delimited JSON-RPC requests (one per line).
//!   - stdout → newline-delimited JSON-RPC responses. NOTHING ELSE may go
//!              here; tracing / panics go to stderr.
//!   - EOF on stdin = graceful shutdown.
//!
//! Identity:
//!   - `--agent-id` (or env `FLOCKMUX_AGENT_ID`): which agent we're acting
//!     as. Required. If missing, exit 1 — claude / codex will surface
//!     "server failed" in their /mcp panel.
//!   - `--server-url` (or env `FLOCKMUX_SERVER_URL`): where the REST API
//!     lives. Defaults to `http://127.0.0.1:7777`.

use anyhow::{Context, Result};
use clap::Parser;
use flockmux_mcp::handlers::dispatch;
use flockmux_mcp::protocol::{JsonRpcRequest, JsonRpcResponse, PARSE_ERROR};
use flockmux_mcp::tools::ToolContext;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tracing::{debug, warn};

#[derive(Debug, Parser)]
#[command(name = "flockmux-mcp", about = "flockmux swarm MCP stdio server")]
struct Args {
    /// Which agent we speak for. Required — comes from the spawn-time env
    /// `FLOCKMUX_AGENT_ID` that flockmux-server injects into the subprocess.
    #[arg(long, env = "FLOCKMUX_AGENT_ID")]
    agent_id: String,

    /// Base URL of the flockmux-server REST API.
    #[arg(
        long,
        env = "FLOCKMUX_SERVER_URL",
        default_value = "http://127.0.0.1:7777"
    )]
    server_url: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Tracing must NEVER write to stdout — stdout is the JSON-RPC stream.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("FLOCKMUX_MCP_LOG")
                .unwrap_or_else(|_| "warn,flockmux_mcp=info".into()),
        )
        .with_writer(std::io::stderr)
        .init();

    let args = Args::parse();
    let ctx = ToolContext::new(args.agent_id.clone(), args.server_url.clone())
        .context("build ToolContext (reqwest client)")?;

    debug!(agent_id = %args.agent_id, server = %args.server_url, "flockmux-mcp starting");

    run(ctx).await
}

async fn run(ctx: ToolContext) -> Result<()> {
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
        assert_eq!(result["serverInfo"]["name"], "flockmux-swarm");
    }
}
