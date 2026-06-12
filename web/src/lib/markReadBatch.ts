import type { MessageRecord } from "../api/types";

/**
 * The two DOM-free rules behind F5 auto-mark-read, pulled out of MessagesPanel
 * so they're unit-testable: which bubbles the IntersectionObserver should watch,
 * and how a server-confirmed read batch folds back into the local list. The
 * hook (useScrollMarkRead) owns the observer + debounce timer; these own the
 * data.
 */

const USER_SENDER = "user";

/**
 * Unread, user-bound message ids the observer should watch: messages addressed
 * TO the user that haven't been read yet. Mirrors the observer's filter
 * (`to_agent === "user" && read_at === null`).
 */
export function collectObservableIds(items: MessageRecord[]): number[] {
  return items
    .filter((m) => m.to_agent === USER_SENDER && m.read_at === null)
    .map((m) => m.id);
}

/**
 * Apply a server-confirmed batch of read ids to the local message list: stamp
 * read_at on the marked ids, leave the rest untouched. Returns the SAME array
 * reference when nothing was marked, so React skips a needless re-render.
 */
export function applyMarkedRead(
  items: MessageRecord[],
  markedIds: number[],
  at: number,
): MessageRecord[] {
  if (markedIds.length === 0) return items;
  const marked = new Set(markedIds);
  return items.map((m) => (marked.has(m.id) ? { ...m, read_at: at } : m));
}
