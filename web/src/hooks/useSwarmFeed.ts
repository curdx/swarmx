/**
 * useSwarmFeed — subscribe to `/ws/swarm` and dispatch parsed `SwarmEvent`s.
 *
 * ONE shared WebSocket for the whole page, multiplexed to every subscriber.
 *
 * Previously each component that called this hook opened its own socket
 * (Shell, Chat, Dag, Ledger, NotificationPopover, AgentDrawer, the chat
 * breadcrumbs, the wizard…). On every navigation the mounting/unmounting
 * views tore down and rebuilt their sockets, and closing a socket that was
 * still mid-handshake logs "WebSocket is closed before the connection is
 * established" — the churn the UI audit flagged on each route change.
 *
 * Now a module-level singleton holds the connection. Subscribers add/remove
 * listeners; the socket opens on the first subscriber and lingers briefly
 * after the last one leaves so a nav unmount→remount within the same tick
 * doesn't bounce it. The server feed is broadcast-only with no resume, so a
 * subscriber's `onReconnect` fires on initial open AND whenever it joins an
 * already-open socket — that's the cue to refetch REST snapshots.
 *
 * Auto-reconnect uses exponential backoff (200ms → 4s cap) while there is at
 * least one subscriber.
 */

import { useEffect, useRef, useState } from "react";
import type { SwarmEvent } from "../api/types";
import { WS_HOST, WS_PROTO } from "../lib/apiBase";

export type SwarmFeedStatus = "connecting" | "open" | "closed";

interface Options {
  onEvent: (ev: SwarmEvent) => void;
  /** Fired when this subscriber's feed becomes live: on the initial open,
   *  after a reconnect, and immediately on subscribe if the shared socket is
   *  already open. Use it to refetch REST snapshots. */
  onReconnect?: () => void;
}

interface Sub {
  onEvent: (ev: SwarmEvent) => void;
  onReconnect?: () => void;
}

const BACKOFF_INITIAL_MS = 200;
const BACKOFF_MAX_MS = 4000;
// Keep the socket alive this long after the last subscriber unmounts, so a
// route change (unmount old view → mount new view) reuses it instead of
// closing + reopening (which causes the mid-handshake-close warning).
const LINGER_CLOSE_MS = 5000;

const subs = new Set<Sub>();
const statusListeners = new Set<(s: SwarmFeedStatus) => void>();
let ws: WebSocket | null = null;
let status: SwarmFeedStatus = "closed";
let retry = BACKOFF_INITIAL_MS;
let reconnectTimer: number | null = null;
let lingerTimer: number | null = null;

function setStatus(s: SwarmFeedStatus) {
  status = s;
  for (const l of statusListeners) l(s);
}

function connect() {
  if (
    ws &&
    (ws.readyState === WebSocket.OPEN || ws.readyState === WebSocket.CONNECTING)
  ) {
    return;
  }
  setStatus("connecting");
  const next = new WebSocket(`${WS_PROTO}//${WS_HOST}/ws/swarm`);
  ws = next;

  next.onopen = () => {
    retry = BACKOFF_INITIAL_MS;
    setStatus("open");
    for (const s of subs) {
      try {
        s.onReconnect?.();
      } catch (err) {
        console.warn("swarm onReconnect threw", err);
      }
    }
  };

  next.onmessage = (msg) => {
    if (typeof msg.data !== "string") return;
    let parsed: unknown;
    try {
      parsed = JSON.parse(msg.data);
    } catch {
      return;
    }
    if (!parsed || typeof (parsed as { type?: unknown }).type !== "string") {
      // Server "lagged" sentinel or other non-event frame.
      return;
    }
    for (const s of subs) {
      try {
        s.onEvent(parsed as SwarmEvent);
      } catch (err) {
        console.warn("swarm event handler threw", err, parsed);
      }
    }
  };

  next.onclose = () => {
    if (ws === next) ws = null;
    setStatus("closed");
    if (subs.size === 0) return; // nobody listening — don't reconnect
    const delay = retry;
    retry = Math.min(retry * 2, BACKOFF_MAX_MS);
    if (reconnectTimer !== null) window.clearTimeout(reconnectTimer);
    reconnectTimer = window.setTimeout(() => {
      reconnectTimer = null;
      connect();
    }, delay);
  };

  next.onerror = () => {
    // onclose fires next; nothing to do here.
  };
}

function ensureConnected() {
  if (lingerTimer !== null) {
    window.clearTimeout(lingerTimer);
    lingerTimer = null;
  }
  connect();
}

function maybeDisconnect() {
  if (subs.size > 0) return;
  if (lingerTimer !== null) window.clearTimeout(lingerTimer);
  lingerTimer = window.setTimeout(() => {
    lingerTimer = null;
    if (subs.size > 0) return; // a new subscriber arrived during the linger
    if (reconnectTimer !== null) {
      window.clearTimeout(reconnectTimer);
      reconnectTimer = null;
    }
    if (ws) {
      const dead = ws;
      ws = null;
      dead.onopen = null;
      dead.onmessage = null;
      dead.onclose = null;
      dead.onerror = null;
      try {
        dead.close();
      } catch {
        /* ignore */
      }
    }
    setStatus("closed");
  }, LINGER_CLOSE_MS);
}

/** Passive read of the shared socket's REAL connection status — for UI that
 *  must reflect "is the live feed actually connected" (e.g. the LIVE badge,
 *  which otherwise stays lit off a stale REST snapshot after the WS drops).
 *  Does NOT register an event subscriber and does NOT open/keep-alive the
 *  socket — the real feed consumers do that; this just observes. */
export function useSwarmFeedStatus(): SwarmFeedStatus {
  const [s, setS] = useState<SwarmFeedStatus>(status);
  useEffect(() => {
    setS(status); // reconcile any change between render and this effect
    statusListeners.add(setS);
    return () => {
      statusListeners.delete(setS);
    };
  }, []);
  return s;
}

export function useSwarmFeed({ onEvent, onReconnect }: Options): SwarmFeedStatus {
  // Seed "connecting" (not the module's idle "closed") on a cold mount — the
  // effect below is about to open the socket, so showing a red "closed" dot
  // for one frame before it flips to yellow would be a misleading flash. If
  // the shared socket is already open, reflect that immediately.
  const [s, setS] = useState<SwarmFeedStatus>(
    status === "closed" ? "connecting" : status,
  );
  // Latest callbacks without re-subscribing — the closures the singleton
  // holds always read through this ref.
  const cbRef = useRef<Sub>({ onEvent, onReconnect });
  cbRef.current.onEvent = onEvent;
  cbRef.current.onReconnect = onReconnect;

  useEffect(() => {
    const sub: Sub = {
      onEvent: (ev) => cbRef.current.onEvent(ev),
      onReconnect: () => cbRef.current.onReconnect?.(),
    };
    subs.add(sub);
    statusListeners.add(setS);
    ensureConnected();
    // Joining an already-open socket still needs the initial refetch — the
    // shared socket won't re-fire onopen just for this new subscriber.
    if (status === "open") {
      try {
        sub.onReconnect?.();
      } catch {
        /* ignore */
      }
    }
    return () => {
      subs.delete(sub);
      statusListeners.delete(setS);
      maybeDisconnect();
    };
    // Mount-once by design: this registers/unregisters one subscriber on the
    // module-level shared socket. The latest onEvent/onReconnect are read
    // through cbRef (refreshed every render above), so they're deliberately
    // NOT deps — adding them would re-run this effect on every render and
    // thrash the shared WS connection. `status` is read for the join-time
    // refetch only; a stale read is harmless (the listener still fires later).
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return s;
}
