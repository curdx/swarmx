/**
 * useSwarmFeed — subscribe to `/ws/swarm` and dispatch parsed `SwarmEvent`s.
 *
 * Single connection per component instance. Auto-reconnect with exponential
 * backoff (200ms → 4s cap). The server feed is broadcast-only with no resume;
 * UIs are expected to refetch their REST snapshots on reconnect (the
 * `onReconnect` hook is provided for exactly that).
 */

import { useEffect, useRef, useState } from "react";
import type { SwarmEvent } from "../api/types";

export type SwarmFeedStatus = "connecting" | "open" | "closed";

interface Options {
  onEvent: (ev: SwarmEvent) => void;
  /** Fired whenever the WS transitions to "open" (initial + after a reconnect). */
  onReconnect?: () => void;
}

const BACKOFF_INITIAL_MS = 200;
const BACKOFF_MAX_MS = 4000;

export function useSwarmFeed({ onEvent, onReconnect }: Options): SwarmFeedStatus {
  const [status, setStatus] = useState<SwarmFeedStatus>("connecting");
  // Pin the callbacks so we can update them without re-opening the socket.
  const onEventRef = useRef(onEvent);
  const onReconnectRef = useRef(onReconnect);
  useEffect(() => {
    onEventRef.current = onEvent;
  }, [onEvent]);
  useEffect(() => {
    onReconnectRef.current = onReconnect;
  }, [onReconnect]);

  useEffect(() => {
    let ws: WebSocket | null = null;
    let retry = BACKOFF_INITIAL_MS;
    let cancelled = false;
    let reconnectTimer: number | null = null;

    const connect = () => {
      if (cancelled) return;
      setStatus("connecting");
      const proto = window.location.protocol === "https:" ? "wss:" : "ws:";
      const url = `${proto}//${window.location.host}/ws/swarm`;
      const next = new WebSocket(url);
      ws = next;

      next.onopen = () => {
        if (cancelled) {
          next.close();
          return;
        }
        retry = BACKOFF_INITIAL_MS;
        setStatus("open");
        onReconnectRef.current?.();
      };

      next.onmessage = (msg) => {
        if (typeof msg.data !== "string") return;
        try {
          const ev = JSON.parse(msg.data) as SwarmEvent;
          // Ignore the server-side "lagged" sentinel which uses a different
          // top-level shape than SwarmEvent. SwarmEvent always carries a
          // discriminator we recognise.
          if (!ev || typeof (ev as { type?: unknown }).type !== "string") return;
          onEventRef.current(ev);
        } catch {
          // ignore malformed frames
        }
      };

      next.onclose = () => {
        if (cancelled) return;
        setStatus("closed");
        const delay = retry;
        retry = Math.min(retry * 2, BACKOFF_MAX_MS);
        reconnectTimer = window.setTimeout(connect, delay);
      };

      next.onerror = () => {
        // onclose will fire next; nothing to do here.
      };
    };

    connect();

    return () => {
      cancelled = true;
      if (reconnectTimer !== null) window.clearTimeout(reconnectTimer);
      if (ws) {
        // Avoid the onclose retry path firing during teardown.
        ws.onopen = null;
        ws.onmessage = null;
        ws.onclose = null;
        ws.onerror = null;
        try {
          ws.close();
        } catch {
          // ignore
        }
      }
    };
  }, []);

  return status;
}
