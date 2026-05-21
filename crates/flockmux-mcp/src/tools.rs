//! The seven swarm tools exposed over MCP. Each one is a thin wrapper that
//! formats inputs, calls the matching flockmux-server REST endpoint, and
//! folds the response into a human-readable `text` content block.
//!
//! Why text and not structured outputs:
//!   - claude / codex read the `text` field verbatim into the agent's
//!     context. A short, scannable string is far easier for the agent to
//!     reason over than a JSON blob it has to parse.
//!   - MCP `content` accepts structured fragments, but tool-using models
//!     are trained on text-heavy results.
//!
//! Errors:
//!   - HTTP / transport failures return `isError: true` with the reason.
//!     The agent should see "swarm server unreachable" and retry, not silently
//!     drop the request.

use anyhow::Result;
use reqwest::Client;
use serde_json::{json, Value};
use std::time::Duration;

/// Hard cap on the per-call HTTP budget. If flockmux-server is wedged we'd
/// rather the agent see a fast error than have its UI lock waiting for the
/// MCP subprocess to time out (claude's default is 60s).
const HTTP_TIMEOUT: Duration = Duration::from_secs(5);

/// Default `limit` for list operations when the caller doesn't pass one.
/// Twenty messages is enough to give the agent recent context without
/// flooding its prompt window.
const DEFAULT_LIMIT: i64 = 20;

#[derive(Clone)]
pub struct ToolContext {
    pub agent_id: String,
    pub server_url: String,
    pub http: Client,
}

impl ToolContext {
    pub fn new(agent_id: String, server_url: String) -> Result<Self> {
        let http = Client::builder()
            .timeout(HTTP_TIMEOUT)
            .build()?;
        Ok(Self {
            agent_id,
            server_url,
            http,
        })
    }
}

/// The seven tool descriptors served by `tools/list`. inputSchema is hand-
/// written JSON-Schema; we don't pull in `schemars` because the surface
/// is small and stable.
pub fn tool_descriptors() -> Vec<Value> {
    vec![
        json!({
            "name": "swarm_send_message",
            "description": "Send a message to another flockmux agent. Use this to coordinate with other agents in the swarm — share findings, request help, hand off work. The `from` field is set automatically to your own agent id.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "to":   { "type": "string", "description": "Recipient agent id, e.g. 'claude-abc12345'. Use swarm_list_agents to discover ids." },
                    "kind": { "type": "string", "description": "Message kind label. Use 'note' for general comms, 'ask' to request something, 'reply' when answering." },
                    "body": { "type": "string", "description": "Message body (plain text or markdown)." }
                },
                "required": ["to", "kind", "body"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "swarm_list_messages",
            "description": "List recent messages addressed to you. Call this near the start of a task to pick up handoffs / context from other agents. By default returns the 20 most recent.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "limit": { "type": "integer", "description": "Max rows to return (default 20).", "minimum": 1, "maximum": 200 },
                    "only_undelivered": { "type": "boolean", "description": "If true, only return messages not yet marked delivered." }
                },
                "additionalProperties": false
            }
        }),
        json!({
            "name": "swarm_search_messages",
            "description": "Full-text search across all swarm messages (FTS5, porter-stemmed). Use this to find prior discussion of a topic before duplicating work.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "q":     { "type": "string", "description": "Search query (single word or phrase)." },
                    "limit": { "type": "integer", "description": "Max rows (default 20).", "minimum": 1, "maximum": 200 }
                },
                "required": ["q"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "swarm_list_agents",
            "description": "List all known agents (live and historical). Use this to discover the recipient id before sending a message. Live agents have `killed_at = null`.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }
        }),
        json!({
            "name": "swarm_list_blackboard",
            "description": "List every known path on the shared blackboard with its latest sha256 + write timestamp. The blackboard is a shared filesystem under ~/.flockmux/blackboard — any agent can read or write it.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }
        }),
        json!({
            "name": "swarm_read_blackboard",
            "description": "Read the current content of a blackboard path. Returns NOT_FOUND if the path has never been written.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Relative path under the blackboard root, e.g. 'tasks.md' or 'notes/plan.md'. Must not contain '..' or absolute components." }
                },
                "required": ["path"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "swarm_write_blackboard",
            "description": "Write (or overwrite) a blackboard path with new content. The write is attributed to your agent id automatically. Other agents watching the blackboard see the update immediately.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path":    { "type": "string", "description": "Relative path under the blackboard root." },
                    "content": { "type": "string", "description": "New file content (UTF-8)." }
                },
                "required": ["path", "content"],
                "additionalProperties": false
            }
        }),
    ]
}

