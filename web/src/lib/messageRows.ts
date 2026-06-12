/**
 * Pure message-row helpers extracted from MessagesPanel — grouping + timestamp
 * formatting + role resolution. Dependency-free and side-effect-free so they're
 * trivially unit-testable (see messageRows.test.ts) and reusable. Keeping the
 * grouping heuristic here as the single source of truth guards against it
 * silently drifting if the panel is ever rewritten.
 */
import type { MessageRecord } from "../api/types";

/** Gap between adjacent messages beyond which a time-divider is inserted and
 *  the sender header is re-shown (same 5-min heuristic as Telegram). */
export const GROUP_GAP_MS = 5 * 60_000;

export interface Row {
  msg: MessageRecord;
  /** Render the avatar + name header row? (false = collapsed into the run). */
  showHeader: boolean;
  showDividerBefore: boolean;
}

/**
 * Resolve a role label for a `from_agent` id.
 *
 * Looks up the lookup map first (populated from /api/agent — covers both active
 * and exited agents). Falls back to a string heuristic so the very first paint,
 * before listAgents() resolves, still shows *something*.
 */
export function resolveRole(fromAgent: string, lookup: Map<string, string>): string {
  const hit = lookup.get(fromAgent);
  if (hit) return hit;
  // agent_ids historically follow either `<cli>-<hash>` or `_<role>_<hash>`.
  // Neither prefix is the role we want, but it's better than the full id.
  const seg = fromAgent.replace(/^_+/, "").split(/[-_]/)[0];
  return seg || "agent";
}

export function formatClock(ms: number): string {
  return new Date(ms).toLocaleTimeString([], {
    hour: "2-digit",
    minute: "2-digit",
  });
}

export function formatFullStamp(ms: number): string {
  return new Date(ms).toLocaleString();
}

export function formatDivider(ms: number): string {
  const now = new Date();
  const d = new Date(ms);
  const sameDay =
    d.getFullYear() === now.getFullYear() &&
    d.getMonth() === now.getMonth() &&
    d.getDate() === now.getDate();
  return sameDay
    ? d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })
    : d.toLocaleString([], {
        month: "short",
        day: "numeric",
        hour: "2-digit",
        minute: "2-digit",
      });
}

export function formatElapsed(ms: number): string {
  const sec = Math.max(0, Math.floor(ms / 1000));
  if (sec < 60) return `${sec}s`;
  const min = Math.floor(sec / 60);
  const s = sec % 60;
  if (min < 60) return `${min}m ${String(s).padStart(2, "0")}s`;
  const h = Math.floor(min / 60);
  return `${h}h ${String(min % 60).padStart(2, "0")}m`;
}

/**
 * Group messages into rows: collapse the avatar/name header for consecutive
 * same-sender messages, and insert a time-divider when the gap between adjacent
 * messages exceeds {@link GROUP_GAP_MS}.
 */
export function buildRows(items: MessageRecord[]): Row[] {
  const rows: Row[] = [];
  let prev: MessageRecord | null = null;
  for (const msg of items) {
    const gap = prev ? msg.sent_at - prev.sent_at : Infinity;
    const sameSender = prev?.from_agent === msg.from_agent;
    const showDividerBefore = prev !== null && gap > GROUP_GAP_MS;
    const showHeader = !sameSender || showDividerBefore;
    rows.push({ msg, showHeader, showDividerBefore });
    prev = msg;
  }
  return rows;
}
