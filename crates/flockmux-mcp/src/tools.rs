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
            "description": "Send a message to another flockmux agent. Use this to coordinate with other agents in the swarm — share findings, request help, hand off work. The `from` field is set automatically to your own agent id. Pass `in_reply_to` with another message's id to thread a reply.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "to":   { "type": "string", "description": "Recipient agent id, e.g. 'claude-abc12345'. Use swarm_list_agents to discover ids." },
                    "kind": { "type": "string", "description": "Message kind label. Use 'note' for general comms, 'ask' to request something, 'reply' when answering." },
                    "body": { "type": "string", "description": "Message body (plain text or markdown)." },
                    "in_reply_to": { "type": "integer", "description": "Optional parent message id (see ids from swarm_list_messages). Threads this message as a reply.", "minimum": 1 }
                },
                "required": ["to", "kind", "body"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "swarm_list_messages",
            "description": "List recent messages addressed to you. Call this near the start of a task to pick up handoffs / context from other agents. By default returns the 20 most recent. SIDE EFFECT: messages returned are marked as read for you — the UI badge for their senders drops to zero after this call.",
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
        json!({
            "name": "swarm_list_spells",
            "description": "List every spell flockmux knows about (loaded from spells/ at server startup). Each spell entry includes its name, a short description, and the list of roles it will spawn (role/cli pairs). Use this BEFORE swarm_run_spell to discover what's available — names are case-sensitive.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }
        }),
        json!({
            "name": "swarm_run_spell",
            "description": "Launch a spell by name. Returns the freshly-spawned agent ids and roles. Use this to programmatically chain workflows — e.g. a planner agent that picks the right spell for a natural-language task, or a tree-executor that decomposes a problem and dispatches sub-spells. The spell's bootstrap prompt is auto-injected into each spawned PTY; you don't need to message the new agents yourself.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Spell name as returned by swarm_list_spells (e.g. 'fullstack-feature')." },
                    "task": { "type": "string", "description": "The task description that gets substituted into each agent's {task} placeholder. Be specific — this is the only context the spawned agents see beyond their role SOP." },
                    "workspace_dir": { "type": "string", "description": "Optional absolute path for shared_workspace spells (e.g. '/tmp/my-project'). Ignored by per-agent spells. Omit to let the server mint a fresh dir under ~/.flockmux/workspaces/." }
                },
                "required": ["name", "task"],
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
        "swarm_list_spells" => list_spells(ctx).await,
        "swarm_run_spell" => run_spell(ctx, args).await,
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
    let in_reply_to = arg_i64_opt(args, "in_reply_to");
    let mut payload = json!({
        "from": ctx.agent_id,
        "to": to,
        "kind": kind,
        "body": body,
    });
    if let Some(parent) = in_reply_to {
        payload["in_reply_to"] = json!(parent);
    }
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
    let mut out = format!(
        "Sent message #{id} from {} to {to} (kind={kind}, {} chars).",
        ctx.agent_id,
        body.chars().count(),
    );
    if let Some(parent) = in_reply_to {
        out.push_str(&format!(" In reply to #{parent}."));
    }
    Ok(out)
}

async fn list_messages(ctx: &ToolContext, args: &Value) -> Result<String, String> {
    let limit = arg_i64_opt(args, "limit").unwrap_or(DEFAULT_LIMIT);
    let only_undelivered = arg_bool_opt(args, "only_undelivered").unwrap_or(false);
    let rows = fetch_messages(ctx, limit, only_undelivered).await?;

    // The mark_read side effect only fires for messages addressed to us
    // (to_agent == ctx.agent_id) that have not yet been read. The REST
    // endpoint enforces the same restriction, but pre-filtering keeps the
    // request body tight when an agent passes only_undelivered or has been
    // CC'd via a future fan-out.
    let unread_ids: Vec<i64> = rows
        .iter()
        .filter(|m| {
            m.get("to_agent").and_then(|v| v.as_str()) == Some(ctx.agent_id.as_str())
                && m.get("read_at").map(|v| v.is_null()).unwrap_or(true)
        })
        .filter_map(|m| m.get("id").and_then(|v| v.as_i64()))
        .collect();

    let (marked_count, mark_err) = if unread_ids.is_empty() {
        (0usize, None)
    } else {
        match try_mark_read(ctx, &ctx.agent_id, unread_ids.clone()).await {
            Ok(n) => (n, None),
            Err(msg) => (0, Some(msg)),
        }
    };

    let unread_set: std::collections::HashSet<i64> = unread_ids.iter().copied().collect();
    let mut out = format_messages_with_state(&rows, &ctx.agent_id, only_undelivered, &unread_set);
    if marked_count > 0 {
        out.push_str(&format!(
            "\nMarked {marked_count} message(s) as read."
        ));
    }
    if let Some(err) = mark_err {
        out.push_str(&format!("\n(note: failed to mark read: {err})"));
    }
    Ok(out)
}

