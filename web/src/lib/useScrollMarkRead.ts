import {
  useCallback,
  useEffect,
  useRef,
  type Dispatch,
  type RefObject,
  type SetStateAction,
} from "react";
import { api } from "../api/http";
import type { MessageRecord } from "../api/types";
import { applyMarkedRead, collectObservableIds } from "./markReadBatch";

const USER_SENDER = "user";

/**
 * F5 auto-mark-read: as agent→user bubbles scroll into the list viewport — and
 * only while the tab is foregrounded — batch a debounced POST /api/message/read
 * and stamp read_at locally. Side-effecting; returns nothing.
 *
 * Extracted verbatim from MessagesPanel: same IntersectionObserver, same 400ms
 * debounce-batch, same foreground gate (a backgrounded tab scrolling via an
 * anchor isn't a human reading), same two cleanups (observer disconnect + the
 * pending-flush clearTimeout on unmount). `listRef`/`rowRefs` stay owned by the
 * component (shared with scroll-to-parent, auto-scroll, JSX ref-callbacks) and
 * are passed in. The read/write data rules live in markReadBatch (unit-tested).
 *
 * `revision` is an opaque token that changes whenever the set of currently
 * mounted rows changes (e.g. the virtualizer's visible range). When the list is
 * virtualized, off-screen rows aren't in the DOM, so a row scrolling INTO view
 * is a fresh mount the existing observer never saw — bumping revision
 * re-subscribes the observer over the now-current rowRefs. Omit it for a
 * fully-rendered (non-virtualized) list.
 */
export function useScrollMarkRead(opts: {
  listRef: RefObject<HTMLDivElement | null>;
  rowRefs: RefObject<Map<number, HTMLDivElement | null>>;
  items: MessageRecord[];
  setItems: Dispatch<SetStateAction<MessageRecord[]>>;
  revision?: string;
}): void {
  const { listRef, rowRefs, items, setItems, revision } = opts;
  const pendingReadRef = useRef<Set<number>>(new Set());
  // Ids currently on screen. P1-06: a fast scroll-PAST (intersected then left
  // before the debounce fires) must NOT count as "read" — only messages still
  // visible at flush time are. This keeps "opened ≠ a human actually read it".
  const visibleRef = useRef<Set<number>>(new Set());
  const flushTimerRef = useRef<number | null>(null);

  const flushAutoRead = useCallback(() => {
    flushTimerRef.current = null;
    const ids = [...pendingReadRef.current].filter((id) => visibleRef.current.has(id));
    pendingReadRef.current.clear();
    if (ids.length === 0) return;
    // All collected ids are to_agent === "user" (see the observer filter).
    api
      .markMessagesRead(USER_SENDER, ids)
      .then((res) => {
        if (res.marked.length === 0) return;
        setItems((prev) => applyMarkedRead(prev, res.marked, res.at));
      })
      .catch(() => {
        /* best-effort — the bubble stays observed and retries next intersect */
      });
  }, [setItems]);

  useEffect(() => {
    const root = listRef.current;
    if (!root || typeof IntersectionObserver === "undefined") return;
    const elToId = new Map<Element, number>();
    const io = new IntersectionObserver(
      (entries) => {
        // Foreground-only: a backgrounded tab scrolling (e.g. via anchor)
        // isn't a human reading. Honors the original "opened ≠ read" caveat.
        if (document.visibilityState !== "visible") return;
        let added = false;
        for (const e of entries) {
          const id = elToId.get(e.target);
          if (id == null) continue;
          if (e.isIntersecting) {
            visibleRef.current.add(id);
            pendingReadRef.current.add(id);
            added = true;
          } else {
            // Left the viewport before the debounce — drop it from the
            // "still visible" set so flush won't mark a scrolled-past bubble.
            visibleRef.current.delete(id);
          }
        }
        if (added && flushTimerRef.current == null) {
          flushTimerRef.current = window.setTimeout(flushAutoRead, 400);
        }
      },
      { root, threshold: 0 },
    );
    for (const id of collectObservableIds(items)) {
      const el = rowRefs.current?.get(id);
      if (el) {
        elToId.set(el, id);
        io.observe(el);
      }
    }
    return () => io.disconnect();
    // revision re-subscribes the observer when the virtualizer's mounted rows
    // change (scroll); harmless (no-op dep) for a non-virtualized list.
  }, [items, flushAutoRead, listRef, rowRefs, revision]);

  // Cancel any pending flush on unmount.
  useEffect(
    () => () => {
      if (flushTimerRef.current != null)
        window.clearTimeout(flushTimerRef.current);
    },
    [],
  );
}