/// Top-level dispatch from `tools/call`. Returns the MCP `content` payload
/// (a JSON object with a `content` array, plus optional `isError: true`).
pub async fn call_tool(ctx: &ToolContext, name: &str, args: &Value) -> Value {
    let result = match name {
        "swarm_send_message" => send_message(ctx, args).await,
        "swarm_list_messages" => list_messages(ctx, args).await,
        "swarm_search_messages" => search_messages(ctx, args).await,
        "swarm_list_agents" => list_agents(ctx).await,
        "swarm_list_blackboard" => list_blackboard(ctx).await,
        "swarm_read_blackboard" => read_blackboard(ctx, args).await,
        "swarm_write_blackboard" => write_blackboard(ctx, args).await,
        other => Err(format!("unknown tool '{other}'")),
    };
    match result {
        Ok(text) => json!({
            "content": [{ "type": "text", "text": text }],
            "isError": false
        }),
        Err(msg) => json!({
            "content": [{ "type": "text", "text": msg }],
            "isError": true
        }),
    }
}

// ── helpers ──────────────────────────────────────────────────────────────

fn arg_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("missing required string field '{key}'"))
}

fn arg_i64_opt(args: &Value, key: &str) -> Option<i64> {
    args.get(key).and_then(|v| v.as_i64())
}

fn arg_bool_opt(args: &Value, key: &str) -> Option<bool> {
    args.get(key).and_then(|v| v.as_bool())
}

async fn http_err_text(resp: reqwest::Response) -> String {
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    format!("HTTP {status}: {body}")
}

// ── tool implementations ─────────────────────────────────────────────────

async fn send_message(ctx: &ToolContext, args: &Value) -> Result<String, String> {
    let to = arg_str(args, "to")?;
    let kind = arg_str(args, "kind")?;
    let body = arg_str(args, "body")?;
    let payload = json!({
        "from": ctx.agent_id,
        "to": to,
        "kind": kind,
        "body": body,
    });
    let url = format!("{}/api/message", ctx.server_url);
    let resp = ctx
        .http
        .post(&url)
        .json(&payload)
        .send()
        .await
        .map_err(|e| format!("flockmux-server unreachable at {url}: {e}"))?;
    if !resp.status().is_success() {
        return Err(http_err_text(resp).await);
    }
    let value: Value = resp
        .json()
        .await
        .map_err(|e| format!("malformed response from {url}: {e}"))?;
    let id = value.get("id").and_then(|v| v.as_i64()).unwrap_or(-1);
    Ok(format!(
        "Sent message #{id} from {} to {to} (kind={kind}, {} chars).",
        ctx.agent_id,
        body.chars().count(),
    ))
}

async fn list_messages(ctx: &ToolContext, args: &Value) -> Result<String, String> {
    let limit = arg_i64_opt(args, "limit").unwrap_or(DEFAULT_LIMIT);
    let only_undelivered = arg_bool_opt(args, "only_undelivered").unwrap_or(false);
    let url = format!("{}/api/message", ctx.server_url);
    let resp = ctx
        .http
        .get(&url)
        .query(&[
            ("to", ctx.agent_id.as_str()),
            ("limit", &limit.to_string()),
            ("only_undelivered", if only_undelivered { "true" } else { "false" }),
        ])
        .send()
        .await
        .map_err(|e| format!("flockmux-server unreachable at {url}: {e}"))?;
    if !resp.status().is_success() {
        return Err(http_err_text(resp).await);
    }
    let rows: Vec<Value> = resp
        .json()
        .await
        .map_err(|e| format!("malformed response from {url}: {e}"))?;
    Ok(format_messages(&rows, &ctx.agent_id, only_undelivered))
}

