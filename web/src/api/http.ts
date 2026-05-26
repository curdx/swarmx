import type {
  AgentInfo,
  BlackboardEntry,
  BlackboardHistoryEntry,
  BlackboardSnapshot,
  CliPluginInfo,
  MarkReadResponse,
  MessageRecord,
  RecordingInfo,
  RunSpellRequest,
  RunSpellResponse,
  SendMessageRequest,
  SpawnAgentRequest,
  SpawnAgentResponse,
  SpellInfo,
  UnreadCountResponse,
  WriteBlackboardRequest,
} from "./types";

import { HTTP_BASE } from "../lib/apiBase";

async function request<T>(
  method: string,
  path: string,
  body?: unknown,
): Promise<T> {
  const res = await fetch(HTTP_BASE + path, {
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
  // M6e: 操作者手动 ⚡ 唤醒。后端走 mailbox + PTY-kick（同主路径），
  // body 写"manual wake from operator"。挡在 M6d-6 quiet gate 后面，
  // agent 正在 stream 时只投 mailbox 不戳 PTY。
  wakeAgent: (id: string) => request<void>("POST", `/api/agent/${id}/wake`),

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
  // Prefix HTTP_BASE so direct <a href> / fetch() outside the api.* layer
  // (asciinema-player, castPreview, download link) still resolves in Tauri prod.
  recordingCastUrl: (id: string) =>
    `${HTTP_BASE}/api/recording/${encodeURIComponent(id)}`,

  // M5c spells
  listSpells: () => request<SpellInfo[]>("GET", "/api/spells"),
  runSpell: (req: RunSpellRequest) =>
    request<RunSpellResponse>("POST", "/api/spell/run", req),
};
