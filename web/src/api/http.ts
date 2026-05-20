import type {
  CliPluginInfo,
  SpawnAgentRequest,
  SpawnAgentResponse,
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

export const api = {
  listPlugins: () => request<CliPluginInfo[]>("GET", "/api/plugins"),
  spawnAgent: (req: SpawnAgentRequest) =>
    request<SpawnAgentResponse>("POST", "/api/agent", req),
  killAgent: (id: string) => request<void>("DELETE", `/api/agent/${id}`),
};
