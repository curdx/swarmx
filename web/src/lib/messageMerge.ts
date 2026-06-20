import type { MessageRecord } from "../api/types";

export interface MergeResult {
  /** The merged list, id-ascending. */
  next: MessageRecord[];
  /** Local rows rescued from the race window (diagnostics). */
  preservedIds: number[];
  /** Local rows not in the result (diagnostics; most are expected). */
  droppedIds: number[];
  /** Same-thread drops — the real "回复消失" regression signal (want []). */
  lostSameThread: number[];
  /** Rows in `next` that weren't in `prev` (diagnostics). */
  addedCount: number;
}

/**
 * Merge a fresh server snapshot of ONE thread's messages with the locally-held
 * `prev` list. Server rows are the source of truth; we additionally PRESERVE
 * local rows the snapshot couldn't have seen yet, so a wholesale replace never
 * clobbers an in-flight message (the historical "回复消失" bug — the list
 * refreshes on mount, direction switch, reconnect, and the manual button, so the
 * overlap window recurs constantly in real use).
 *
 * Which local rows survive: same-thread, not already in the server result, and
 * NOT OLDER than the server window. The server returns the most-recent N rows
 * for the thread with NO id gaps inside [minServerId, maxServerId], so a missing
 * same-thread id INSIDE that range can only be a read/write-race victim — it
 * committed after this fetch's read snapshot, but its real id already landed
 * locally (an optimistic-send echo, or a concurrent reply). Rows BELOW the
 * window (id < minServerId) are history that scrolled out of the N-cap and are
 * correctly dropped; the thread gate makes a real direction switch read as a
 * clean replace regardless.
 *
 * The bound used to be `id > maxServerId`, which only rescued rows NEWER than
 * everything the server returned and so sacrificed in-window race victims → a
 * sub-second "message flickers then disappears". `id >= minServerId` widens the
 * rescue to the whole window without ever resurrecting a cross-thread or
 * genuinely-older row. An EMPTY snapshot (brand-new thread) keeps same-thread
 * local rows so an optimistic send to a fresh thread isn't dropped before its
 * row is readable.
 */
export function mergeServerSnapshot(
  prev: MessageRecord[],
  serverRowsAsc: MessageRecord[],
  activeThreadId: string | null,
): MergeResult {
  const prevIds = new Set(prev.map((m) => m.id));
  const serverIds = new Set(serverRowsAsc.map((m) => m.id));
  const minServerId = serverRowsAsc.reduce(
    (mn, m) => Math.min(mn, m.id),
    Infinity,
  );
  const sameThread = (m: MessageRecord) =>
    (m.thread_id ?? null) === (activeThreadId ?? null);

  const preserved = prev.filter(
    (m) =>
      !serverIds.has(m.id) &&
      sameThread(m) &&
      (serverRowsAsc.length === 0 || m.id >= minServerId),
  );
  const next = preserved.length
    ? [...serverRowsAsc, ...preserved].sort((a, b) => a.id - b.id)
    : serverRowsAsc;

  const dropped = prev.filter((m) => !next.some((n) => n.id === m.id));
  return {
    next,
    preservedIds: preserved.map((m) => m.id),
    droppedIds: dropped.map((m) => m.id),
    lostSameThread: dropped.filter(sameThread).map((m) => m.id),
    addedCount: next.filter((m) => !prevIds.has(m.id)).length,
  };
}
