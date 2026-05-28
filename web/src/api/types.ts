// Mirror of flockmux-protocol's wire types. Hand-maintained for now;
// ts-rs auto-generation lands in M3 once flockmux-protocol grows.

export type ServerControl =
  | {
      type: "hello";
      seq_start: number;
      agent_id: string;
      shim_ready?: boolean;
      shim_exit?: number | null;
    }
  | { type: "shim_ready" }
  | { type: "shim_exit"; code: number }
  | { type: "eof" }
  | { type: "error"; message: string };

export type ClientControl =
  | { type: "resize"; cols: number; rows: number }
  | { type: "ack"; seq: number }
  | { type: "resume"; last_seq: number }
  | { type: "signal"; sig: "SIGINT" | "SIGTERM" | "SIGHUP" }
  | { type: "kill" }
  | { type: "detach" };

export interface CliPluginInfo {
  id: string;
  display_name: string;
  binary: string;
}

export interface SpawnAgentRequest {
  cli: string;
  role?: string;
  workspace?: string;
  /** FK into the workspaces table. Mandatory post-Step-3 of the
   *  workspace-as-first-class rollout — the orphan `+ Claude` button
   *  is routed through CreateWizard when no active workspace exists. */
  workspace_id: string;
}

export interface SpawnAgentResponse {
  agent_id: string;
  cli: string;
  role: string;
  workspace: string;
}

export interface AgentInfo {
  agent_id: string;
  cli: string;
  role: string;
  workspace: string;
  shim_ready: boolean;
  shim_exit: number | null;
  killed_at?: number | null;
  spawned_at?: number | null;
  /** Blackboard keys this agent is subscribed to (M6b wake-coordinator).
   *  Empty for historical / inline-only agents. */
  depends_on?: string[];
  /** Blackboard key this agent will write when its phase completes.
   *  Empty for roles with no handoff_signal (e.g. planner). */
  handoff_signal?: string;
  /** FK into the workspaces table. Null for pre-migration rows or for
   *  legacy spawn paths from before Step 3. The frontend groups the
   *  left nav by this — historical rows fall through to the unnamed
   *  bucket and won't appear as orphan tabs. */
  workspace_id?: string | null;
  /** FK into the spell_runs table. Set for every agent spawned by a
   *  spell launch; null for direct `+ Claude` clicks. Reserved for a
   *  future "group by spell run" toggle. */
  spell_run_id?: string | null;
  /** Derived server-side from spell_runs.caller_agent_id. Non-null only
   *  for sub-agents spawned via MCP `swarm_run_spell` from another agent.
   *  Drives the "雇佣关系" (parent → child) overlay in GraphPanel. */
  parent_agent_id?: string | null;
  /** In-memory pause state. True when the operator has hit "暂停" — the
   *  WakeCoordinator skips auto-wake for this agent until resume. Manual
   *  ⚡ wakes still work. Resets on server restart. */
  paused?: boolean;
}

// ── M3 swarm DTOs ────────────────────────────────────────────────────────

export interface MessageRecord {
  id: number;
  from_agent: string;
  to_agent: string;
  kind: string;
  body: string;
  sent_at: number;
  delivered_at: number | null;
  read_at: number | null;
  in_reply_to: number | null;
}

export interface SendMessageRequest {
  from?: string;
  to: string;
  kind: string;
  body: string;
  in_reply_to?: number;
}

export interface MarkReadRequest {
  to: string;
  ids: number[];
}

export interface MarkReadResponse {
  marked: number[];
  at: number;
}

export interface UnreadCountResponse {
  to: string;
  count: number;
}

export interface BlackboardHistoryEntry {
  id: number;
  agent_id: string | null;
  op: string;
  path: string;
  sha256: string;
  at: number;
  content?: string | null;
}

export interface BlackboardEntry {
  path: string;
  sha256: string;
  at: number;
  op: string;
}

export interface BlackboardSnapshot {
  path: string;
  content: string;
  sha256: string;
  at: number;
}

export interface WriteBlackboardRequest {
  agent_id?: string;
  content: string;
}

// ── M5c spell DTOs ───────────────────────────────────────────────────────

export interface SpellAgentInfo {
  role: string;
  cli: string;
}

export interface SpellInfo {
  name: string;
  description: string;
  agents: SpellAgentInfo[];
}

export interface RunSpellRequest {
  name: string;
  task: string;
  /** Absolute path to a shared workspace directory. Honoured by spells
   *  whose manifest declares `shared_workspace = true` (e.g. the M6a
   *  fullstack-feature spell where FE/BE/Test work in one monorepo).
   *  Ignored by per-agent spells. Server defaults to the resolved
   *  workspace's `cwd` if omitted. */
  workspace_dir?: string;
  /** FK into the workspaces table. The UI's launcher passes this
   *  whenever the user starts a spell from outside an existing agent
   *  context (CreateWizard's init launch, the top-bar SpellsLauncher).
   *  MCP `swarm_run_spell` calls leave it empty and pass `caller_agent_id`
   *  instead — server reverse-resolves both into the same field. */
  workspace_id?: string;
  /** agent_id of the caller. Only set by the MCP tool; UI callers leave
   *  this empty. */
  caller_agent_id?: string;
}

export interface RunSpellAgent {
  role: string;
  cli: string;
  agent_id: string;
}

export interface RunSpellResponse {
  spell: string;
  agents: RunSpellAgent[];
}

// ── workspaces (workspace-as-first-class refactor) ───────────────────────

export interface Workspace {
  id: string;
  /** First 8 chars of `id`. Used as the URL slug `/chat/:slug`. */
  slug: string;
  name: string;
  cwd: string;
  accent?: string | null;
  created_at: number;
  /** Live agents whose `workspace_id` points here. Computed server-side
   *  at list time; not persisted. */
  member_count: number;
}

export interface CreateWorkspaceRequest {
  name: string;
  cwd: string;
  accent?: string;
}

// ── M3 recording DTOs ────────────────────────────────────────────────────

export interface RecordingInfo {
  id: string;
  agent_id: string;
  started_at: number;
  finalized_at: number | null;
  duration_ms: number | null;
  cols: number;
  rows: number;
  last_seq: number | null;
}

// ── /ws/swarm event stream ───────────────────────────────────────────────

export type SwarmAgentState =
  | "spawning"
  | "ready"
  | "thinking"
  | "idle"
  | "exited";

export type SwarmEvent =
  | { type: "agent_state"; agent_id: string; state: SwarmAgentState }
  | {
      type: "message";
      id: number;
      from_agent: string;
      to_agent: string;
      kind: string;
      body: string;
      sent_at: number;
      in_reply_to?: number | null;
    }
  | {
      type: "message_read";
      ids: number[];
      to_agent: string;
      at: number;
    }
  | {
      type: "blackboard_changed";
      id: number;
      agent_id: string | null;
      op: string;
      path: string;
      sha256: string;
      at: number;
    };
