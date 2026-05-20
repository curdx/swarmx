/**
 * SwarmPanel — right-hand side drawer with three tabs: messages, blackboard,
 * recordings. Owns the single `/ws/swarm` subscription and routes events
 * down to the appropriate child via cheap props.
 */

import { useState } from "react";
import { useSwarmFeed } from "../hooks/useSwarmFeed";
import type { MessageRecord, SwarmEvent } from "../api/types";
import { MessagesPanel } from "./MessagesPanel";
import { BlackboardPanel } from "./BlackboardPanel";
import { RecordingsPanel } from "./RecordingsPanel";

type Tab = "messages" | "blackboard" | "recordings";

export function SwarmPanel() {
  const [tab, setTab] = useState<Tab>("messages");
  const [liveMessage, setLiveMessage] = useState<MessageRecord | null>(null);
  const [liveChange, setLiveChange] = useState<{
    path: string;
    agent_id: string | null;
    op: string;
  } | null>(null);
  const [recordingsTick, setRecordingsTick] = useState(0);

  const status = useSwarmFeed({
    onEvent: (ev: SwarmEvent) => {
      switch (ev.type) {
        case "message":
          setLiveMessage({
            id: ev.id,
            from_agent: ev.from_agent,
            to_agent: ev.to_agent,
            kind: ev.kind,
            body: ev.body,
            sent_at: ev.sent_at,
            delivered_at: null,
            read_at: null,
          });
          break;
        case "blackboard_changed":
          setLiveChange({
            path: ev.path,
            agent_id: ev.agent_id,
            op: ev.op,
          });
          break;
        case "agent_state":
          // Bump recordings list when an agent transitions to exited — its
          // recording (if any) has just been finalized.
          if (ev.state === "exited") {
            setRecordingsTick((t) => t + 1);
          }
          break;
      }
    },
    onReconnect: () => {
      // Force a refresh tick across the dependent panels.
      setRecordingsTick((t) => t + 1);
    },
  });

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
            {t}
          </button>
        ))}
        <span style={statusDot} title={`ws/swarm: ${status}`}>
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
        {tab === "messages" && <MessagesPanel liveMessage={liveMessage} />}
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
