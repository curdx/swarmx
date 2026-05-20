// Mirror of flockmux-protocol's wire types. Hand-maintained for now;
// ts-rs auto-generation lands in M3 once flockmux-protocol grows.

export type ServerControl =
  | { type: "hello"; seq_start: number; agent_id: string }
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