/// Pure fetch — no side effects. Returns the rows verbatim from the server.
async fn fetch_messages(
    ctx: &ToolContext,
    limit: i64,
    only_undelivered: bool,
) -> Result<Vec<Value>, String> {
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
    resp.json::<Vec<Value>>()
        .await
        .map_err(|e| format!("malformed response from {url}: {e}"))
}

/// Best-effort POST to /api/message/read. Returns the number of ids actually
/// marked (the server may report fewer than we asked for — idempotent). On
/// transport / HTTP error returns Err so the caller can surface a footnote.
async fn try_mark_read(
    ctx: &ToolContext,
    to: &str,
    ids: Vec<i64>,
) -> Result<usize, String> {
    let url = format!("{}/api/message/read", ctx.server_url);
    let resp = ctx
        .http
        .post(&url)
        .json(&json!({ "to": to, "ids": ids }))
        .send()
        .await
        .map_err(|e| format!("flockmux-server unreachable at {url}: {e}"))?;
    if !resp.status().is_success() {
        return Err(http_err_text(resp).await);
    }
    let body: Value = resp
        .json()
        .await
        .map_err(|e| format!("malformed response from {url}: {e}"))?;
    let marked = body
        .get("marked")
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    Ok(marked)
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
    // Search doesn't mark anything read; pass an empty set so every row
    // shows its persisted-state flag (✓ for read, ★ for still-unread).
    let empty: std::collections::HashSet<i64> = std::collections::HashSet::new();
    for (i, m) in rows.iter().take(limit as usize).enumerate() {
        let id = m.get("id").and_then(|v| v.as_i64()).unwrap_or(-1);
        let is_unread = m.get("read_at").map(|v| v.is_null()).unwrap_or(true);
        // For search output, ★ means "still unread" (persisted state),
        // not "freshly marked this call" — pass the id into the set so the
        // formatter renders ★ for it.
        let mut local = empty.clone();
        if is_unread {
            local.insert(id);
        }
        out.push_str(&format_message_line(i, m, &local));
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

// ── spell discovery / dispatch ───────────────────────────────────────────

/// `swarm_list_spells` — GET /api/spells, formatted for LLM consumption.
/// Includes the resolved (role, cli) pair per agent so the LLM can reason
/// about which CLI a spell will spawn for each slot. Empty registry returns
/// "No spells registered." rather than an error so a planner agent can still
/// surface that fact to the user.
async fn list_spells(ctx: &ToolContext) -> Result<String, String> {
    let url = format!("{}/api/spells", ctx.server_url);
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
        return Ok("No spells registered.".into());
    }
    let mut out = format!("{} spell(s) available:\n", rows.len());
    for s in &rows {
        let name = s.get("name").and_then(|v| v.as_str()).unwrap_or("?");
        let desc = s
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        let agents = s
            .get("agents")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .map(|x| {
                        let role = x.get("role").and_then(|v| v.as_str()).unwrap_or("?");
                        let cli = x.get("cli").and_then(|v| v.as_str()).unwrap_or("?");
                        format!("{role}:{cli}")
                    })
                    .collect::<Vec<_>>()
                    .join(" → ")
            })
            .unwrap_or_default();
        out.push_str(&format!("\n• {name}\n  agents: {agents}\n"));
        if !desc.is_empty() {
            out.push_str(&format!("  {desc}\n"));
        }
    }
    Ok(out)
}

