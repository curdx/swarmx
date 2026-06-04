import type {
  AgentInfo,
  BlackboardEntry,
  BlackboardHistoryEntry,
  BlackboardSnapshot,
  BranchInfo,
  CliPluginInfo,
  CreateThreadRequest,
  CreateWorkspaceRequest,
  MarkReadResponse,
  MessageRecord,
  ModelConfig,
  ModelsResponse,
  RecordingInfo,
  RunSpellRequest,
  RunSpellResponse,
  SendMessageRequest,
  SpawnAgentRequest,
  SpawnAgentResponse,
  SpellInfo,
  ThreadInfo,
  UnreadCountResponse,
  Workspace,
  WorkspaceRoot,
  WriteBlackboardRequest,
} from "./types";

import { HTTP_BASE } from "../lib/apiBase";

/** Thrown by `request()` on any non-2xx response. Carries the HTTP `status`
 *  (so callers can special-case 404 → empty-state instead of a red error)
 *  and a `detail` string that is the server's `{ error }` envelope unwrapped
 *  (or the raw body text when not JSON). `.message` keeps the
 *  `METHOD path → status: detail` form for dev-facing logs. */
export class ApiError extends Error {
  status: number;
  detail: string;
  constructor(status: number, detail: string, message: string) {
    super(message);
    this.name = "ApiError";
    this.status = status;
    this.detail = detail;
  }
}

/** Percent-encode each path segment but keep the `/` separators intact —
 *  blackboard keys are slash-delimited (`<wsId>/task.ledger.md`). Using a
 *  bare `encodeURI` left reserved delimiters (`? # &`) unescaped, so a key
 *  containing them got silently truncated into a bogus query/fragment. */
function encodePath(path: string): string {
  return path.split("/").map(encodeURIComponent).join("/");
}

