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
  /** Keystroke-settle delay (ms) the terminal applies after the CLI is ready
   *  (codex ~300, claude 0). Mirrors the backend manifest; drives the input
   *  policy so adding a CLI needs no frontend edit. Optional for back-compat
   *  with an older server that doesn't send it. */
  input_settle_ms?: number;
  /** Default model this CLI runs when a spawn doesn't override it (L5c).
   *  Undefined ⇒ the CLI picks its own default. A spawn UI can use this to
   *  pre-fill / offer a model picker without hardcoding per-CLI names. */
  default_model?: string;
}

export interface SpawnAgentRequest {
  cli: string;
  role?: string;
  workspace?: string;
  /** FK into the workspaces table. Mandatory post-Step-3 of the
   *  workspace-as-first-class rollout — the orphan `+ Claude` button
   *  is routed through CreateWizard when no active workspace exists. */
  workspace_id: string;
  /** Optional model overlay (L5c). Passed to the CLI via its manifest
   *  `model_args` template. Omit to use the plugin's default_model / the
   *  CLI's own default. Decouples model from CLI id. */
  model?: string;
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
   *  Drives the parent → child spawn edges in the DAG view (see
   *  lib/dagEdgeDerivation deriveSpawnEdges). */
  parent_agent_id?: string | null;
  /** FK into the threads table (a workspace's "direction"). Null = the
   *  workspace's main direction (legacy rows + pre-thread spawns). The UI
   *  groups chat/members/ledger by this; null is treated as the main thread. */
  thread_id?: string | null;
  /** In-memory pause state. True when the operator has hit "暂停" — the
   *  WakeCoordinator skips auto-wake for this agent until resume. Manual
   *  ⚡ wakes still work. Resets on server restart. */
  paused?: boolean;
  /** Unix-ms of the agent's most recent tool-level activity, persisted by the
   *  transcript tailer (server migration 0013). Null for agents that never
   *  emitted a tool event (e.g. the orchestrator, which isn't tailed). Used to
   *  tell "wedged" from "idle" even on a cold load, before the live
   *  AgentActivity WS stream has caught up. */
  last_activity_at?: number | null;
}

// ── M3 swarm DTOs ────────────────────────────────────────────────────────

/** Structured metadata the server stamps on system-generated messages
 *  (typed-payload pattern; see flockmux-storage migration 0012). The UI
 *  renders / filters from `subtype` instead of regex-parsing the prose body.
 *  Absent (undefined/null) for agent free-text messages. */
export interface MessageMeta {
  /** "wake" (coordination ping) | "completion" (worker delivered + disbanded). */
  subtype?: string;
  /** For wakes: "blackboard" (auto, redundant → UI filters) | "manual" (operator). */
  reason?: string;
  /** For blackboard wakes: the raw blackboard key the agent must check. */
  key?: string;
  /** For completions: the handoff signal that was delivered. */
  signal?: string;
}

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
  /** Direction (thread) this message belongs to; null = main / untagged. */
  thread_id?: string | null;
  meta?: MessageMeta | null;
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
  /** The direction (thread) the spell should run in. UI launchers set this to
   *  target a specific direction; omitted resolves to the main direction. */
  thread_id?: string;
}

/** `POST /api/workspaces/:id/threads` — open a new direction. `name` optional
 *  (zero-friction: the orchestrator names it from the first message). `branch`
 *  opens an EXISTING branch as the direction (attach a worktree to it) instead
 *  of creating a fresh one. */
export interface CreateThreadRequest {
  name?: string | null;
  branch?: string | null;
}

/** One local branch of a workspace's repo (`GET /api/workspaces/:id/branches`),
 *  for the "open existing branch as a direction" picker. */