async fn search_messages(ctx: &ToolContext, args: &Value) -> Result<String, String> {
    let q = arg_str(args, "q")?;
    let limit = arg_i64_opt(args, "limit").unwrap_or(DEFAULT_LIMIT);
    let url = format!("{}/api/message", ctx.server_url);
    let resp = ctx
        .http
        .get(&url)
        .query(&[("q", q), ("limit", &limit.to_string())])
        .send()
        .await
        .map_err(|e| format!("flockmux-server unreachable at {url}: {e}"))?;
    if !resp.status().is_success() {
        return Err(http_err_text(resp).await);
    }
    let rows: Vec<Value> = resp
        .json()
        .await
        .map_err(|e| format!("malformed response from {url}: {e}"))?;
    if rows.is_empty() {
        return Ok(format!("No messages match query '{q}'."));
    }
    let mut out = format!("Found {} message(s) matching '{q}':\n", rows.len());
    for (i, m) in rows.iter().take(limit as usize).enumerate() {
        out.push_str(&format_message_line(i, m));
        out.push('\n');
    }
    Ok(out)
}

async fn list_agents(ctx: &ToolContext) -> Result<String, String> {
    let url = format!("{}/api/agent", ctx.server_url);
    let resp = ctx
        .http
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("flockmux-server unreachable at {url}: {e}"))?;
    if !resp.status().is_success() {
        return Err(http_err_text(resp).await);
    }
    let rows: Vec<Value> = resp
        .json()
        .await
        .map_err(|e| format!("malformed response from {url}: {e}"))?;
    if rows.is_empty() {
        return Ok("No agents registered.".into());
    }
    let mut out = format!("{} agent(s) known (you are {}):\n", rows.len(), ctx.agent_id);
    for a in &rows {
        let id = a.get("agent_id").and_then(|v| v.as_str()).unwrap_or("?");
        let cli = a.get("cli").and_then(|v| v.as_str()).unwrap_or("?");
        let role = a.get("role").and_then(|v| v.as_str()).unwrap_or("?");
        let killed = a.get("killed_at").and_then(|v| v.as_i64()).is_some();
        let marker = if killed { "✗" } else { "●" };
        let me = if id == ctx.agent_id { " (you)" } else { "" };
        out.push_str(&format!("  {marker} {id}  cli={cli}  role={role}{me}\n"));
    }
    Ok(out)
}

async fn list_blackboard(ctx: &ToolContext) -> Result<String, String> {
    let url = format!("{}/api/blackboard", ctx.server_url);
    let resp = ctx
        .http
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("flockmux-server unreachable at {url}: {e}"))?;
    if !resp.status().is_success() {
        return Err(http_err_text(resp).await);
    }
    let rows: Vec<Value> = resp
        .json()
        .await
        .map_err(|e| format!("malformed response from {url}: {e}"))?;
    if rows.is_empty() {
        return Ok("Blackboard is empty.".into());
    }
    let mut out = format!("Blackboard has {} path(s):\n", rows.len());
    for r in &rows {
        let path = r.get("path").and_then(|v| v.as_str()).unwrap_or("?");
        let sha = r
            .get("sha256")
            .and_then(|v| v.as_str())
            .map(|s| &s[..s.len().min(12)])
            .unwrap_or("?");
        let op = r.get("op").and_then(|v| v.as_str()).unwrap_or("?");
        out.push_str(&format!("  {path}  ({op}, sha {sha}…)\n"));
    }
    Ok(out)
}

async fn read_blackboard(ctx: &ToolContext, args: &Value) -> Result<String, String> {
    let path = arg_str(args, "path")?;
    let url = format!("{}/api/blackboard/{}", ctx.server_url, path);
    let resp = ctx
        .http
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("flockmux-server unreachable at {url}: {e}"))?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Err(format!("blackboard path not found: {path}"));
    }
    if !resp.status().is_success() {
        return Err(http_err_text(resp).await);
    }
    let snap: Value = resp
        .json()
        .await
        .map_err(|e| format!("malformed response from {url}: {e}"))?;
    let content = snap.get("content").and_then(|v| v.as_str()).unwrap_or("");
    let sha = snap.get("sha256").and_then(|v| v.as_str()).unwrap_or("?");
    let sha_short = &sha[..sha.len().min(12)];
    Ok(format!(
        "[blackboard:{path}, sha {sha_short}…, {} bytes]\n{content}",
        content.len()
    ))
}

