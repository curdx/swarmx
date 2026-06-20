// Chat-lifecycle debug logger.
//
// Why this exists: "I sent a message and the chat panel didn't show it / the
// left-side reply vanished" is a cross-boundary bug — the message crosses
// browser state → REST → backend store → broadcast → WS → back into browser
// state → render. Console logs alone only see the browser half. So every
// breadcrumb here ALSO ships to `POST /api/debug/log`, where the backend folds
// it into the SAME `tracing` stream (and `~/.flockmux/logs/flockmux.log`) as
// its own `flockmux::msg` / `flockmux::ws` lines. One file, one timeline, so a
// dropped message tells you exactly which hop lost it.
//
// Enabled in dev by default; in any build it can be forced on/off with
// `localStorage["flockmux:debug"] = "1" | "0"`.

import { HTTP_BASE } from "./apiBase";

type DebugEvent = {
  /** client wall clock (ms) */
  ts: number;
  /** per-page monotonic counter — survives same-ms ordering */
  seq: number;
  ev: string;
  data?: Record<string, unknown>;
};

let seq = 0;
let buffer: DebugEvent[] = [];
let flushTimer: ReturnType<typeof setTimeout> | null = null;

function enabled(): boolean {
  try {
    const override = localStorage.getItem("flockmux:debug");
    if (override === "1") return true;
    if (override === "0") return false;
  } catch {
    /* localStorage may be unavailable (SSR / privacy mode) — fall through */
  }
  return import.meta.env.DEV;
}

function flush(): void {
  flushTimer = null;
  if (buffer.length === 0) return;
  const events = buffer;
  buffer = [];
  // Fire-and-forget. `keepalive` lets the last batch survive a tab close.
  // NEVER route a failure here back through dlog() — that would loop.
  void fetch(HTTP_BASE + "/api/debug/log", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ events }),
    keepalive: true,
  }).catch(() => {
    /* debug sink is best-effort; losing a breadcrumb must never break the UI */
  });
}

/**
 * Log one chat-lifecycle breadcrumb. Cheap no-op when disabled.
 *
 * `ev` is a short dotted tag (e.g. "send.start", "refresh.replace",
 * "live.append", "ws.message"); `data` is free-form context (ids, counts).
 */
export function dlog(ev: string, data?: Record<string, unknown>): void {
  if (!enabled()) return;
  const e: DebugEvent = { ts: Date.now(), seq: ++seq, ev, data };
  // Console for live reading (chrome-devtools / the dev tools panel).
  // eslint-disable-next-line no-console
  console.log(
    `%c[FMX]%c #${e.seq} ${ev}`,
    "color:#7c3aed;font-weight:bold",
    "color:inherit",
    data ?? "",
  );
  buffer.push(e);
  // Coalesce a burst (e.g. send + optimistic + ws-echo) into one POST.
  if (flushTimer == null) flushTimer = setTimeout(flush, 400);
}