/// `swarm_run_spell` — POST /api/spell/run. Mirrors the HTTP route's
/// RunSpellRequest shape: required `name` + `task`, optional `workspace_dir`
/// for shared-workspace spells. Returns the spawned agents so the caller can
/// reference them in subsequent messages or status checks. Errors from the
/// server (unknown spell name, depends_on cycle, role_ref resolution
/// failure, …) are surfaced verbatim so the LLM can decide whether to retry
/// with a different name / task / workspace.
async fn run_spell(ctx: &ToolContext, args: &Value) -> Result<String, String> {
    let name = arg_str(args, "name")?;
    let task = arg_str(args, "task")?;
    let workspace_dir = args.get("workspace_dir").and_then(|v| v.as_str());

    let mut payload = json!({ "name": name, "task": task });
    if let Some(wd) = workspace_dir.filter(|s| !s.is_empty()) {
        payload
            .as_object_mut()
            .expect("constructed as object")
            .insert("workspace_dir".into(), Value::String(wd.into()));
    }

    let url = format!("{}/api/spell/run", ctx.server_url);
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
    let body: Value = resp
        .json()
        .await
        .map_err(|e| format!("malformed response from {url}: {e}"))?;

    let spell_name = body.get("spell").and_then(|v| v.as_str()).unwrap_or(name);
    let agents = body
        .get("agents")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    if agents.is_empty() {
        return Ok(format!("Spell `{spell_name}` accepted but no agents reported."));
    }
    let mut out = format!(
        "Launched spell `{spell_name}` with {} agent(s):\n",
        agents.len()
    );
    for a in &agents {
        let role = a.get("role").and_then(|v| v.as_str()).unwrap_or("?");
        let cli = a.get("cli").and_then(|v| v.as_str()).unwrap_or("?");
        let id = a.get("agent_id").and_then(|v| v.as_str()).unwrap_or("?");
        out.push_str(&format!("  {role:>10} ({cli})  {id}\n"));
    }
    Ok(out)
}

// ── formatting helpers ───────────────────────────────────────────────────

/// Format with awareness of which ids are newly-read this call.
/// `★` = was unread coming in (will be / has been marked by this call).
/// `✓` = already read before this call (read_at != null at fetch time).
fn format_messages_with_state(
    rows: &[Value],
    me: &str,
    only_undelivered: bool,
    newly_read: &std::collections::HashSet<i64>,
) -> String {
    if rows.is_empty() {
        let qualifier = if only_undelivered { " undelivered" } else { "" };
        return format!("No{qualifier} messages for {me}.");
    }
    let new_count = newly_read.len();
    let read_count = rows.len() - new_count;
    let mut out = format!(
        "{} message(s) for {me} ({new_count} new, {read_count} already read):\n",
        rows.len()
    );
    for (i, m) in rows.iter().enumerate() {
        out.push_str(&format_message_line(i, m, newly_read));
        out.push('\n');
    }
    out
}