async fn write_blackboard(ctx: &ToolContext, args: &Value) -> Result<String, String> {
    let path = arg_str(args, "path")?;
    let content = arg_str(args, "content")?;
    let payload = json!({
        "agent_id": ctx.agent_id,
        "content": content,
    });
    let url = format!("{}/api/blackboard/{}", ctx.server_url, path);
    let resp = ctx
        .http
        .put(&url)
        .json(&payload)
        .send()
        .await
        .map_err(|e| format!("flockmux-server unreachable at {url}: {e}"))?;
    if !resp.status().is_success() {
        return Err(http_err_text(resp).await);
    }
    let resp_json: Value = resp.json().await.unwrap_or(Value::Null);
    let sha = resp_json
        .get("sha256")
        .and_then(|v| v.as_str())
        .unwrap_or("?");
    let sha_short = &sha[..sha.len().min(12)];
    Ok(format!(
        "Wrote {} bytes to {path} (sha {sha_short}…).",
        content.len()
    ))
}

// ── formatting helpers ───────────────────────────────────────────────────

fn format_messages(rows: &[Value], me: &str, only_undelivered: bool) -> String {
    if rows.is_empty() {
        let qualifier = if only_undelivered { " undelivered" } else { "" };
        return format!("No{qualifier} messages for {me}.");
    }
    let mut out = format!("{} message(s) for {me}:\n", rows.len());
    for (i, m) in rows.iter().enumerate() {
        out.push_str(&format_message_line(i, m));
        out.push('\n');
    }
    out
}

