import type {
  AgentInfo,
  BlackboardEntry,
  BlackboardHistoryEntry,
  BlackboardSnapshot,
  CliPluginInfo,
  MarkReadResponse,
  MessageRecord,
  RecordingInfo,
  SendMessageRequest,
  SpawnAgentRequest,
  SpawnAgentResponse,
  UnreadCountResponse,
  WriteBlackboardRequest,
} from "./types";

async function request<T>(
  method: string,
  path: string,
  body?: unknown,
): Promise<T> {
  const res = await fetch(path, {
    method,
    headers: body ? { "content-type": "application/json" } : undefined,
    body: body ? JSON.stringify(body) : undefined,
  });
  if (!res.ok) {
    let detail = "";
    try {
      detail = JSON.stringify(await res.json());
    } catch {
      detail = await res.text();
    }
    throw new Error(`${method} ${path} → ${res.status}: ${detail}`);
  }
  // 204 No Content
  if (res.status === 204) return undefined as T;
  return res.json() as Promise<T>;
}

export interface ListMessagesQuery {
  to?: string;
  from?: string;
  q?: string;
  limit?: number;
  only_undelivered?: boolean;
}

function qs(params: Record<string, string | number | boolean | undefined>): string {
  const parts: string[] = [];
  for (const [k, v] of Object.entries(params)) {
    if (v === undefined || v === "" || v === false) continue;
    parts.push(`${encodeURIComponent(k)}=${encodeURIComponent(String(v))}`);
  }
  return parts.length ? `?${parts.join("&")}` : "";
}

export const api = {
  listPlugins: () => request<CliPluginInfo[]>("GET", "/api/plugins"),
  listAgents: () => request<AgentInfo[]>("GET", "/api/agent"),
  spawnAgent: (req: SpawnAgentRequest) =>
    request<SpawnAgentResponse>("POST", "/api/agent", req),
  killAgent: (id: string) => request<void>("DELETE", `/api/agent/${id}`),

  // M3 swarm
  listMessages: (q: ListMessagesQuery = {}) =>
    request<MessageRecord[]>("GET", `/api/message${qs(q as Record<string, string | number | boolean | undefined>)}`),
  sendMessage: (req: SendMessageRequest) =>
    request<MessageRecord>("POST", "/api/message", req),
  markMessagesRead: (to: string, ids: number[]) =>
    request<MarkReadResponse>("POST", "/api/message/read", { to, ids }),
  unreadCount: (to: string) =>
    request<UnreadCountResponse>("GET", `/api/message/unread_count${qs({ to })}`),
  listBlackboard: () =>
    request<BlackboardEntry[]>("GET", "/api/blackboard"),
  readBlackboard: (path: string) =>
    request<BlackboardSnapshot>("GET", `/api/blackboard/${encodeURI(path)}`),
  writeBlackboard: (path: string, req: WriteBlackboardRequest) =>
    request<{ id: number; path: string; sha256: string; at: number }>(
      "PUT",
      `/api/blackboard/${encodeURI(path)}`,
      req,
    ),
  listBlackboardHistory: (path: string, limit = 50, includeContent = false) =>
    request<BlackboardHistoryEntry[]>(
      "GET",
      `/api/blackboard-history/${encodeURI(path)}${qs({ limit, include_content: includeContent })}`,
    ),

  // M3 recordings
  listRecordings: (agentId?: string) =>
    request<RecordingInfo[]>(
      "GET",
      `/api/recording${qs({ agent_id: agentId })}`,
    ),
  recordingCastUrl: (id: string) => `/api/recording/${encodeURIComponent(id)}`,
};
