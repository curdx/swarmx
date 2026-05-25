/**
 * MessagesPanel — list + composer for /api/message.
 *
 * Tailwind-rewritten in UI/F.1 to match the new neutral surface theme;
 * behaviour identical to the original M5a inline-styled version.
 *
 * Live updates piggy-back on the parent's /ws/swarm subscription via three
 * props: `liveMessage` (a fresh inbound message), `liveRead` (someone marked
 * messages read — apply read_at locally), `unreadByFrom` (parent-maintained
 * tally rendered as per-sender badges).
 *
 * Functional contract preserved:
 *   - per-message ✓ / ★ marker (read vs. unread)
 *   - `↩ #<id>` lineage rendered on the meta row; click scrolls to parent
 *   - "Reply" action pre-fills composer with `to=from`, `in_reply_to=id`
 *   - "by sender" header lists unread counts as badges
 *   - UI does NOT auto-mark-read — opening the panel is not the same as a
 *     human having actually read a message.
 */

import { useEffect, useMemo, useRef, useState } from "react";
import {
  ChevronDown,
  ChevronRight,
  CornerUpLeft,
  RefreshCw,
  Search,
  Send,
  X,
} from "lucide-react";
import { api } from "../api/http";
import type { MessageRecord } from "../api/types";
import { cn } from "@/lib/cn";

interface Props {
  /** Latest swarm `message` event observed by the parent (or null). */
  liveMessage: MessageRecord | null;
  /** Latest swarm `message_read` event observed by the parent (or null). */
  liveRead: { ids: number[]; to_agent: string; at: number } | null;
  /** Parent-maintained unread tally keyed by from_agent. */
  unreadByFrom: Record<string, number>;
}

const KIND_DEFAULT = "note";

