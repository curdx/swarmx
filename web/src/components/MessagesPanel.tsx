/**
 * MessagesPanel — list + composer for /api/message.
 *
 * Live updates piggy-back on the parent's /ws/swarm subscription via three
 * props: `liveMessage` (a fresh inbound message), `liveRead` (someone marked
 * messages read — apply read_at locally), `unreadByFrom` (parent-maintained
 * tally rendered as per-sender badges).
 *
 * M5a additions:
 *   - per-message ✓ / ★ marker (read vs. unread)
 *   - `↩ #<id>` lineage rendered on the meta row; click scrolls to parent
 *   - "Reply" action pre-fills composer with `to=from`, `in_reply_to=id`
 *   - "by sender" header lists unread counts as badges
 *
 * UI does NOT auto-mark-read — opening the panel is not the same as a human
 * having actually read a message. Marking is done explicitly (via the row
 * ✓ button) or implicitly via `swarm_list_messages` from the agent side.
 */

import { useEffect, useMemo, useRef, useState } from "react";
import { api } from "../api/http";
import type { MessageRecord } from "../api/types";

interface Props {
  /** Latest swarm `message` event observed by the parent (or null). */
  liveMessage: MessageRecord | null;
  /** Latest swarm `message_read` event observed by the parent (or null). */
  liveRead: { ids: number[]; to_agent: string; at: number } | null;
  /** Parent-maintained unread tally keyed by from_agent. */
  unreadByFrom: Record<string, number>;
}

const KIND_DEFAULT = "note";