async function request<T>(
  method: string,
  path: string,
  body?: unknown,
  signal?: AbortSignal,
): Promise<T> {
  // `signal` lets a caller abort an in-flight request when its component
  // unmounts (pass an AbortController.signal). Components that don't care just
  // omit it and instead guard their setState with a mounted-ref. Aborting
  // rejects the fetch with an AbortError the caller can ignore.
  const res = await fetch(HTTP_BASE + path, {
    method,
    headers: body ? { "content-type": "application/json" } : undefined,
    body: body ? JSON.stringify(body) : undefined,
    signal,
  });
  if (!res.ok) {
    // Read the body stream EXACTLY ONCE. The old code did `res.json()` then
    // `res.text()` in the catch — but `res.json()` already consumes the
    // stream, so on a non-JSON error body (empty 404, HTML 5xx from a proxy)
    // the `res.text()` retry threw "body stream already read" and the real
    // status code was lost. Read text first, then try to parse.
    const raw = await res.text().catch(() => "");
    let detail = raw;
    try {
      const parsed = JSON.parse(raw);
      if (parsed && typeof parsed.error === "string") detail = parsed.error;
    } catch {
      /* not JSON — keep raw text */
    }
    throw new ApiError(
      res.status,
      detail,
      `${method} ${path} → ${res.status}: ${detail || res.statusText}`,
    );
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

/** One detected runtime (node / npm / uv) for the MCP env probe. */
export interface RuntimeInfo {
  present: boolean;
  version: string | null;
}
export interface McpEnv {
  node: RuntimeInfo;
  npm: RuntimeInfo;
  uv: RuntimeInfo;
}
/** Per-server API-key state: set?, masked preview, and whether claude/codex agree. */
export interface McpKeyState {
  present: boolean;
  masked: string | null;
  consistent: boolean;
}
/** Names of MCP servers already configured per CLI (user scope) + key states. */
export interface McpStatus {
  claude: string[];
  codex: string[];
  keys?: Record<string, McpKeyState>;
}

export const api = {
  listPlugins: () => request<CliPluginInfo[]>("GET", "/api/plugins"),
  // MCP admin (「快捷装 MCP」页面)
  mcpEnv: () => request<McpEnv>("GET", "/api/mcp/env"),
  mcpStatus: () => request<McpStatus>("GET", "/api/mcp/status"),
  mcpInstall: (name: string, cli: "claude" | "codex", apiKey?: string) =>
    request<{ ok: boolean; output: string }>("POST", "/api/mcp/install", {
      name,
      cli,
      ...(apiKey ? { api_key: apiKey } : {}),
    }),
  mcpUninstall: (name: string, cli: "claude" | "codex") =>
    request<{ ok: boolean; output: string }>("POST", "/api/mcp/uninstall", { name, cli }),
  listAgents: () => request<AgentInfo[]>("GET", "/api/agent"),
  spawnAgent: (req: SpawnAgentRequest) =>
    request<SpawnAgentResponse>("POST", "/api/agent", req),
  killAgent: (id: string) => request<void>("DELETE", `/api/agent/${id}`),
  // M6e: 操作者手动 ⚡ 唤醒。后端走 mailbox + PTY-kick（同主路径），
  // body 写"manual wake from operator"。挡在 M6d-6 quiet gate 后面，
  // agent 正在 stream 时只投 mailbox 不戳 PTY。
  wakeAgent: (id: string) => request<void>("POST", `/api/agent/${id}/wake`),
  // 不杀 PTY 的暂停:发 Ctrl-C 取消当前 turn + 设 paused flag。WakeCoordinator
  // 跳过这个 agent 的自动唤醒,manual ⚡ 仍然能用。
  interruptAgent: (id: string) =>
    request<{ ok: boolean; agent_id: string; paused: boolean }>(
      "POST",
      `/api/agent/${id}/interrupt`,
    ),
  resumeAgent: (id: string) =>
    request<{ ok: boolean; agent_id: string; paused: boolean }>(
      "POST",
      `/api/agent/${id}/resume`,
    ),
  // workspace 级"全停"。后端遍历 registry 找匹配 workspace_id 的 live agent,
  // 逐个 interrupt。返回 { interrupted, agent_ids, failed }。
  interruptAllInWorkspace: (workspaceId: string) =>
    request<{
      ok: boolean;
      interrupted: number;
      agent_ids: string[];
      failed: Array<{ agent_id: string; error: string }>;
    }>("POST", `/api/agent/interrupt-all${qs({ workspace_id: workspaceId })}`),

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
    request<BlackboardSnapshot>("GET", `/api/blackboard/${encodePath(path)}`),
  writeBlackboard: (path: string, req: WriteBlackboardRequest) =>
    request<{ id: number; path: string; sha256: string; at: number }>(
      "PUT",
      `/api/blackboard/${encodePath(path)}`,
      req,
    ),
  listBlackboardHistory: (path: string, limit = 50, includeContent = false) =>
    request<BlackboardHistoryEntry[]>(
      "GET",
      `/api/blackboard-history/${encodePath(path)}${qs({ limit, include_content: includeContent })}`,
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

  // workspaces (workspace-as-first-class refactor)
  listWorkspaces: () => request<Workspace[]>("GET", "/api/workspaces"),
  createWorkspace: (req: CreateWorkspaceRequest) =>
    request<Workspace>("POST", "/api/workspaces", req),
  deleteWorkspace: (id: string) =>
    request<void>("DELETE", `/api/workspaces/${id}`),
  // threads (directions within a workspace). `id` here is the workspace UUID
  // (workspaceId), matching the server's path param.
  listThreads: (id: string) =>
    request<ThreadInfo[]>("GET", `/api/workspaces/${id}/threads`),
  // Local branches of a workspace's repo, for the "open existing branch as a
  // direction" picker.
  listBranches: (id: string) =>
    request<BranchInfo[]>("GET", `/api/workspaces/${id}/branches`),
  createThread: (id: string, req: CreateThreadRequest) =>
    request<ThreadInfo>("POST", `/api/workspaces/${id}/threads`, req),
  updateThread: (id: string, threadId: string, req: { name: string }) =>
    request<ThreadInfo>(
      "PATCH",
      `/api/workspaces/${id}/threads/${threadId}`,
      req,
    ),
  deleteThread: (id: string, threadId: string) =>
    request<void>("DELETE", `/api/workspaces/${id}/threads/${threadId}`),
  // attached dependency-source roots (post-create management)
  addWorkspaceRoot: (id: string, root: WorkspaceRoot) =>
    request<WorkspaceRoot>("POST", `/api/workspaces/${id}/roots`, root),
  /** Remove a root node (by its server id); the backend cascade-deletes any
   *  children mounted under it. */
  deleteWorkspaceRoot: (id: string, rootId: string) =>
    request<{ deleted: number }>(
      "DELETE",
      `/api/workspaces/${id}/roots?id=${encodeURIComponent(rootId)}`,
    ),
  /** Candidate local source deps parsed from a project's manifests
   *  (package.json file:/link:, Cargo path, go.mod replace, uv sources).
   *  `projectPath` scopes the scan to a specific project dir; defaults to the
   *  primary cwd when omitted. */
  rootSuggestions: (id: string, projectPath?: string) =>
    request<WorkspaceRoot[]>(
      "GET",
      `/api/workspaces/${id}/root-suggestions${projectPath ? `?path=${encodeURIComponent(projectPath)}` : ""}`,
    ),

  // F1 model settings: per-CLI tier→concrete-model mapping.
  getModels: () => request<ModelsResponse>("GET", "/api/models"),
  putModels: (config: ModelConfig) =>
    request<{ config: ModelConfig }>("PUT", "/api/models", config),
};
