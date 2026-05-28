//! REST DTOs.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpawnAgentRequest {
    /// Plugin id from `cli-plugins/<id>.toml`, e.g. "claude" or "codex".
    pub cli: String,
    /// Optional role label shown in the UI; defaults to the cli name.
    #[serde(default)]
    pub role: Option<String>,
    /// Optional workspace path. If absent, server allocates a temp dir.
    #[serde(default)]
    pub workspace: Option<String>,
    /// Workspaces table FK. Optional in Step 1 of the rollout for compatibility;
    /// becomes mandatory in Step 3 once the frontend always passes the
    /// active workspace's id and orphan `+ Claude` clicks are routed through
    /// CreateWizard. The server will error if missing post-Step-3.
    #[serde(default)]
    pub workspace_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpawnAgentResponse {
    pub agent_id: String,
    pub cli: String,
    pub role: String,
    pub workspace: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliPluginInfo {
    pub id: String,
    pub display_name: String,
    pub binary: String,
}

/// One entry in `GET /api/agent`. Mirrors `SpawnAgentResponse` plus the
/// fields a reconnecting UI needs to render initial status before its
/// WS Hello frame arrives. `killed_at = Some(_)` means the agent's PTY
/// has been torn down — the entry comes from the SQLite history only.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentInfo {
    pub agent_id: String,
    pub cli: String,
    pub role: String,
    pub workspace: String,
    pub shim_ready: bool,
    pub shim_exit: Option<i32>,
    /// Unix-ms timestamp the agent was killed; `None` while live.
    #[serde(default)]
    pub killed_at: Option<i64>,
    /// Unix-ms timestamp the agent was spawned. Always populated for
    /// SQLite-backed rows; `None` for in-memory-only entries (shouldn't
    /// happen in practice since spawn always writes to SQLite first).
    #[serde(default)]
    pub spawned_at: Option<i64>,
    /// Blackboard keys this agent is waiting on. Populated from the
    /// `wake_subs` runtime map (whatever was registered at spell launch
    /// from the agent's role.depends_on or the spell-level override).
    /// Empty for agents that don't subscribe to anything — including
    /// historical SQLite-only rows where the subscription has been torn
    /// down. Frontend uses this to draw the depends_on DAG.
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// Blackboard key this agent will write when its phase completes —
    /// the inverse of `depends_on`. Read from the agent's role manifest
    /// at list time. Empty if the agent has no `role_ref` (inline-only
    /// agents like critic-loop's writer) or its role has no
    /// `handoff_signal`. Frontend uses this to wire DAG edges from
    /// dependents back to their producers.
    #[serde(default)]
    pub handoff_signal: String,
    /// FK into the workspaces table. `None` for pre-migration rows or for
    /// agents spawned via legacy code paths before Step 3 enforces the
    /// field. Frontend uses this for nav grouping in place of the cwd
    /// (`workspace`) string.
    #[serde(default)]
    pub workspace_id: Option<String>,
    /// FK into the spell_runs table — set only for agents that came out of
    /// a spell launch (the planner/scout pair from `init` and every
    /// downstream spell). UI doesn't render this directly yet; reserved
    /// for a future "group by spell run" toggle.
    #[serde(default)]
    pub spell_run_id: Option<String>,
}

// ── swarm REST DTOs (M3 #18) ─────────────────────────────────────────────

/// `POST /api/message` payload. `from` is optional so a system-emitted
/// notification can omit it; the server fills in a sentinel if it's
/// missing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendMessageRequest {
    #[serde(default)]
    pub from: Option<String>,
    pub to: String,
    pub kind: String,
    pub body: String,
    /// Optional parent message id; threads this message as a reply.
    #[serde(default)]
    pub in_reply_to: Option<i64>,
}

/// Returned by `POST /api/message` so the client knows the persisted row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageRecord {
    pub id: i64,
    pub from_agent: String,
    pub to_agent: String,
    pub kind: String,
    pub body: String,
    pub sent_at: i64,
    pub delivered_at: Option<i64>,
    pub read_at: Option<i64>,
    #[serde(default)]
    pub in_reply_to: Option<i64>,
}

/// `POST /api/message/read` payload. The server enforces `to` matches each
/// id's actual recipient, so callers can't mark someone else's mail read.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarkReadRequest {
    pub to: String,
    pub ids: Vec<i64>,
}

/// `POST /api/message/read` response. `marked` is the subset that this call
/// actually updated (idempotent — repeats return empty).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarkReadResponse {
    pub marked: Vec<i64>,
    pub at: i64,
}

/// `GET /api/message/unread_count?to=<agent>` response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnreadCountResponse {
    pub to: String,
    pub count: i64,
}

