/** Shared helpers for workspace "directions" (threads). Kept in one module so
 *  the shell data hook, the views, and every navigation site agree on the
 *  main-direction fallback, the agent→direction membership rule, and the
 *  direction URL shape. */

import type { AgentInfo, ThreadInfo } from "../api/types";

/** The workspace's main direction: the explicit `main` slug, else the oldest
 *  row (list order is oldest-first). `null` for a legacy/empty workspace with
 *  no thread rows. */
export function mainThreadOf(threads: ThreadInfo[]): ThreadInfo | null {
  if (threads.length === 0) return null;
  return threads.find((t) => t.slug === "main") ?? threads[0];
}

/** Does agent `a` belong to the active direction? For the MAIN direction,
 *  agents with `thread_id == null` (legacy + pre-thread spawns) fold in. With
 *  no thread rows (`activeThread` null) every workspace agent counts — the
 *  workspace is then one implicit direction. */
export function agentInThread(
  a: AgentInfo,
  workspaceId: string,
  activeThread: ThreadInfo | null,
  mainThread: ThreadInfo | null,
): boolean {
  if (a.workspace_id !== workspaceId) return false;
  if (!activeThread) return true;
  if (mainThread && activeThread.id === mainThread.id) {
    return a.thread_id == null || a.thread_id === activeThread.id;
  }
  return a.thread_id === activeThread.id;
}

/** Route base for a direction: bare `/chat/:wsId` for the main direction (slug
 *  `main` or absent), else `/chat/:wsId/t/:threadSlug`. Append a view suffix
 *  (`/dag`, `/ledger`, …) as needed so in-app navigation stays in-direction. */
export function directionBase(wsId: string, threadSlug?: string | null): string {
  return threadSlug && threadSlug !== "main"
    ? `/chat/${wsId}/t/${threadSlug}`
    : `/chat/${wsId}`;
}

/** Extract the direction slug from a blackboard key shaped
 *  `{workspace_id}/{thread_slug}/…`. Returns `null` for a legacy/short key
 *  (< 3 segments) so the caller defaults to the main direction. */
export function directionSlugFromKey(path: string): string | null {
  const parts = path.split("/");
  return parts.length >= 3 ? parts[1] : null;
}