function formatTime(ms: number): string {
  return new Date(ms).toLocaleTimeString();
}

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
  const rowRefs = useRef<Map<number, HTMLLIElement | null>>(new Map());
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
      if (res.marked.includes(m.id)) {
        setItems((prev) =>
          prev.map((x) => (x.id === m.id ? { ...x, read_at: res.at } : x)),
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
    window.setTimeout(
      () => setHighlightId((cur) => (cur === parentId ? null : cur)),
      1200,
    );
  };

  const senders = Object.entries(unreadByFrom).filter(([, n]) => n > 0);

  return (
    <div className="flex h-full flex-col bg-surface-primary">
      {/* Filter row */}
      <div className="flex shrink-0 items-center gap-2 border-b border-border-subtle px-4 py-2">
        <div className="flex h-8 min-w-0 flex-1 items-center gap-2 rounded-md bg-surface-tertiary px-3">
          <Search className="size-3.5 text-foreground-tertiary" />
          <input
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            placeholder="过滤 发送方 / 接收方 / 正文"
            className="min-w-0 flex-1 bg-transparent text-xs text-foreground-primary placeholder:text-foreground-tertiary focus:outline-none"
          />
        </div>
        <button
          onClick={refresh}
          className="flex size-8 items-center justify-center rounded-md text-foreground-tertiary hover:bg-surface-tertiary"
          title="刷新"
        >
          <RefreshCw className="size-3.5" />
        </button>
      </div>

      {/* By-sender toggle */}
      <div className="flex shrink-0 flex-col gap-1 border-b border-border-subtle px-4 py-2">
        <button
          onClick={() => setShowBySender((v) => !v)}
          className="flex items-center gap-1 self-start font-caption text-[11px] text-foreground-tertiary hover:text-foreground-secondary"
          title="按发送方查看未读"
        >
          {showBySender ? (
            <ChevronDown className="size-3" />
          ) : (
            <ChevronRight className="size-3" />
          )}
          按发送方未读 ({senders.length})
        </button>
        {showBySender && (
          <div className="flex flex-wrap gap-1.5 pt-1">
            {senders.length === 0 && (
              <span className="font-caption text-[11px] text-foreground-tertiary">
                无
              </span>
            )}
            {senders.map(([who, n]) => (
              <span
                key={who}
                className="inline-flex items-center gap-1.5 rounded-md bg-surface-tertiary px-2 py-0.5 text-[11px]"
              >
                <span className="font-mono text-foreground-secondary">{who}</span>
                <span className="rounded-full bg-state-danger px-1.5 text-[10px] font-semibold text-foreground-on-accent">
                  {n}
                </span>
              </span>
            ))}
          </div>
        )}
      </div>

      {error && (
        <div className="shrink-0 border-b border-state-danger/30 bg-status-danger-soft px-4 py-1.5 font-caption text-[11px] text-state-danger">
          {error}
        </div>
      )}

      {/* Messages list */}
      <div ref={listRef} className="min-h-0 flex-1 overflow-y-auto px-4 py-3">
        {visible.length === 0 && (
          <p className="mt-6 text-center font-caption text-xs text-foreground-tertiary">
            暂无消息
          </p>
        )}
        <ul className="flex flex-col gap-1.5">
          {visible.map((m) => {
            const unread = m.read_at === null;
            const highlighted = highlightId === m.id;
            return (
              <li
                key={m.id}
                ref={(el) => {
                  if (el) rowRefs.current.set(m.id, el);
                  else rowRefs.current.delete(m.id);
                }}
                className={cn(
                  "group rounded-md border px-3 py-2 transition-colors",
                  highlighted
                    ? "border-accent-primary bg-accent-primary-soft"
                    : unread
                      ? "border-state-busy/40 bg-surface-accent-tint"
                      : "border-border-subtle bg-surface-elevated",
                )}
              >
                <div className="flex flex-wrap items-center gap-1.5 font-caption text-[10px] text-foreground-tertiary">
                  <span
                    className={cn(
                      "font-bold",
                      unread ? "text-state-busy" : "text-state-success",
                    )}
                    title={unread ? "未读" : "已读"}
                  >
                    {unread ? "★" : "✓"}
                  </span>
                  <span className="font-mono">#{m.id}</span>
                  <span className="font-mono text-accent-primary">
                    {m.from_agent}
                  </span>
                  <span>→</span>
                  <span className="font-mono text-state-success">
                    {m.to_agent}
                  </span>
                  <span>·</span>
                  <span>{m.kind}</span>
                  <span>·</span>
                  <span>{formatTime(m.sent_at)}</span>
                  {m.in_reply_to != null && (
                    <button
                      onClick={() => jumpToParent(m.in_reply_to!)}
                      className="ml-1 flex items-center gap-0.5 rounded px-1 text-state-busy hover:bg-status-busy-soft"
                      title="跳转到被回复的消息"
                    >
                      <CornerUpLeft className="size-2.5" />#{m.in_reply_to}
                    </button>
                  )}
                  <span className="flex-1" />
                  <button
                    onClick={() => startReply(m)}
                    className="opacity-0 transition-opacity group-hover:opacity-100 rounded border border-border-subtle bg-surface-elevated px-1.5 text-foreground-secondary hover:bg-surface-tertiary"
                    title="回复这条消息"
                  >
                    回复
                  </button>
                  {unread && (
                    <button
                      onClick={() => markRead(m)}
                      disabled={marking === m.id}
                      className="rounded border border-border-subtle bg-surface-elevated px-1.5 text-foreground-secondary hover:bg-surface-tertiary disabled:opacity-50"
                      title="标记为已读"
                    >
                      ✓
                    </button>
                  )}
                </div>
                <p
                  className={cn(
                    "mt-1 whitespace-pre-wrap font-body text-xs leading-relaxed break-words text-foreground-primary",
                    !unread && "opacity-70",
                  )}
                >
                  {m.body}
                </p>
              </li>
            );
          })}
        </ul>
      </div>

      {/* Composer */}
      <div className="flex shrink-0 flex-col gap-2 border-t border-border-subtle bg-surface-secondary px-4 py-3">
        {inReplyTo != null && (
          <div className="flex items-center gap-2 self-start rounded-md bg-accent-primary-soft px-2 py-1 text-[11px] text-accent-primary-deep">
            <CornerUpLeft className="size-3" />
            正在回复 #{inReplyTo}
            <button
              onClick={() => setInReplyTo(null)}
              className="ml-1 rounded hover:bg-surface-elevated"
              title="取消回复"
            >
              <X className="size-3" />
            </button>
          </div>
        )}
        <div className="flex gap-2">
          <input
            value={from}
            onChange={(e) => setFrom(e.target.value)}
            placeholder="发送方（留空 = system）"
            className="min-w-0 flex-1 rounded-md border border-border-subtle bg-surface-elevated px-2.5 py-1.5 font-mono text-xs text-foreground-primary placeholder:text-foreground-tertiary focus:border-accent-primary focus:outline-none"
          />
          <input
            value={to}
            onChange={(e) => setTo(e.target.value)}
            placeholder="接收方（agent id）"
            className="min-w-0 flex-1 rounded-md border border-border-subtle bg-surface-elevated px-2.5 py-1.5 font-mono text-xs text-foreground-primary placeholder:text-foreground-tertiary focus:border-accent-primary focus:outline-none"
          />
        </div>
        <textarea
          value={body}
          onChange={(e) => setBody(e.target.value)}
          placeholder="消息正文"
          rows={3}
          className="resize-y rounded-md border border-border-subtle bg-surface-elevated px-2.5 py-1.5 font-body text-xs text-foreground-primary placeholder:text-foreground-tertiary focus:border-accent-primary focus:outline-none"
        />
        <div className="flex justify-end">
          <button
            onClick={send}
            disabled={sending || !to.trim() || !body.trim()}
            className="flex items-center gap-1.5 rounded-md bg-accent-primary px-4 py-1.5 text-xs font-semibold text-foreground-on-accent hover:bg-accent-primary-deep disabled:opacity-50"
          >
            <Send className="size-3.5" />
            {sending ? "发送中…" : "发送"}
          </button>
        </div>
      </div>
    </div>
  );
}
