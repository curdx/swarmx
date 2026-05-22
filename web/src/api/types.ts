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
   *  Ignored by per-agent spells. Server mints a fresh dir under
   *  workspaces_root if omitted. */
  workspace_dir?: string;
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
