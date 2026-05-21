//! Dispatch for the four JSON-RPC methods we expose:
//!   - `initialize`                  — handshake; returns protocol version + caps
//!   - `notifications/initialized`   — no response (notification)
//!   - `tools/list`                  — array of tool descriptors
//!   - `tools/call`                  — invoke one of the seven swarm_* tools
//!
//! Anything else returns METHOD_NOT_FOUND. We never panic on bad params —
//! claude treats a closed stdio as a server crash and stops trying to use it.

use crate::protocol::{JsonRpcResponse, INVALID_PARAMS, METHOD_NOT_FOUND};
use crate::tools::{call_tool, tool_descriptors, ToolContext};
use serde_json::{json, Value};

/// Protocol version we advertise on `initialize`. The string is checked by
/// claude / codex against their supported set; if they upgrade past this
/// version, the symptom is the server failing the handshake and bump
/// strings here.
const PROTOCOL_VERSION: &str = "2024-11-05";

pub async fn dispatch(
    ctx: &ToolContext,
    id: Value,
    method: &str,
    params: Option<Value>,
) -> JsonRpcResponse {
    match method {
        "initialize" => JsonRpcResponse::ok(id, initialize()),
        "tools/list" => JsonRpcResponse::ok(id, tools_list()),
        "tools/call" => match tools_call(ctx, params).await {
            Ok(result) => JsonRpcResponse::ok(id, result),
            Err(msg) => JsonRpcResponse::err(id, INVALID_PARAMS, msg),
        },
        other => JsonRpcResponse::err(
            id,
            METHOD_NOT_FOUND,
            format!("method not found: {other}"),
        ),
    }
}

fn initialize() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "serverInfo": {
            "name": "flockmux-swarm",
            "version": env!("CARGO_PKG_VERSION"),
        },
        "capabilities": {
            "tools": { "listChanged": false }
        }
    })
}

fn tools_list() -> Value {
    json!({ "tools": tool_descriptors() })
}

async fn tools_call(ctx: &ToolContext, params: Option<Value>) -> Result<Value, String> {
    let params = params.ok_or_else(|| "missing params".to_string())?;
    let name = params
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing 'name' in tools/call params".to_string())?;
    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    Ok(call_tool(ctx, name, &args).await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn dummy_ctx() -> ToolContext {
        ToolContext::new(
            "claude-test".into(),
            "http://127.0.0.1:1".into(), // never actually hit in these tests
        )
        .unwrap()
    }

    #[tokio::test]
    async fn initialize_returns_protocol_version() {
        let ctx = dummy_ctx();
        let resp = dispatch(&ctx, json!(1), "initialize", None).await;
        let result = resp.result.unwrap();
        assert_eq!(result["protocolVersion"], PROTOCOL_VERSION);
        assert_eq!(result["serverInfo"]["name"], "flockmux-swarm");
        assert!(result["capabilities"]["tools"].is_object());
    }

    #[tokio::test]
    async fn tools_list_returns_seven_tools() {
        let ctx = dummy_ctx();
        let resp = dispatch(&ctx, json!(2), "tools/list", None).await;
        let result = resp.result.unwrap();
        let arr = result["tools"].as_array().unwrap();
        assert_eq!(arr.len(), 7);
    }

    #[tokio::test]
    async fn unknown_method_returns_method_not_found() {
        let ctx = dummy_ctx();
        let resp = dispatch(&ctx, json!(3), "ping", None).await;
        let err = resp.error.unwrap();
        assert_eq!(err.code, METHOD_NOT_FOUND);
    }

    #[tokio::test]
    async fn tools_call_without_params_returns_invalid_params() {
        let ctx = dummy_ctx();
        let resp = dispatch(&ctx, json!(4), "tools/call", None).await;
        let err = resp.error.unwrap();
        assert_eq!(err.code, INVALID_PARAMS);
    }
}
