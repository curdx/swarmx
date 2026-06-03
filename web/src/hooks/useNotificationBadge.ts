/**
 * AppShell 顶栏 bell icon 的红点逻辑 — 不要重新实现一套通知 inbox（那
 * 是 /notifications route 的活），只回答一个问题："自上次访问通知页之
 * 后，有没有新事件 worth 看一眼"。
 *
 * 触发条件：useSwarmFeed 收到 message / blackboard_changed / agent_state
 * 事件 → bump。这跟 /notifications 收集事件的口径一致，但状态独立存于
 * localStorage 的一个简单时间戳：`flockmux:notif:badgeAt`。访问通知页
 * 时清掉 (= setLastSeen(now))。
 *
 * 简化原因：把 NotificationsRoute 的完整 state 全局化要 Context Provider
 * + 全局订阅，工程量太大。badge 红点不需要精确数字，知道"有没有"就够。
 */

import { useCallback, useEffect, useState } from "react";
import { useSwarmFeed } from "./useSwarmFeed";

const SEEN_KEY = "flockmux:notif:seenAt";
const LATEST_KEY = "flockmux:notif:latestAt";

function readSeen(): number {
  try {
    const v = window.localStorage.getItem(SEEN_KEY);
    return v ? Number(v) : 0;
  } catch {
    return 0;
  }
}

function readLatest(): number {
  try {
    const v = window.localStorage.getItem(LATEST_KEY);
    return v ? Number(v) : 0;
  } catch {
    return 0;
  }
}

function writeLatest(at: number) {
  try {
    window.localStorage.setItem(LATEST_KEY, String(at));
  } catch {
    /* ignore */
  }
}

export function useNotificationBadge() {
  const [seenAt, setSeenAt] = useState<number>(readSeen);
  const [latestAt, setLatestAt] = useState<number>(readLatest);

  useSwarmFeed({
    onEvent: (ev) => {
      // Only bump on events the inbox actually shows/seeds: messages and
      // blackboard writes (both backfilled on the /notifications mount). Bare
      // `agent_state` transitions used to light the dot too, but the inbox
      // doesn't persist those — so a fresh spawn lit the badge while the inbox
      // stayed empty ("red dot → empty inbox", FAULT-015). Errors/completions
      // still badge: they arrive as blackboard `.error`/`.done` writes + messages.
      let at: number | null = null;
      if (ev.type === "message") at = ev.sent_at;
      else if (ev.type === "blackboard_changed") at = ev.at;
      if (at == null) return;
      setLatestAt((prev) => {
        const next = Math.max(prev, at!);
        if (next !== prev) writeLatest(next);
        return next;
      });
    },
  });

  /** Mark current state as seen — call when the user opens /notifications. */
  const markSeen = useCallback(() => {
    const now = Date.now();
    try {
      window.localStorage.setItem(SEEN_KEY, String(now));
    } catch {
      /* ignore */
    }
    setSeenAt(now);
  }, []);

  // Other tabs (Tauri multi-window if it ever happens) → keep in sync.
  useEffect(() => {
    const onStorage = (e: StorageEvent) => {
      if (e.key === SEEN_KEY) setSeenAt(readSeen());
      if (e.key === LATEST_KEY) setLatestAt(readLatest());
    };
    window.addEventListener("storage", onStorage);
    return () => window.removeEventListener("storage", onStorage);
  }, []);

  return {
    hasUnseen: latestAt > seenAt,
    markSeen,
  };
}