/// One row from `GET /api/blackboard-history/*path`. `content` is omitted by
/// default (`?include_content=true` to include) so listing 50 versions of a
/// large file doesn't blow up the JSON payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlackboardHistoryEntry {
    pub id: i64,
    #[serde(default)]
    pub agent_id: Option<String>,
    pub op: String,
    pub path: String,
    pub sha256: String,
    pub at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

/// `PUT /api/blackboard/:path` payload. The path itself rides in the URL;
/// the body carries the new content. `agent_id` lets the caller attribute
/// the write — `None` ⇒ system / external.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WriteBlackboardRequest {
    #[serde(default)]
    pub agent_id: Option<String>,
    pub content: String,
}

/// Returned by `GET /api/blackboard/:path` (snapshot of the latest content).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlackboardSnapshot {
    pub path: String,
    pub content: String,
    pub sha256: String,
    pub at: i64,
}

/// Returned by `GET /api/blackboard` — one entry per known path, latest op.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlackboardEntry {
    pub path: String,
    pub sha256: String,
    pub at: i64,
    pub op: String,
}

// ── recording DTOs (M3 #19) ──────────────────────────────────────────────

/// One entry in `GET /api/recording`. The .cast file content is *not*
/// inlined — clients hit `GET /api/recording/:id` to stream the bytes.
/// `finalized_at = None` means the recording is still live (its agent's
/// PTY hasn't EOFed yet).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordingInfo {
    pub id: String,
    pub agent_id: String,
    pub started_at: i64,
    #[serde(default)]
    pub finalized_at: Option<i64>,
    #[serde(default)]
    pub duration_ms: Option<i64>,
    pub cols: i64,
    pub rows: i64,
    #[serde(default)]
    pub last_seq: Option<i64>,
}

/// Public summary of a loaded spell. Returned by `GET /api/spells` so
/// the UI can populate a launcher dropdown.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpellInfo {
    pub name: String,
    pub description: String,
    /// Roles the spell will spawn, in declaration order. UI shows this so
    /// users know what the spell will fork on their machine before clicking
    /// "run".
    pub agents: Vec<SpellAgentInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpellAgentInfo {
    pub role: String,
    pub cli: String,
}

/// `POST /api/spell/run` request body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunSpellRequest {
    pub name: String,
    /// Free-form task description; substituted into each agent's
    /// `system_prompt` wherever `{task}` appears.
    pub task: String,
    /// Optional workspace directory (absolute path). When the spell
    /// has `shared_workspace = true` (M6a fullstack-feature), every
    /// agent runs with cwd set to this path. When omitted, the server
    /// generates a fresh `<workspaces_root>/spell-<uuid>/` shared dir
    /// so a launch never silently no-ops. Ignored for spells that
    /// don't set `shared_workspace` (each agent gets its own subdir
    /// under workspaces_root as before).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_dir: Option<String>,
    /// Workspaces table FK the spawned agents should be associated with.
    /// When omitted, the server tries to reverse-look-up via
    /// `caller_agent_id`; if neither is available the request is rejected
    /// (post-Step-3). This is what fixes the "orphan workspace tab" bug —
    /// every spell agent inherits the caller's workspace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    /// agent_id of the caller, transparently injected by the
    /// `swarm_run_spell` MCP tool from its `ToolContext`. Server reverse-
    /// resolves it to the caller's `workspace_id`. Direct REST callers
    /// (CreateWizard, top-bar launcher) leave this empty and pass
    /// `workspace_id` instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caller_agent_id: Option<String>,
}

/// `POST /api/spell/run` response. Lists the agents the runner actually
/// spawned (role → agent_id), so the UI can deep-link directly into them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunSpellResponse {
    pub spell: String,
    pub agents: Vec<RunSpellAgent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunSpellAgent {
    pub role: String,
    pub cli: String,
    pub agent_id: String,
}

// ── workspaces (Step 1 of workspace-as-first-class refactor) ─────────────

/// One row from `GET /api/workspaces`. `member_count` is computed at list
/// time (live agents whose `workspace_id` points here). Soft-deleted
/// workspaces aren't included by default.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workspace {
    pub id: String,
    /// First 8 chars of `id`, used as the URL slug for `/chat/:slug`.
    pub slug: String,
    pub name: String,
    pub cwd: String,
    #[serde(default)]
    pub accent: Option<String>,
    pub created_at: i64,
    pub member_count: i64,
}

/// `POST /api/workspaces` payload. CreateWizard sends this before launching
/// the `init` spell so the new workspace is persisted up-front (replaces
/// the old blackboard `workspace.name.<slug>` / `workspace.accent.<slug>`
/// keys).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateWorkspaceRequest {
    pub name: String,
    pub cwd: String,
    #[serde(default)]
    pub accent: Option<String>,
}