fn format_message_line(idx: usize, m: &Value) -> String {
    let id = m.get("id").and_then(|v| v.as_i64()).unwrap_or(-1);
    let from = m.get("from_agent").and_then(|v| v.as_str()).unwrap_or("?");
    let to = m.get("to_agent").and_then(|v| v.as_str()).unwrap_or("?");
    let kind = m.get("kind").and_then(|v| v.as_str()).unwrap_or("?");
    let body = m.get("body").and_then(|v| v.as_str()).unwrap_or("");
    let delivered = m
        .get("delivered_at")
        .map(|v| !v.is_null())
        .unwrap_or(false);
    let flag = if delivered { " " } else { "★" };
    format!("  {flag} [{idx}] #{id}  {from} → {to}  ({kind})\n      {body}", )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{extract::Path, routing::{get, post}, Json, Router};
    use serde_json::json;
    use std::net::SocketAddr;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    /// Minimal stub of flockmux-server's REST surface. Each test starts one
    /// of these on a random port so we can hit the real reqwest client.
    fn build_app(state: Arc<Mutex<Vec<Value>>>) -> Router {
        Router::new()
            .route("/api/agent", get({
                || async {
                    Json(json!([
                        { "agent_id": "claude-aaa", "cli": "claude", "role": "claude",
                          "workspace": "/tmp/a", "shim_ready": true, "shim_exit": null,
                          "killed_at": null, "spawned_at": 1 },
                        { "agent_id": "codex-bbb",  "cli": "codex",  "role": "codex",
                          "workspace": "/tmp/b", "shim_ready": true, "shim_exit": 0,
                          "killed_at": 9, "spawned_at": 1 }
                    ]))
                }
            }))
            .route("/api/message", post({
                let state = state.clone();
                move |Json(body): Json<Value>| {
                    let state = state.clone();
                    async move {
                        let mut s = state.lock().await;
                        let id = (s.len() as i64) + 1;
                        let row = json!({
                            "id": id,
                            "from_agent": body.get("from").cloned().unwrap_or(json!("system")),
                            "to_agent":   body.get("to"),
                            "kind":       body.get("kind"),
                            "body":       body.get("body"),
                            "sent_at": 1700,
                            "delivered_at": null,
                            "read_at": null,
                        });
                        s.push(row.clone());
                        Json(row)
                    }
                }
            }).get({
                let state = state.clone();
                move |axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String, String>>| {
                    let state = state.clone();
                    async move {
                        let s = state.lock().await;
                        let to = q.get("to").cloned();
                        let rows: Vec<Value> = s.iter()
                            .filter(|m| match &to {
                                Some(t) => m.get("to_agent").and_then(|v| v.as_str()) == Some(t.as_str()),
                                None => true,
                            })
                            .cloned()
                            .collect();
                        Json(rows)
                    }
                }
            }))
            .route("/api/blackboard", get(|| async {
                Json(json!([
                    { "path": "tasks.md", "sha256": "abcd1234ef", "at": 1, "op": "write" }
                ]))
            }))
            .route("/api/blackboard/*path", get({
                |Path(path): Path<String>| async move {
                    Json(json!({
                        "path": path,
                        "content": "hello blackboard",
                        "sha256": "deadbeefcafe1234",
                        "at": 2
                    }))
                }
            }).put({
                |Path(path): Path<String>, Json(body): Json<Value>| async move {
                    let content = body.get("content").and_then(|v| v.as_str()).unwrap_or("");
                    Json(json!({
                        "id": 1,
                        "path": path,
                        "sha256": format!("sha-of-{}", content.len()),
                        "at": 3
                    }))
                }
            }))
    }

    async fn start_stub() -> (SocketAddr, Arc<Mutex<Vec<Value>>>) {
        let state: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
        let app = build_app(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (addr, state)
    }

    fn ctx_for(addr: SocketAddr, agent_id: &str) -> ToolContext {
        ToolContext::new(
            agent_id.into(),
            format!("http://{addr}"),
        ).unwrap()
    }

    #[tokio::test]
    async fn send_then_list_round_trip() {
        let (addr, _state) = start_stub().await;
        let ctx = ctx_for(addr, "claude-aaa");

        let out = call_tool(&ctx, "swarm_send_message", &json!({
            "to": "codex-bbb",
            "kind": "note",
            "body": "hello bb"
        })).await;
        assert_eq!(out["isError"], json!(false));
        let text = out["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("Sent message #1"));
        assert!(text.contains("to codex-bbb"));

        // List as codex-bbb — should see the message.
        let ctx_b = ctx_for(addr, "codex-bbb");
        let out = call_tool(&ctx_b, "swarm_list_messages", &json!({})).await;
        assert_eq!(out["isError"], json!(false));
        let text = out["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("1 message(s) for codex-bbb"), "got: {text}");
        assert!(text.contains("hello bb"));
    }

    #[tokio::test]
    async fn list_agents_marks_self_and_dead() {
        let (addr, _) = start_stub().await;
        let ctx = ctx_for(addr, "claude-aaa");
        let out = call_tool(&ctx, "swarm_list_agents", &json!({})).await;
        let text = out["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("claude-aaa") && text.contains("(you)"));
        assert!(text.contains("codex-bbb"));
        assert!(text.contains("✗"), "killed agent marker missing: {text}");
    }

    #[tokio::test]
    async fn read_blackboard_returns_content() {
        let (addr, _) = start_stub().await;
        let ctx = ctx_for(addr, "claude-aaa");
        let out = call_tool(&ctx, "swarm_read_blackboard", &json!({
            "path": "tasks.md"
        })).await;
        assert_eq!(out["isError"], json!(false));
        let text = out["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("hello blackboard"));
        assert!(text.contains("blackboard:tasks.md"));
    }

    #[tokio::test]
    async fn write_blackboard_reports_sha() {
        let (addr, _) = start_stub().await;
        let ctx = ctx_for(addr, "claude-aaa");
        let out = call_tool(&ctx, "swarm_write_blackboard", &json!({
            "path": "notes/plan.md",
            "content": "- [ ] step one"
        })).await;
        assert_eq!(out["isError"], json!(false));
        let text = out["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("Wrote 14 bytes to notes/plan.md"), "got: {text}");
    }

    #[tokio::test]
    async fn unknown_tool_returns_error() {
        let (addr, _) = start_stub().await;
        let ctx = ctx_for(addr, "claude-aaa");
        let out = call_tool(&ctx, "swarm_nope", &json!({})).await;
        assert_eq!(out["isError"], json!(true));
    }

    #[tokio::test]
    async fn missing_required_arg_returns_error() {
        let (addr, _) = start_stub().await;
        let ctx = ctx_for(addr, "claude-aaa");
        let out = call_tool(&ctx, "swarm_send_message", &json!({
            "to": "codex-bbb"
            // missing kind + body
        })).await;
        assert_eq!(out["isError"], json!(true));
        let text = out["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("missing required string field 'kind'"));
    }

    #[tokio::test]
    async fn unreachable_server_returns_error() {
        // Use a port we know nothing is listening on.
        let ctx = ToolContext::new(
            "claude-aaa".into(),
            "http://127.0.0.1:1".into(),
        ).unwrap();
        let out = call_tool(&ctx, "swarm_list_agents", &json!({})).await;
        assert_eq!(out["isError"], json!(true));
        let text = out["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("unreachable"));
    }

    #[test]
    fn tool_descriptors_have_required_fields() {
        let tools = tool_descriptors();
        assert_eq!(tools.len(), 7);
        for t in &tools {
            assert!(t["name"].is_string());
            assert!(t["description"].is_string());
            assert!(t["inputSchema"]["type"] == "object");
        }
    }
}
