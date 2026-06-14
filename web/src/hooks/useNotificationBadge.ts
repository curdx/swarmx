/**
 * AppShell 顶栏 bell icon 的红点逻辑 — 不要重新实现一套通知 inbox（那
 * 是 /notifications route 的活），只回答一个问题："还有没有没读的通知"。
 *
 * 方向必须跟通知中心一致。早先红点用一个独立的「seenAt 时间戳」：popover
 * 一打开就 setSeenAt(now) 把红点抹掉，可通知中心仍按 id 维度的「已读集合」
 * （localStorage `flockmux:notif:read:v1`）算未读 —— 于是出现「红点没了但
 * 中心还显示一堆未读」的自相矛盾（瞄一眼 ≠ 读过）。
 *
 * 现在红点直接以中心的「已读集合」为唯一真相：useSwarmFeed 收到 message /
 * blackboard_changed 时,用与中心完全相同的稳定 id 方案（`msg-<id>` /
 * `bb-<path>`）记下「见过的事件 id」,只要其中存在不在已读集合里的 id,红点
 * 就亮。用户在中心把它标已读 → 已读集合写回 localStorage → storage 事件
 * 同步过来 → 红点自动熄灭。两个界面对「已读」是同一个口径,不会再打架。
 *
 * 仍只覆盖 inbox 真正会展示/回灌的两类事件（消息 + 黑板写入）；裸
 * `agent_state` 不亮红点（中心不持久化它,见 FAULT-015）。
 */

import { useCallback, useEffect, useRef, useState } from "react";
import { useSwarmFeed } from "./useSwarmFeed";
import { isHiddenWake } from "@/lib/notif";

// 与 /notifications 共用的「已读集合」key —— 红点的唯一真相,保证方向一致。
const READ_KEY = "flockmux:notif:read:v1";

function readReadSet(): Set<string> {
  try {
    const raw = window.localStorage.getItem(READ_KEY);
    if (!raw) return new Set();
    return new Set(JSON.parse(raw) as string[]);
  } catch {
    return new Set();
  }
}

export function useNotificationBadge() {
  // 本会话见过的事件 id（与中心同款稳定 id）。存在 ref 里给稳定的 feed 闭包
  // 读写,同时用一个计数 state 触发重算。
  const seenIdsRef = useRef<Set<string>>(new Set());
  const [, bump] = useState(0);
  const [readSet, setReadSet] = useState<Set<string>>(readReadSet);

  useSwarmFeed({
    onEvent: (ev) => {
      let id: string | null = null;
      if (ev.type === "message") {
        // 跟中心一致:被中心过滤掉的自动唤醒不该亮红点。
        if (isHiddenWake(ev)) return;
        id = `msg-${ev.id}`;
      } else if (ev.type === "blackboard_changed") {
        // worker 心跳（.progress.md）中心不展示,别亮红点。
        if (ev.path.endsWith(".progress.md")) return;
        id = `bb-${ev.path}`; // 稳定 id,与中心同款
      }
      if (id == null) return;
      if (seenIdsRef.current.has(id)) return;
      seenIdsRef.current.add(id);
      bump((n) => n + 1);
    },
  });

  /** 兼容旧调用点（AppShell 进入 /notifications 时调一次）。真正清红点靠中心
   *  把通知标已读后写回 READ_KEY —— 这里只把本会话见过的 id 也并进已读集合
   *  并持久化,使「访问通知中心」与「红点熄灭」方向一致:你看过的就算已读。 */
  const markSeen = useCallback(() => {
    if (seenIdsRef.current.size === 0) return;
    try {
      const cur = readReadSet();
      let changed = false;
      for (const id of seenIdsRef.current) {
        if (!cur.has(id)) {
          cur.add(id);
          changed = true;
        }
      }
      if (!changed) return;
      // 与中心相同的 500 上限,避免 key 无限膨胀。
      const arr = Array.from(cur).slice(-500);
      window.localStorage.setItem(READ_KEY, JSON.stringify(arr));
      setReadSet(new Set(arr));
    } catch {
      /* ignore */
    }
  }, []);

  // 中心把通知标已读 → 写 READ_KEY → 同步到红点。同窗口 storage 事件不触发,
  // 所以 markSeen 内部已直接 setReadSet;此处覆盖中心标已读 + 多窗口场景。
  useEffect(() => {
    const onStorage = (e: StorageEvent) => {
      if (e.key === READ_KEY) setReadSet(readReadSet());
    };
    window.addEventListener("storage", onStorage);
    return () => window.removeEventListener("storage", onStorage);
  }, []);

  // 红点亮 = 本会话见过、但不在已读集合里的事件 id 至少有一个。与中心
  // 「totalUnread = items 里 !read.has(id) 的条数」完全同向。
  let hasUnseen = false;
  for (const id of seenIdsRef.current) {
    if (!readSet.has(id)) {
      hasUnseen = true;
      break;
    }
  }

  return {
    hasUnseen,
    markSeen,
  };
}
