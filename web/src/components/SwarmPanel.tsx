/**
 * SwarmPanel — right-hand side drawer with three tabs: messages, blackboard,
 * recordings. Owns the single `/ws/swarm` subscription and routes events
 * down to the appropriate child via cheap props.
 *
 * M5a: maintains an `unreadByFrom` map. The map is seeded from a REST
 * snapshot on mount + each reconnect, then maintained incrementally from
 * `message` / `message_read` events. We index by `from_agent` (rather than
 * `to_agent`) because the UI is a single-user observer — "X unread from
 * agent A" is what the user wants to see in the tab badge.
 */

import { useCallback, useEffect, useRef, useState } from "react";
import { useSwarmFeed } from "../hooks/useSwarmFeed";
import { api } from "../api/http";
import type { MessageRecord, SwarmEvent } from "../api/types";
import { MessagesPanel } from "./MessagesPanel";
import { BlackboardPanel } from "./BlackboardPanel";
import { RecordingsPanel } from "./RecordingsPanel";

// The collaboration graph lives in the primary product at /chat/:wsId/dag
// (ReactFlow + dagre). The old hand-rolled SVG GraphPanel that used to be a
// tab here was deleted — there's now ONE DAG implementation (edge logic in
// lib/dagEdgeDerivation), so the two can no longer drift.
type Tab = "messages" | "blackboard" | "recordings";

// 中文显示名：tab 内部 key 仍用英文（避免改一堆 switch/比较），
// 渲染时映射到中文。
const TAB_LABELS: Record<Tab, string> = {
  messages: "消息",
  blackboard: "共享区",
  recordings: "录像",
};

export function SwarmPanel() {
  const [tab, setTab] = useState<Tab>("messages");
  const [liveMessage, setLiveMessage] = useState<MessageRecord | null>(null);
  const [liveRead, setLiveRead] = useState<{
    ids: number[];
    to_agent: string;
    at: number;
  } | null>(null);
  const [liveChange, setLiveChange] = useState<{
    path: string;
    agent_id: string | null;
    op: string;
  } | null>(null);
  const [recordingsTick, setRecordingsTick] = useState(0);
  const [unreadByFrom, setUnreadByFrom] = useState<Record<string, number>>({});

  // Reverse lookup: id → from_agent. Lets `message_read` events decrement
  // the right bucket without forcing the panel to refetch the whole list.
  const idToFromRef = useRef<Map<number, string>>(new Map());

  const recomputeUnread = useCallback(async () => {
    try {
      const rows = await api.listMessages({ limit: 200 });
      const counts: Record<string, number> = {};
      const ids = new Map<number, string>();
      for (const m of rows) {
        ids.set(m.id, m.from_agent);
        if (m.read_at === null) {
          counts[m.from_agent] = (counts[m.from_agent] ?? 0) + 1;
        }
      }
      idToFromRef.current = ids;
      setUnreadByFrom(counts);
    } catch {
      // best-effort; leave existing state
    }
  }, []);

  useEffect(() => {
    recomputeUnread();
  }, [recomputeUnread]);

  const status = useSwarmFeed({
    onEvent: (ev: SwarmEvent) => {
      switch (ev.type) {
        case "message": {
          const rec: MessageRecord = {
            id: ev.id,
            from_agent: ev.from_agent,
            to_agent: ev.to_agent,
            kind: ev.kind,
            body: ev.body,
            sent_at: ev.sent_at,
            delivered_at: null,
            read_at: null,
            in_reply_to: ev.in_reply_to ?? null,
            thread_id: ev.thread_id ?? null,
            meta: ev.meta ?? null,
            thought_trace: ev.thought_trace ?? null,
          };
          setLiveMessage(rec);
          idToFromRef.current.set(ev.id, ev.from_agent);
          // Every newly arrived message is unread by definition — the agent
          // who eventually reads it will trigger `message_read` to decrement.
          setUnreadByFrom((prev) => ({
            ...prev,
            [ev.from_agent]: (prev[ev.from_agent] ?? 0) + 1,
          }));
          break;
        }
        case "message_read": {
          setLiveRead({ ids: ev.ids, to_agent: ev.to_agent, at: ev.at });
          setUnreadByFrom((prev) => {
            const next = { ...prev };
            for (const id of ev.ids) {
              const from = idToFromRef.current.get(id);
              if (!from) continue;
              const cur = next[from] ?? 0;
              const dec = Math.max(0, cur - 1);
              if (dec === 0) delete next[from];
              else next[from] = dec;
            }
            return next;
          });
          break;
        }
        case "blackboard_changed":
          setLiveChange({
            path: ev.path,
            agent_id: ev.agent_id,
            op: ev.op,
          });
          break;
        case "agent_state":
          // Only exits matter to this panel now (the Recordings tab); the
          // graph that consumed every state change moved to /chat/:wsId/dag.
          if (ev.state === "exited") {
            setRecordingsTick((t) => t + 1);
          }
          break;
      }
    },
    onReconnect: () => {
      setRecordingsTick((t) => t + 1);
      recomputeUnread();
    },
  });

  const totalUnread = Object.values(unreadByFrom).reduce((a, b) => a + b, 0);

  return (
    <aside style={container}>
      <div style={tabBar}>
        {(["messages", "blackboard", "recordings"] as Tab[]).map((t) => (
          <button
            key={t}
            onClick={() => setTab(t)}
            style={{
              ...tabButton,
              background: tab === t ? "#1e3a8a" : "transparent",
              color: tab === t ? "#e2e8f0" : "#94a3b8",
            }}
          >
            {TAB_LABELS[t]}
            {t === "messages" && totalUnread > 0 && (
              <span style={tabBadge}>{totalUnread}</span>
            )}
          </button>
        ))}
        <span style={statusDot} title={`协作 WS：${status}`}>
          <span
            style={{
              display: "inline-block",
              width: 8,
              height: 8,
              borderRadius: "50%",
              background:
                status === "open"
                  ? "#22c55e"
                  : status === "connecting"
                    ? "#fbbf24"
                    : "#ef4444",
            }}
          />
        </span>
      </div>
      <div style={body}>
        {tab === "messages" && (
          <MessagesPanel
            liveMessage={liveMessage}
            liveRead={liveRead}
            unreadByFrom={unreadByFrom}
          />
        )}
        {tab === "blackboard" && <BlackboardPanel liveChange={liveChange} />}
        {tab === "recordings" && <RecordingsPanel refreshTick={recordingsTick} />}
      </div>
    </aside>
  );
}

const container: React.CSSProperties = {
  width: 360,
  borderLeft: "1px solid #374151",
  background: "#0f172a",
  display: "flex",
  flexDirection: "column",
  minHeight: 0,
};

const tabBar: React.CSSProperties = {
  display: "flex",
  borderBottom: "1px solid #374151",
  background: "#1f2937",
};

const tabButton: React.CSSProperties = {
  flex: 1,
  border: "none",
  borderRight: "1px solid #374151",
  padding: "6px 0",
  fontSize: 12,
  cursor: "pointer",
  display: "inline-flex",
  alignItems: "center",
  justifyContent: "center",
  gap: 6,
};

const tabBadge: React.CSSProperties = {
  background: "#dc2626",
  color: "#fff",
  borderRadius: 8,
  padding: "0 6px",
  fontSize: 10,
  fontWeight: 600,
  lineHeight: "14px",
};

const statusDot: React.CSSProperties = {
  display: "flex",
  alignItems: "center",
  padding: "0 8px",
};

const body: React.CSSProperties = {
  flex: 1,
  minHeight: 0,
  display: "flex",
  flexDirection: "column",
};