fn format_message_line(
    idx: usize,
    m: &Value,
    newly_read: &std::collections::HashSet<i64>,
) -> String {
    let id = m.get("id").and_then(|v| v.as_i64()).unwrap_or(-1);
    let from = m.get("from_agent").and_then(|v| v.as_str()).unwrap_or("?");
    let to = m.get("to_agent").and_then(|v| v.as_str()).unwrap_or("?");
    let kind = m.get("kind").and_then(|v| v.as_str()).unwrap_or("?");
    let body = m.get("body").and_then(|v| v.as_str()).unwrap_or("");
    let flag = if newly_read.contains(&id) { "★" } else { "✓" };
    let reply = m
        .get("in_reply_to")
        .and_then(|v| v.as_i64())
        .map(|p| format!("  ↩ #{p}"))
        .unwrap_or_default();
    format!("  {flag} [{idx}] #{id}  {from} → {to}  ({kind}){reply}\n      {body}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{extract::Path, routing::{get, post}, Json, Router};
    use serde_json::json;
    use std::net::SocketAddr;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    /// State shared by stub routes. `messages` is the canonical list of rows;
    /// `mark_read_calls` records every POST body so tests can assert exactly
    /// which ids the MCP tool sent.
    #[derive(Default)]
    struct StubState {
        messages: Vec<Value>,
        mark_read_calls: Vec<Value>,
    }

    /// Minimal stub of flockmux-server's REST surface. Each test starts one
    /// of these on a random port so we can hit the real reqwest client.
    fn build_app(state: Arc<Mutex<StubState>>) -> Router {
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
                        let id = (s.messages.len() as i64) + 1;
                        let row = json!({
                            "id": id,
                            "from_agent": body.get("from").cloned().unwrap_or(json!("system")),
                            "to_agent":   body.get("to"),
                            "kind":       body.get("kind"),
                            "body":       body.get("body"),
                            "sent_at": 1700,
                            "delivered_at": null,
                            "read_at": null,
                            "in_reply_to": body.get("in_reply_to").cloned().unwrap_or(Value::Null),
                        });
                        s.messages.push(row.clone());
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
                        let rows: Vec<Value> = s.messages.iter()
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
            .route("/api/message/read", post({
                let state = state.clone();
                move |Json(body): Json<Value>| {
                    let state = state.clone();
                    async move {
                        let mut s = state.lock().await;
                        s.mark_read_calls.push(body.clone());
                        let to = body.get("to").and_then(|v| v.as_str()).unwrap_or("");
                        let ids: Vec<i64> = body
                            .get("ids")
                            .and_then(|v| v.as_array())
                            .map(|a| a.iter().filter_map(|x| x.as_i64()).collect())
                            .unwrap_or_default();
                        let mut marked = Vec::new();
                        for m in s.messages.iter_mut() {
                            let id = m.get("id").and_then(|v| v.as_i64()).unwrap_or(-1);
                            let to_match = m.get("to_agent").and_then(|v| v.as_str()) == Some(to);
                            let unread = m.get("read_at").map(|v| v.is_null()).unwrap_or(true);
                            if to_match && unread && ids.contains(&id) {
                                m["read_at"] = json!(9999);
                                marked.push(id);
                            }
                        }
                        Json(json!({ "marked": marked, "at": 9999 }))
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
            .route("/api/spells", get(|| async {
                Json(json!([
                    { "name": "critic-loop",
                      "description": "writer → critic → editor",
                      "agents": [
                          { "role": "writer", "cli": "claude" },
                          { "role": "critic", "cli": "codex"  },
                          { "role": "editor", "cli": "claude" }
                      ] },
                    { "name": "fullstack-feature",
                      "description": "FE + BE parallel → test",
                      "agents": [
                          { "role": "frontend", "cli": "claude" },
                          { "role": "backend",  "cli": "codex"  },
                          { "role": "test",     "cli": "claude" }
                      ] }
                ]))
            }))
            .route("/api/spell/run", post({
                |Json(body): Json<Value>| async move {
                    let name = body.get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("?")
                        .to_string();
                    if name == "unknown-spell" {
                        return (
                            axum::http::StatusCode::NOT_FOUND,
                            Json(json!({"error": "spell `unknown-spell` not found"})),
                        );
                    }
                    (
                        axum::http::StatusCode::OK,
                        Json(json!({
                            "spell": name,
                            "agents": [
                                { "role": "writer", "cli": "claude", "agent_id": "claude-xxx" },
                                { "role": "critic", "cli": "codex",  "agent_id": "codex-yyy"  },
                            ]
                        })),
                    )
                }
            }))
    }

    async fn start_stub() -> (SocketAddr, Arc<Mutex<StubState>>) {
        let state: Arc<Mutex<StubState>> = Arc::new(Mutex::new(StubState::default()));
        let app = build_app(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (addr, state)
    }

    /// Helper: seed the stub with pre-existing rows so list_messages tests
    /// don't have to round-trip through send_message first.
    async fn seed_messages(state: &Arc<Mutex<StubState>>, rows: Vec<Value>) {
        let mut s = state.lock().await;
        s.messages = rows;
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
    async fn list_spells_renders_registry() {
        let (addr, _) = start_stub().await;
        let ctx = ctx_for(addr, "claude-aaa");
        let out = call_tool(&ctx, "swarm_list_spells", &json!({})).await;
        assert_eq!(out["isError"], json!(false));
        let text = out["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("2 spell(s) available"), "header missing: {text}");
        assert!(text.contains("critic-loop"));
        assert!(text.contains("fullstack-feature"));
        // Agent triples should be rendered as "role:cli → role:cli ..."
        assert!(text.contains("writer:claude → critic:codex → editor:claude"));
    }

    #[tokio::test]
    async fn run_spell_returns_spawned_agents() {
        let (addr, _) = start_stub().await;
        let ctx = ctx_for(addr, "claude-aaa");
        let out = call_tool(&ctx, "swarm_run_spell", &json!({
            "name": "critic-loop",
            "task": "haiku about Rust async cancellation"
        })).await;
        assert_eq!(out["isError"], json!(false));
        let text = out["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("Launched spell `critic-loop`"), "got: {text}");
        assert!(text.contains("claude-xxx"));
        assert!(text.contains("codex-yyy"));
    }

    #[tokio::test]
    async fn run_spell_surfaces_unknown_spell_error() {
        let (addr, _) = start_stub().await;
        let ctx = ctx_for(addr, "claude-aaa");
        let out = call_tool(&ctx, "swarm_run_spell", &json!({
            "name": "unknown-spell",
            "task": "anything"
        })).await;
        assert_eq!(out["isError"], json!(true));
        let text = out["content"][0]["text"].as_str().unwrap();
        // The stub returns a 404 with a descriptive body; we should surface it.
        assert!(text.to_lowercase().contains("unknown-spell"), "got: {text}");
    }

    #[tokio::test]
    async fn run_spell_missing_task_arg_returns_error() {
        let (addr, _) = start_stub().await;
        let ctx = ctx_for(addr, "claude-aaa");
        let out = call_tool(&ctx, "swarm_run_spell", &json!({
            "name": "critic-loop"
            // missing required `task`
        })).await;
        assert_eq!(out["isError"], json!(true));
        let text = out["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("missing required string field 'task'"), "got: {text}");
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
        // 7 original + 2 spells (M6c) = 9. Bump this when adding tools.
        assert_eq!(tools.len(), 9);
        for t in &tools {
            assert!(t["name"].is_string());
            assert!(t["description"].is_string());
            assert!(t["inputSchema"]["type"] == "object");
        }
        // Sanity: the two new M6c tools are wired into the descriptor list.
        let names: Vec<&str> = tools
            .iter()
            .filter_map(|t| t["name"].as_str())
            .collect();
        assert!(names.contains(&"swarm_list_spells"));
        assert!(names.contains(&"swarm_run_spell"));
    }

    fn make_row(id: i64, from: &str, to: &str, body: &str, read_at: Option<i64>, in_reply_to: Option<i64>) -> Value {
        json!({
            "id": id,
            "from_agent": from,
            "to_agent": to,
            "kind": "note",
            "body": body,
            "sent_at": 1000 + id,
            "delivered_at": null,
            "read_at": read_at,
            "in_reply_to": in_reply_to,
        })
    }

    #[tokio::test]
    async fn list_messages_marks_unread_subset_read() {
        let (addr, state) = start_stub().await;
        // Seed: two rows for codex-bbb, one unread (id=1) and one already
        // read (id=2 has read_at=500). The mark_read call should only carry
        // id=1.
        seed_messages(&state, vec![
            make_row(1, "claude-aaa", "codex-bbb", "fresh", None, None),
            make_row(2, "claude-aaa", "codex-bbb", "stale", Some(500), None),
        ]).await;

        let ctx = ctx_for(addr, "codex-bbb");
        let out = call_tool(&ctx, "swarm_list_messages", &json!({})).await;
        assert_eq!(out["isError"], json!(false));
        let text = out["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("2 message(s) for codex-bbb (1 new, 1 already read)"), "got: {text}");
        assert!(text.contains("Marked 1 message(s) as read."), "missing mark footer: {text}");

        let s = state.lock().await;
        assert_eq!(s.mark_read_calls.len(), 1, "exactly one POST /api/message/read");
        let call = &s.mark_read_calls[0];
        assert_eq!(call.get("to").and_then(|v| v.as_str()), Some("codex-bbb"));
        let ids: Vec<i64> = call.get("ids").and_then(|v| v.as_array()).unwrap()
            .iter().filter_map(|x| x.as_i64()).collect();
        assert_eq!(ids, vec![1], "only the unread id should be marked");
    }

    #[tokio::test]
    async fn list_messages_idempotent_when_all_already_read() {
        let (addr, state) = start_stub().await;
        seed_messages(&state, vec![
            make_row(1, "claude-aaa", "codex-bbb", "old1", Some(100), None),
            make_row(2, "claude-aaa", "codex-bbb", "old2", Some(200), None),
        ]).await;

        let ctx = ctx_for(addr, "codex-bbb");
        let out = call_tool(&ctx, "swarm_list_messages", &json!({})).await;
        let text = out["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("2 message(s) for codex-bbb (0 new, 2 already read)"), "got: {text}");
        assert!(!text.contains("Marked "), "no mark footer expected: {text}");

        let s = state.lock().await;
        assert_eq!(s.mark_read_calls.len(), 0, "no POST when nothing unread");
    }

    #[tokio::test]
    async fn send_message_in_reply_to_passes_through() {
        let (addr, state) = start_stub().await;
        let ctx = ctx_for(addr, "claude-aaa");
        let out = call_tool(&ctx, "swarm_send_message", &json!({
            "to": "codex-bbb",
            "kind": "reply",
            "body": "pong",
            "in_reply_to": 6
        })).await;
        assert_eq!(out["isError"], json!(false));
        let text = out["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("In reply to #6"), "missing reply footer: {text}");

        let s = state.lock().await;
        let row = s.messages.last().expect("stub recorded send");
        assert_eq!(row.get("in_reply_to").and_then(|v| v.as_i64()), Some(6));
    }

    #[tokio::test]
    async fn send_message_without_in_reply_to_omits_field() {
        let (addr, state) = start_stub().await;
        let ctx = ctx_for(addr, "claude-aaa");
        let out = call_tool(&ctx, "swarm_send_message", &json!({
            "to": "codex-bbb",
            "kind": "note",
            "body": "ping"
        })).await;
        let text = out["content"][0]["text"].as_str().unwrap();
        assert!(!text.contains("In reply to"), "should not advertise a reply: {text}");

        let s = state.lock().await;
        let row = s.messages.last().expect("stub recorded send");
        assert!(row.get("in_reply_to").map(|v| v.is_null()).unwrap_or(true),
            "in_reply_to should be null/absent: {row}");
    }

    #[tokio::test]
    async fn list_messages_renders_reply_lineage() {
        let (addr, state) = start_stub().await;
        // Order matches the real server: newest first.
        seed_messages(&state, vec![
            make_row(42, "claude-aaa", "codex-bbb", "child",  None,      Some(38)),
            make_row(38, "claude-aaa", "codex-bbb", "parent", Some(500), None),
        ]).await;

        let ctx = ctx_for(addr, "codex-bbb");
        let out = call_tool(&ctx, "swarm_list_messages", &json!({})).await;
        let text = out["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("↩ #38"), "reply marker missing: {text}");
        // The freshly-marked row is the child (#42); parent stays ✓.
        assert!(text.contains("★ [0] #42"), "star marker for unread child missing: {text}");
        assert!(text.contains("✓ [1] #38"), "check marker for read parent missing: {text}");
    }
}
