/**
 * MessagesPanel — list + composer for /api/message.
 *
 * Live updates piggy-back on the parent's /ws/swarm subscription via the
 * `liveMessage` prop: whenever the parent sees a `message` event, it bumps
 * `liveMessage`; this panel appends or refreshes accordingly.
 */

import { useEffect, useMemo, useRef, useState } from "react";
import { api } from "../api/http";
import type { MessageRecord } from "../api/types";

interface Props {
  /** Latest swarm `message` event observed by the parent (or null). */
  liveMessage: MessageRecord | null;
}

const KIND_DEFAULT = "note";

export function MessagesPanel({ liveMessage }: Props) {
  const [items, setItems] = useState<MessageRecord[]>([]);
  const [filter, setFilter] = useState("");
  const [from, setFrom] = useState("");
  const [to, setTo] = useState("");
  const [body, setBody] = useState("");
  const [sending, setSending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const listRef = useRef<HTMLDivElement>(null);

  const refresh = async () => {
    try {
      const rows = await api.listMessages({ limit: 200 });
      // server orders DESC by id; reverse to chat-style chronological.
      setItems(rows.slice().reverse());
      setError(null);
    } catch (e) {
      setError((e as Error).message);
    }
  };

  useEffect(() => {
    refresh();
  }, []);

  useEffect(() => {
    if (!liveMessage) return;
    setItems((prev) =>
      prev.some((m) => m.id === liveMessage.id) ? prev : [...prev, liveMessage],
    );
  }, [liveMessage]);

  useEffect(() => {
    const el = listRef.current;
    if (!el) return;
    el.scrollTop = el.scrollHeight;
  }, [items.length]);

  const visible = useMemo(() => {
    if (!filter) return items;
    const f = filter.toLowerCase();
    return items.filter(
      (m) =>
        m.from_agent.toLowerCase().includes(f) ||
        m.to_agent.toLowerCase().includes(f) ||
        m.body.toLowerCase().includes(f),
    );
  }, [items, filter]);

  const send = async () => {
    if (!to.trim() || !body.trim()) return;
    setSending(true);
    try {
      const rec = await api.sendMessage({
        from: from.trim() || undefined,
        to: to.trim(),
        kind: KIND_DEFAULT,
        body,
      });
      setItems((prev) =>
        prev.some((m) => m.id === rec.id) ? prev : [...prev, rec],
      );
      setBody("");
      setError(null);
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setSending(false);
    }
  };

  return (
    <div style={{ display: "flex", flexDirection: "column", height: "100%" }}>
      <div style={headerRow}>
        <input
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          placeholder="filter (from / to / body)"
          style={input}
        />
        <button onClick={refresh} title="refresh">
          ↻
        </button>
      </div>

      {error && <div style={errorRow}>{error}</div>}

      <div ref={listRef} style={listStyle}>
        {visible.length === 0 && (
          <div style={emptyHint}>No messages yet.</div>
        )}
        {visible.map((m) => (
          <div key={m.id} style={messageRow}>
            <div style={messageMeta}>
              <span style={{ color: "#a5b4fc" }}>{m.from_agent}</span>
              <span style={{ color: "#64748b" }}> → </span>
              <span style={{ color: "#86efac" }}>{m.to_agent}</span>
              <span style={{ color: "#64748b", marginLeft: 6 }}>
                {m.kind} · {formatTime(m.sent_at)}
              </span>
            </div>
            <div style={messageBody}>{m.body}</div>
          </div>
        ))}
      </div>

      <div style={composer}>
        <div style={{ display: "flex", gap: 4 }}>
          <input
            value={from}
            onChange={(e) => setFrom(e.target.value)}
            placeholder="from (blank = system)"
            style={{ ...input, flex: 1 }}
          />
          <input
            value={to}
            onChange={(e) => setTo(e.target.value)}
            placeholder="to (agent id)"
            style={{ ...input, flex: 1 }}
          />
        </div>
        <textarea
          value={body}
          onChange={(e) => setBody(e.target.value)}
          placeholder="message body"
          rows={3}
          style={{ ...input, resize: "vertical" }}
        />
        <div style={{ display: "flex", justifyContent: "flex-end" }}>
          <button onClick={send} disabled={sending || !to.trim() || !body.trim()}>
            send
          </button>
        </div>
      </div>
    </div>
  );
}

function formatTime(ms: number): string {
  const d = new Date(ms);
  return d.toLocaleTimeString();
}

const headerRow: React.CSSProperties = {
  display: "flex",
  gap: 4,
  padding: "6px 8px",
  borderBottom: "1px solid #374151",
};

const input: React.CSSProperties = {
  background: "#0b1220",
  color: "#e2e8f0",
  border: "1px solid #374151",
  borderRadius: 4,
  padding: "4px 6px",
  fontSize: 12,
  fontFamily: "inherit",
};

const errorRow: React.CSSProperties = {
  color: "#fca5a5",
  fontSize: 11,
  padding: "4px 8px",
  background: "#1f2937",
};

const listStyle: React.CSSProperties = {
  flex: 1,
  overflowY: "auto",
  padding: "6px 8px",
  display: "flex",
  flexDirection: "column",
  gap: 6,
  minHeight: 0,
};

const emptyHint: React.CSSProperties = {
  color: "#64748b",
  fontSize: 12,
  textAlign: "center",
  marginTop: 16,
};

const messageRow: React.CSSProperties = {
  borderLeft: "2px solid #374151",
  paddingLeft: 6,
};

const messageMeta: React.CSSProperties = {
  fontSize: 10,
  marginBottom: 2,
};

const messageBody: React.CSSProperties = {
  fontSize: 12,
  whiteSpace: "pre-wrap",
  wordBreak: "break-word",
  color: "#e2e8f0",
};

const composer: React.CSSProperties = {
  borderTop: "1px solid #374151",
  padding: "6px 8px",
  display: "flex",
  flexDirection: "column",
  gap: 4,
};