export function MessagesPanel({ liveMessage, liveRead, unreadByFrom }: Props) {
  const [items, setItems] = useState<MessageRecord[]>([]);
  const [filter, setFilter] = useState("");
  const [from, setFrom] = useState("");
  const [to, setTo] = useState("");
  const [body, setBody] = useState("");
  const [inReplyTo, setInReplyTo] = useState<number | null>(null);
  const [sending, setSending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [marking, setMarking] = useState<number | null>(null);
  const [showBySender, setShowBySender] = useState(false);
  const listRef = useRef<HTMLDivElement>(null);
  const rowRefs = useRef<Map<number, HTMLDivElement | null>>(new Map());
  const [highlightId, setHighlightId] = useState<number | null>(null);

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

  // Reflect remote mark_read events in the local list so ★ → ✓ live.
  useEffect(() => {
    if (!liveRead) return;
    const idSet = new Set(liveRead.ids);
    setItems((prev) =>
      prev.map((m) =>
        idSet.has(m.id) && m.to_agent === liveRead.to_agent && m.read_at === null
          ? { ...m, read_at: liveRead.at }
          : m,
      ),
    );
  }, [liveRead]);

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
        in_reply_to: inReplyTo ?? undefined,
      });
      setItems((prev) =>
        prev.some((m) => m.id === rec.id) ? prev : [...prev, rec],
      );
      setBody("");
      setInReplyTo(null);
      setError(null);
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setSending(false);
    }
  };

  const startReply = (m: MessageRecord) => {
    setTo(m.from_agent);
    if (m.to_agent && !from.trim()) setFrom(m.to_agent);
    setInReplyTo(m.id);
  };

  const markRead = async (m: MessageRecord) => {
    if (m.read_at !== null) return;
    setMarking(m.id);
    try {
      const res = await api.markMessagesRead(m.to_agent, [m.id]);
      // Apply locally — the broadcast event will arrive in parallel and
      // setItems' equality check makes the second update a no-op.
      if (res.marked.includes(m.id)) {
        setItems((prev) =>
          prev.map((x) =>
            x.id === m.id ? { ...x, read_at: res.at } : x,
          ),
        );
      }
      setError(null);
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setMarking(null);
    }
  };

  const jumpToParent = (parentId: number) => {
    const el = rowRefs.current.get(parentId);
    if (!el) return;
    el.scrollIntoView({ behavior: "smooth", block: "center" });
    setHighlightId(parentId);
    window.setTimeout(() => setHighlightId((cur) => (cur === parentId ? null : cur)), 1200);
  };

  const senders = Object.entries(unreadByFrom).filter(([, n]) => n > 0);

  return (
    <div style={{ display: "flex", flexDirection: "column", height: "100%" }}>
      <div style={headerRow}>
        <input
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          placeholder="过滤（发送方 / 接收方 / 正文）"
          style={input}
        />
        <button onClick={refresh} title="刷新">
          ↻
        </button>
      </div>

      <div style={bySenderHeader}>
        <button
          onClick={() => setShowBySender((v) => !v)}
          style={bySenderToggle}
          title="按发送方查看未读"
        >
          {showBySender ? "▾" : "▸"} 按发送方未读 ({senders.length})
        </button>
        {showBySender && (
          <div style={bySenderList}>
            {senders.length === 0 && (
              <span style={{ color: "#64748b", fontSize: 11 }}>无</span>
            )}
            {senders.map(([who, n]) => (
              <span key={who} style={bySenderRow}>
                <span style={{ color: "#a5b4fc" }}>{who}</span>
                <span style={badge}>{n}</span>
              </span>
            ))}
          </div>
        )}
      </div>

      {error && <div style={errorRow}>{error}</div>}

      <div ref={listRef} style={listStyle}>
        {visible.length === 0 && (
          <div style={emptyHint}>暂无消息</div>
        )}
        {visible.map((m) => {
          const unread = m.read_at === null;
          const highlighted = highlightId === m.id;
          return (
            <div
              key={m.id}
              ref={(el) => {
                if (el) rowRefs.current.set(m.id, el);
                else rowRefs.current.delete(m.id);
              }}
              style={{
                ...messageRow,
                borderLeftColor: unread ? "#fbbf24" : "#374151",
                background: highlighted ? "#1e3a8a" : "transparent",
                transition: "background 200ms",
              }}
            >
              <div style={messageMeta}>
                <span title={unread ? "未读" : "已读"}>
                  {unread ? "★" : "✓"}
                </span>
                <span style={{ marginLeft: 4, color: "#94a3b8" }}>
                  #{m.id}
                </span>
                <span style={{ color: "#a5b4fc", marginLeft: 6 }}>{m.from_agent}</span>
                <span style={{ color: "#64748b" }}> → </span>
                <span style={{ color: "#86efac" }}>{m.to_agent}</span>
                <span style={{ color: "#64748b", marginLeft: 6 }}>
                  {m.kind} · {formatTime(m.sent_at)}
                </span>
                {m.in_reply_to != null && (
                  <button
                    onClick={() => jumpToParent(m.in_reply_to!)}
                    style={replyLink}
                    title="跳转到被回复的消息"
                  >
                    ↩ #{m.in_reply_to}
                  </button>
                )}
                <span style={{ flex: 1 }} />
                <button
                  onClick={() => startReply(m)}
                  style={rowAction}
                  title="回复这条消息"
                >
                  回复
                </button>
                {unread && (
                  <button
                    onClick={() => markRead(m)}
                    style={rowAction}
                    disabled={marking === m.id}
                    title="标记为已读"
                  >
                    ✓
                  </button>
                )}
              </div>
              <div style={{ ...messageBody, opacity: unread ? 1 : 0.7 }}>{m.body}</div>
            </div>
          );
        })}
      </div>

      <div style={composer}>
        {inReplyTo != null && (
          <div style={replyBanner}>
            正在回复 #{inReplyTo}
            <button
              onClick={() => setInReplyTo(null)}
              style={replyClear}
              title="取消回复"
            >
              ✕
            </button>
          </div>
        )}
        <div style={{ display: "flex", gap: 4 }}>
          <input
            value={from}
            onChange={(e) => setFrom(e.target.value)}
            placeholder="发送方（留空=system）"
            style={{ ...input, flex: 1 }}
          />
          <input
            value={to}
            onChange={(e) => setTo(e.target.value)}
            placeholder="接收方（agent id）"
            style={{ ...input, flex: 1 }}
          />
        </div>
        <textarea
          value={body}
          onChange={(e) => setBody(e.target.value)}
          placeholder="消息正文"
          rows={3}
          style={{ ...input, resize: "vertical" }}
        />
        <div style={{ display: "flex", justifyContent: "flex-end" }}>
          <button onClick={send} disabled={sending || !to.trim() || !body.trim()}>
            发送
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

const bySenderHeader: React.CSSProperties = {
  borderBottom: "1px solid #374151",
  padding: "4px 8px",
  display: "flex",
  flexDirection: "column",
  gap: 4,
};

const bySenderToggle: React.CSSProperties = {
  background: "transparent",
  border: "none",
  color: "#94a3b8",
  fontSize: 11,
  textAlign: "left",
  cursor: "pointer",
  padding: 0,
};

const bySenderList: React.CSSProperties = {
  display: "flex",
  flexWrap: "wrap",
  gap: 6,
  paddingTop: 2,
};

const bySenderRow: React.CSSProperties = {
  display: "inline-flex",
  alignItems: "center",
  gap: 4,
  fontSize: 11,
  background: "#0b1220",
  borderRadius: 4,
  padding: "2px 6px",
};

const badge: React.CSSProperties = {
  background: "#dc2626",
  color: "#fff",
  borderRadius: 8,
  padding: "0 5px",
  fontSize: 10,
  fontWeight: 600,
  lineHeight: "14px",
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
  paddingRight: 4,
  paddingTop: 2,
  paddingBottom: 2,
};

const messageMeta: React.CSSProperties = {
  fontSize: 10,
  marginBottom: 2,
  display: "flex",
  alignItems: "center",
  gap: 2,
  flexWrap: "wrap",
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

const replyBanner: React.CSSProperties = {
  display: "flex",
  alignItems: "center",
  gap: 8,
  background: "#1e3a8a",
  color: "#cbd5f5",
  fontSize: 11,
  padding: "3px 8px",
  borderRadius: 4,
};

const replyClear: React.CSSProperties = {
  background: "transparent",
  border: "none",
  color: "#cbd5f5",
  cursor: "pointer",
  fontSize: 12,
  padding: 0,
};

const replyLink: React.CSSProperties = {
  background: "transparent",
  border: "none",
  color: "#fbbf24",
  cursor: "pointer",
  fontSize: 10,
  padding: 0,
  marginLeft: 6,
};

const rowAction: React.CSSProperties = {
  background: "transparent",
  border: "1px solid #374151",
  borderRadius: 3,
  color: "#94a3b8",
  cursor: "pointer",
  fontSize: 10,
  marginLeft: 4,
  padding: "0 4px",
};