export interface BranchInfo {
  name: string;
  /** Already checked out in some worktree → can't be attached again. */
  checked_out: boolean;
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

/** An attached dependency-source root folder. The workspace's primary
 *  project dir is `Workspace.cwd`; these are extra source roots agents may
 *  read directly (so they don't have to decompile/guess a dependency).
 *  `role` is "dependency" | "tool" today (kept open as string). */
export interface WorkspaceRoot {
  /** Server-assigned id. Omitted on create/add payloads (server fills it). */
  id?: string;
  path: string;
  /** "project" (a top-level peer project) | "dependency" | "tool". */
  role: string;
  label?: string | null;
  /** Logical-tree parent: another root's id, or null/undefined for a
   *  top-level node. Decoupled from physical path — a node's `path` can live
   *  anywhere; parent_id only expresses the "depends on / belongs under"
   *  relationship the user chose. */
  parent_id?: string | null;
  /** Branch currently checked out at `path`, filled by the workspaces list for
   *  the sidebar's live branch chip. null for a non-git dir / detached HEAD. */
  branch?: string | null;
}

/** One "direction" inside a workspace: its own orchestrator + worker subtree +
 *  dual ledger + (optionally) an isolated git worktree. Mirrors the server
 *  `ThreadInfo`. `slug` is the per-direction blackboard / URL segment; the main
 *  direction's slug is always `main`. */
export interface ThreadInfo {
  id: string;
  workspace_id: string;
  slug: string;
  name?: string | null;
  /** "shared" | "worktree" */
  isolation: string;
  branch?: string | null;
  /** Working dir agents in this direction run in (= workspace cwd for a shared
   *  thread; the worktree path once isolated). */
  cwd: string;
  /** "ready" | "preparing" | "failed" */
  state: string;
  /** Per-direction model override (abstract tier opus|sonnet|haiku or a concrete
   *  model id). Null = use the global default. Set via the chat model picker. */
  model_tier?: string | null;
  /** Live (computed server-side at list time): does `cwd` have uncommitted
   *  changes? Drives the sidebar's dirty dot. */
  dirty?: boolean;
  /** Live (computed server-side, purely local — no fetch): commits this
   *  direction's branch is ahead of / behind the workspace's base branch.
   *  Null for the main/shared direction or a non-git tree. */
  ahead?: number | null;
  behind?: number | null;
  created_at: number;
}

/** Preview of what a direction changed, before merging it back to the main line
 *  (`GET /api/workspaces/:id/threads/:tid/diff`). */
export interface ThreadDiff {
  /** Base branch (the workspace cwd's current branch). */
  base?: string | null;
  /** The direction's own branch. */
  branch?: string | null;
  /** Repo-relative files this direction changed vs the merge-base. */
  files: string[];
  /** Base work tree has uncommitted changes → a merge would be refused. */
  base_dirty: boolean;
}

/** Result of `POST /api/workspaces/:id/threads/:tid/merge`. `status` discriminates:
 *  - "merged": clean merge into `base` (`files` = changed-file count).
 *  - "resolving": conflicts; an AI agent (`agent_id`) was spawned to resolve. */
export type MergeResult =
  | { status: "merged"; base: string; files: number }
  | { status: "resolving"; agent_id: string; files: string[] };

export interface Workspace {
  id: string;
  /** First 8 chars of `id`. Used as the URL slug `/chat/:slug`. */
  slug: string;
  name: string;
  cwd: string;
  /** Branch currently checked out at `cwd` (the agent's terminal home), filled
   *  by the workspaces list for the sidebar's branch chip. null for a non-git
   *  cwd / detached HEAD. */
  cwd_branch?: string | null;
  accent?: string | null;
  created_at: number;
  /** Live agents whose `workspace_id` points here. Computed server-side
   *  at list time; not persisted. */
  member_count: number;
  /** Attached dependency-source roots (excludes the primary `cwd`). */
  roots?: WorkspaceRoot[];
  /** The workspace's directions (always ≥1: an auto-created `main`).
   *  Oldest-first; the first entry is the main thread. */
  threads?: ThreadInfo[];
}

export interface CreateWorkspaceRequest {
  name: string;
  cwd: string;
  accent?: string;
  /** Attached dependency-source roots to persist alongside the workspace. */
  roots?: WorkspaceRoot[];
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
  | "exited"
  /** Blocked on an upstream handoff (a `consumes` dependency hasn't been
   *  produced yet). The WakeCoordinator will re-wake it once the blackboard
   *  key it depends on is written. */
  | "waiting_dep"
  /** Exited abnormally (non-zero shim_exit / crashed mid-turn). Distinct from
   *  a clean `exited` so the UI can surface it red + float it to the top. */
  | "error";

/** A step-level activity event the server derives by tailing the CLI's
 *  session JSONL: one per tool call (Edit/Bash/Read…) or system step. The
 *  member list shows the latest one as "what this worker is doing right now";
 *  the AgentDrawer activity tab streams the whole round.
 *
 *  - `kind`     — "tool" (Edit/Bash/…) or "system" (a non-tool step).
 *  - `label`    — human-facing line, e.g. "Edit src/foo.rs".
 *  - `phase`    — "running" (in flight, no duration yet) → "ok" / "error".
 *  - `seq`      — monotonic per-agent activity index.
 *  - `duration_ms` — wall time once the step settles; absent while running.
 *  - `at`       — unix-ms the event was emitted. */
export interface AgentActivity {
  agent_id: string;
  kind: "tool" | "system";
  label: string;
  phase: "running" | "ok" | "error";
  seq: number;
  duration_ms?: number;
  at: number;
}

/** Per-agent live slice the swarm WS accumulates client-side (state + latest
 *  activity), keyed by agent_id. The REST `AgentInfo` row carries no live
 *  state/activity — those only arrive over `/ws/swarm` — so this is the only
 *  source of truth for them, with `inferAgentStatus` as the back-compat
 *  fallback when an event hasn't been seen yet. */
export interface AgentLiveState {
  state?: SwarmAgentState;
  activity?: AgentActivity;
}

export type SwarmEvent =
  | { type: "agent_state"; agent_id: string; state: SwarmAgentState }
  | ({ type: "agent_activity" } & AgentActivity)
  | {
      type: "message";
      id: number;
      from_agent: string;
      to_agent: string;
      kind: string;
      body: string;
      sent_at: number;
      in_reply_to?: number | null;
      thread_id?: string | null;
      meta?: MessageMeta | null;
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
    }
  | {
      type: "thread_changed";
      workspace_id: string;
      thread_id: string;
      op: string;
    };

// ── F1 model settings: per-CLI tier→concrete-model mapping ──────────────────
export interface CliModels {
  /** Concrete model id when a spawn resolves to NO tier. Empty = CLI default. */
  default: string;
  /** tier (opus/sonnet/haiku) → concrete model id. Empty value = CLI default
   *  for that tier; absent key = fall back to `default`. */
  tiers: Record<string, string>;
}
export interface ModelConfig {
  version: number;
  clis: Record<string, CliModels>;
}
export interface ModelCliInfo {
  id: string;
  display_name: string;
  /** false ⇒ the CLI declares no model_args; the UI greys it out. */
  supports_model: boolean;
}
export interface ModelsResponse {
  config: ModelConfig;
  clis: ModelCliInfo[];
}
