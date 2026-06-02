//! The eight swarm tools exposed over MCP. Each one is a thin wrapper that
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

/// The eight tool descriptors served by `tools/list`. inputSchema is hand-
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
            "name": "swarm_spawn_worker",
            "description": "Spawn a worker agent for a registry ROLE. This is the primary way an orchestrator delegates real work. Pick a role (call swarm_list_roles to see the catalog) — the role supplies the default CLI + model tier + tool affordances, so you usually omit `cli`/`model`. Declare dependencies TYPED via `consumes` ({from_role, kind}); the server mints the canonical blackboard handoff key and validates the whole dependency graph at spawn time — NEVER hand-type blackboard keys. The minted key your worker must write when done is appended to its system prompt automatically. Returns the new agent_id.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "role": {
                        "type": "string",
                        "description": "Role slug from the registry (e.g. frontend, backend, reviewer, test-runner, docs-writer, researcher, fixer). Call swarm_list_roles to see each role's when_to_use + defaults. An unknown slug is rejected with the valid options (no silent mis-route)."
                    },
                    "system_prompt": {
                        "type": "string",
                        "description": "Full system prompt for the worker: (1) the concrete task, (2) which files / cwd to operate on, (3) explicit STOP instruction. Do NOT write blackboard-key plumbing — the server appends the exact minted handoff key for you. Write it like briefing a contractor: concrete, terse, no fluff."
                    },
                    "cli": {
                        "type": "string",
                        "description": "Optional CLI override (claude or codex). Omit to use the role's default_cli. Only set this to deliberately deviate from the role.",
                        "enum": ["claude", "codex"]
                    },
                    "model": {
                        "type": "string",
                        "description": "Optional model override, e.g. 'opus' / 'sonnet'. Omit to use the role's default_model_tier. Use to match model strength to task weight WITHOUT changing cli."
                    },
                    "produces": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Optional typed output-kinds this worker produces (e.g. ['done'] or ['spec','done']). Omit to use the role's declared produces (defaults to ['done']). The server mints one blackboard key per kind."
                    },
                    "consumes": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "from_role": {"type": "string", "description": "Upstream role slug whose output this worker waits on."},
                                "kind": {"type": "string", "description": "Output-kind of that upstream role. Defaults to 'done'."}
                            },
                            "required": ["from_role"],
                            "additionalProperties": false
                        },
                        "description": "Typed upstream dependencies. Instead of hand-typing blackboard keys, reference {from_role, kind}; the server resolves each to the producer's minted key, validates the producer exists & produces that kind (rejects unknown/typo with did-you-mean), and wires WakeCoordinator. Empty = start immediately."
                    }
                },
                "required": ["role", "system_prompt"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "swarm_list_roles",
            "description": "List the role registry catalog: each role's slug, when_to_use hint, default CLI + model tier, and produced output-kinds. Call this before swarm_spawn_worker to pick the right role instead of guessing cli/model. Read-only.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }
        }),
        json!({
            "name": "swarm_name_thread",
            "description": "Give THIS direction (thread) a short human name, ONCE, right after you read the user's first message. On a git project this also silently starts file isolation (a private git worktree) so this direction can't clobber another direction's working tree — the user never sees git/branches/checkout. No-op if you're on the workspace's main direction. Pick a 2-4 word lowercase name describing the goal, e.g. 'dark mode', 'payment retry', 'api v2 migration'.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Short human-readable direction name (2-4 words, e.g. 'dark mode'). Automatically derived into a filesystem-friendly slug + git branch."
                    }
                },
                "required": ["name"],
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
        "swarm_spawn_worker" => spawn_worker(ctx, args).await,
        "swarm_list_roles" => list_roles(ctx).await,
        "swarm_name_thread" => name_thread(ctx, args).await,
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
    //
    // M6f bug fix (2026-05-24): we deliberately EXCLUDE `kind="wake"` from
    // the auto-mark-read set. Wake messages are system triggers consumed
    // by `wake_check` (Stop-hook helper) — NOT human-readable mail. If
    // the LLM calls swarm_list_messages mid-turn (e.g. to check for new
    // input before stopping), any wake that landed during that turn would
    // get marked read *before* wake_check has a chance to see it on the
    // next Stop, silently dropping the wake. Observed in 2026-05-23
    // strict e2e #6: critic ate frontend.done in turn 1, mid-turn called
    // list_messages, picked up the just-arrived backend.done wake, marked
    // it read, then stop-hook fired with unread=0 → noop → critic idled
    // for 46 minutes until a manual ⚡ wake. The wake is still RETURNED
    // in the response so the LLM can see it; it's just not auto-read.
    // `wake_check` itself marks the wake read when it emits `block`,
    // which is the moment the wake's reason is actually delivered to
    // the LLM's next turn.
    let unread_ids: Vec<i64> = rows
        .iter()
        .filter(|m| {
            m.get("to_agent").and_then(|v| v.as_str()) == Some(ctx.agent_id.as_str())
                && m.get("read_at").map(|v| v.is_null()).unwrap_or(true)
                && m.get("kind").and_then(|v| v.as_str()) != Some("wake")
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

// ── ad-hoc worker spawn ──────────────────────────────────────────────────

/// `swarm_spawn_worker` — POST /api/worker. Magentic-One 重构后 orchestrator
/// 直接拉 worker 的入口,不走 spell + role 模板。caller_agent_id 自动从
/// ToolContext 注入,workspace_id 由 server 反查得到。
async fn spawn_worker(ctx: &ToolContext, args: &Value) -> Result<String, String> {
    let role = arg_str(args, "role")?;
    let system_prompt = arg_str(args, "system_prompt")?;
    // Optional CLI override — omitted means "use the role's default_cli".
    let cli = args.get("cli").and_then(|v| v.as_str());
    // Optional model overlay — omitted means "use the role's default_model_tier".
    let model = args.get("model").and_then(|v| v.as_str());
    // Typed output-kinds; empty forwards as "use the role's produces".
    let produces: Vec<String> = args
        .get("produces")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    // Typed upstream deps {from_role, kind} — forwarded verbatim; the server
    // resolves them to minted keys and validates the dependency graph.
    let consumes: Vec<Value> = args
        .get("consumes")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    // Reverse-resolve our own workspace_id via /api/agent (server attaches
    // workspace_id to every AgentInfo). caller_agent_id is the orchestrator
    // that just called this tool (or an upstream worker that's delegating
    // further). Avoid making the LLM care about workspace_id plumbing.
    let agents_url = format!("{}/api/agent", ctx.server_url);
    let agents_resp = ctx
        .http
        .get(&agents_url)
        .send()
        .await
        .map_err(|e| format!("flockmux-server unreachable at {agents_url}: {e}"))?;
    if !agents_resp.status().is_success() {
        return Err(http_err_text(agents_resp).await);
    }
    let agents_body: Vec<Value> = agents_resp
        .json()
        .await
        .map_err(|e| format!("malformed response from {agents_url}: {e}"))?;
    let workspace_id = agents_body
        .iter()
        .find(|a| a.get("agent_id").and_then(|v| v.as_str()) == Some(ctx.agent_id.as_str()))
        .and_then(|a| a.get("workspace_id").and_then(|v| v.as_str()))
        .map(|s| s.to_string())
        .ok_or_else(|| {
            format!(
                "could not resolve workspace_id for caller agent `{}` — \
                 agent missing from /api/agent or has no workspace_id (pre-Step3 spawn?)",
                ctx.agent_id
            )
        })?;

    let payload = json!({
        "role": role,
        "system_prompt": system_prompt,
        "cli": cli,        // Option → null when omitted; server falls back to role.default_cli
        "model": model,    // Option → null when omitted; server falls back to role.default_model_tier
        "produces": produces,
        "consumes": consumes,
        "caller_agent_id": ctx.agent_id,
        "workspace_id": workspace_id,
    });

    let url = format!("{}/api/worker", ctx.server_url);
    let resp = ctx
        .http
        .post(&url)
        .json(&payload)
        .send()
        .await
        .map_err(|e| format!("flockmux-server unreachable at {url}: {e}"))?;
    if !resp.status().is_success() {
        // The server returns 400 with valid options + did-you-mean on an
        // unknown role or an unresolvable consumes ref — surface it verbatim.
        return Err(http_err_text(resp).await);
    }
    let body: Value = resp
        .json()
        .await
        .map_err(|e| format!("malformed response from {url}: {e}"))?;

    let agent_id = body.get("agent_id").and_then(|v| v.as_str()).unwrap_or("?");
    let resolved_cli = body.get("cli").and_then(|v| v.as_str()).unwrap_or("?");
    let role_label = body.get("role_label").and_then(|v| v.as_str()).unwrap_or(role);
    let handoff = body.get("handoff_signal").and_then(|v| v.as_str()).unwrap_or("");
    let deps: Vec<&str> = body
        .get("depends_on")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();
    Ok(format!(
        "Spawned worker `{role_label}` ({resolved_cli}) agent_id={agent_id}\n\
         handoff_signal={}  (server-minted — your worker was told to write this)\n\
         depends_on={deps:?}",
        if handoff.is_empty() { "(none)" } else { handoff }
    ))
}

/// `swarm_list_roles` — GET /api/roles. Returns the role catalog so the
/// orchestrator can pick a role (and inherit its cli/model defaults) instead
/// of guessing. Read-only.
async fn list_roles(ctx: &ToolContext) -> Result<String, String> {
    let url = format!("{}/api/roles", ctx.server_url);
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
        return Ok("No roles registered.".to_string());
    }
    let mut out = format!("{} role(s):\n", rows.len());
    for r in &rows {
        let id = r.get("id").and_then(|v| v.as_str()).unwrap_or("?");
        let cli = r.get("default_cli").and_then(|v| v.as_str()).unwrap_or("?");
        let tier = r.get("default_model_tier").and_then(|v| v.as_str()).unwrap_or("");
        let when = r.get("when_to_use").and_then(|v| v.as_str()).unwrap_or("");
        let produces: Vec<&str> = r
            .get("produces")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
            .unwrap_or_default();
        out.push_str(&format!(
            "  • {id}  [cli={cli}{}]  produces={produces:?}\n      {when}\n",
            if tier.is_empty() { String::new() } else { format!(", model={tier}") }
        ));
    }
    Ok(out)
}

/// Name (and thereby isolate) the caller's current direction. The orchestrator
/// calls this once, after reading the first user message, to give the direction
/// a human label — which on a git project also kicks off background worktree
/// isolation. `thread_id` + `workspace_id` are reverse-resolved from
/// `/api/agent`; the agent never plumbs them.
async fn name_thread(ctx: &ToolContext, args: &Value) -> Result<String, String> {
    let name = arg_str(args, "name")?;

    // Reverse-resolve our own workspace_id + thread_id via /api/agent (server
    // attaches both to every AgentInfo).
    let agents_url = format!("{}/api/agent", ctx.server_url);
    let agents_resp = ctx
        .http
        .get(&agents_url)
        .send()
        .await
        .map_err(|e| format!("flockmux-server unreachable at {agents_url}: {e}"))?;
    if !agents_resp.status().is_success() {
        return Err(http_err_text(agents_resp).await);
    }
    let agents_body: Vec<Value> = agents_resp
        .json()
        .await
        .map_err(|e| format!("malformed response from {agents_url}: {e}"))?;
    let me = agents_body
        .iter()
        .find(|a| a.get("agent_id").and_then(|v| v.as_str()) == Some(ctx.agent_id.as_str()))
        .ok_or_else(|| format!("could not find caller agent `{}` in /api/agent", ctx.agent_id))?;
    let workspace_id = me
        .get("workspace_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "caller agent has no workspace_id (pre-Step3 spawn?)".to_string())?;
    let thread_id = match me.get("thread_id").and_then(|v| v.as_str()) {
        Some(tid) => tid,
        None => {
            // No thread row = the workspace's main direction. Main IS the
            // project and is never renamed/isolated — no-op so the orchestrator
            // doesn't treat it as an error.
            return Ok(format!(
                "You're on the main direction — it can't be renamed or isolated (it IS the \
                 project). Naming only applies to additional directions. (requested: {name:?})"
            ));
        }
    };

    let url = format!(
        "{}/api/workspaces/{}/threads/{}",
        ctx.server_url, workspace_id, thread_id
    );
    let resp = ctx
        .http
        .patch(&url)
        .json(&json!({ "name": name }))
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
    let slug = body.get("slug").and_then(|v| v.as_str()).unwrap_or("?");
    let state = body.get("state").and_then(|v| v.as_str()).unwrap_or("?");
    let isolation = body.get("isolation").and_then(|v| v.as_str()).unwrap_or("?");
    Ok(format!(
        "Named this direction `{name}` (slug `{slug}`, state={state}, isolation={isolation}). \
         On a git project, file isolation (a worktree) is prepared in the background — just \
         keep working."
    ))
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
        // 7 swarm primitives + swarm_spawn_worker (派活入口) + swarm_list_roles
        // (P0 角色目录) + swarm_name_thread (multi-direction naming/isolation) = 10.
        // Bump this when adding tools.
        assert_eq!(tools.len(), 10);
        for t in &tools {
            assert!(t["name"].is_string());
            assert!(t["description"].is_string());
            assert!(t["inputSchema"]["type"] == "object");
        }
        let names: Vec<&str> = tools
            .iter()
            .filter_map(|t| t["name"].as_str())
            .collect();
        assert!(names.contains(&"swarm_spawn_worker"));
        assert!(names.contains(&"swarm_list_roles"));
        assert!(names.contains(&"swarm_name_thread"));
        // 老 spell 工具已经退役;orchestrator 直接 spawn_worker。
        assert!(!names.contains(&"swarm_list_spells"));
        assert!(!names.contains(&"swarm_run_spell"));
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

    /// M6f variant of `make_row` that builds a wake message. Wake
    /// messages must NOT be auto-marked-read by list_messages.
    fn make_wake_row(id: i64, from: &str, to: &str, body: &str) -> Value {
        json!({
            "id": id,
            "from_agent": from,
            "to_agent": to,
            "kind": "wake",
            "body": body,
            "sent_at": 1000 + id,
            "delivered_at": null,
            "read_at": null,
            "in_reply_to": null,
        })
    }

    #[tokio::test]
    async fn list_messages_does_not_mark_wake_messages_read() {
        // M6f regression: wake messages were getting auto-marked read by
        // mid-turn list_messages calls, hiding them from the Stop-hook's
        // wake_check. The fix: list_messages skips kind="wake" in its
        // mark-read filter.
        let (addr, state) = start_stub().await;
        // Mix: one unread note (id=1), one unread wake (id=2). Only id=1
        // should be in the mark_read POST body.
        seed_messages(&state, vec![
            make_row(1, "claude-aaa", "codex-bbb", "human note", None, None),
            make_wake_row(2, "system", "codex-bbb", "blackboard `x` updated"),
        ]).await;

        let ctx = ctx_for(addr, "codex-bbb");
        let out = call_tool(&ctx, "swarm_list_messages", &json!({})).await;
        assert_eq!(out["isError"], json!(false));
        let text = out["content"][0]["text"].as_str().unwrap();
        // Both rows are returned (so LLM sees the wake context).
        assert!(text.contains("2 message(s) for codex-bbb"), "got: {text}");
        // Only the note got marked read.
        assert!(text.contains("Marked 1 message(s) as read."), "got: {text}");

        let s = state.lock().await;
        assert_eq!(s.mark_read_calls.len(), 1);
        let ids: Vec<i64> = s.mark_read_calls[0]
            .get("ids").and_then(|v| v.as_array()).unwrap()
            .iter().filter_map(|x| x.as_i64()).collect();
        assert_eq!(ids, vec![1], "only note (id=1) marked read; wake (id=2) untouched");
    }

    #[tokio::test]
    async fn list_messages_skips_mark_read_when_all_unread_are_wakes() {
        // M6f: when the unread set is entirely wakes, no mark_read POST
        // should fire at all.
        let (addr, state) = start_stub().await;
        seed_messages(&state, vec![
            make_wake_row(1, "system", "codex-bbb", "blackboard `a` updated"),
            make_wake_row(2, "system", "codex-bbb", "blackboard `b` updated"),
        ]).await;

        let ctx = ctx_for(addr, "codex-bbb");
        let out = call_tool(&ctx, "swarm_list_messages", &json!({})).await;
        let text = out["content"][0]["text"].as_str().unwrap();
        // Both wakes returned to the LLM…
        assert!(text.contains("2 message(s) for codex-bbb"), "got: {text}");
        // …but nothing marked read (so wake_check still sees them next Stop).
        assert!(!text.contains("Marked"), "must not mark any wakes: {text}");

        let s = state.lock().await;
        assert_eq!(s.mark_read_calls.len(), 0, "no POST when all unread are wakes");
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
