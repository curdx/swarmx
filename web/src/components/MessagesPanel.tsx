/**
 * MessagesPanel — Telegram-style chat bubbles + minimal composer.
 *
 * Rewritten in UI/F.2 to match the chat-room metaphor introduced by /chat:
 *   - user-authored messages (from_agent === "user") right-aligned, accent fill
 *   - agent messages left-aligned with role-coloured avatar + name header
 *   - "system" messages render as a centered hairline note
 *   - consecutive messages from the same sender collapse the avatar/name row
 *   - a time-divider is inserted when the gap between adjacent messages > 5min
 *   - meta (#id, kind, full timestamp) hides behind hover via title-tooltip
 *   - composer collapses from from/to/body trio into a single auto-grow
 *     textarea; the recipient is picked from the active workspace members
 *
 * Functional contract preserved with the legacy panel:
 *   - sendMessage / markMessagesRead / listMessages API calls unchanged
 *   - `↩ #<id>` lineage rendered inside the bubble; click scrolls to parent
 *   - "Reply" / "Mark read" remain reachable via per-bubble hover actions
 *   - filter / refresh kept on the top bar; "by sender" demoted to a popover
 *   - UI does NOT auto-mark-read — opening the panel is not the same as a
 *     human having actually read a message.
 */

import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { useTranslation } from "react-i18next";
import {
  CornerUpLeft,
  Filter,
  RefreshCw,
  Search,
  Send,
  X,
} from "lucide-react";
import { api } from "../api/http";
import type { AgentInfo, MessageRecord } from "../api/types";
import { cn } from "@/lib/cn";

interface Props {
  /** Latest swarm `message` event observed by the parent (or null). */
  liveMessage: MessageRecord | null;
  /** Latest swarm `message_read` event observed by the parent (or null). */
  liveRead: { ids: number[]; to_agent: string; at: number } | null;
  /** Parent-maintained unread tally keyed by from_agent. */
  unreadByFrom: Record<string, number>;
  /** Agents alive in the currently-selected workspace; drives composer
   *  recipient and avatar role-color resolution. Optional so the legacy
   *  /debug SwarmPanel can keep rendering messages without a room. */
  activeMembers?: AgentInfo[];
  /** All agents alive across every workspace; drives the "X is responding"
   *  pending-bubble inference. Defaults to activeMembers when absent. */
  allAliveAgents?: AgentInfo[];
  /** Agent ids that historically belonged to the active workspace (live +
   *  killed). When provided, only messages whose from/to hits this set are
   *  rendered — so each workspace is a self-contained chat room. Omitting it
   *  keeps the legacy /debug behaviour of showing every message. */
  workspaceAgentIds?: string[];
  /** Human-readable room label shown inside the composer placeholder. */
  workspaceLabel?: string;
  /** Override for the composer's send action. When provided, the textarea is
   *  enabled even with no recipient and pressing Enter / 发送 calls this
   *  function instead of api.sendMessage. ChatRoute wires this for
   *  init-only workspaces so the user's first message triggers
   *  auto-dispatch (rather than getting swallowed by a STOPped scout).
   *  Note: override receives the trimmed body — it's responsible for any
   *  side effects (running spells, persisting a user message, etc.). */
  composerOverride?: (body: string) => Promise<void>;
  /** Click-handler when the user taps an avatar — typically opens AgentDrawer. */
  onOpenAgent?: (agentId: string) => void;
}

const KIND_DEFAULT = "note";
const USER_SENDER = "user";
const SYSTEM_SENDER = "system";
const GROUP_GAP_MS = 5 * 60_000; // 5 minutes — same heuristic as Telegram
/** Window during which an unanswered inbound message keeps the "typing"
 *  placeholder alive. Beyond this, the agent is probably stuck/done and
 *  the indicator is more misleading than helpful. */
const PENDING_TIMEOUT_MS = 60_000;

const ROLE_COLOR: Record<string, string> = {
  planner: "bg-agent-planner",
  backend: "bg-agent-backend",
  frontend: "bg-agent-frontend",
  architect: "bg-agent-architect",
  critic: "bg-agent-critic",
  test: "bg-agent-test",
};

function roleColor(role: string) {
  return ROLE_COLOR[role.toLowerCase()] ?? "bg-state-idle";
}

/** Resolve a role label for a from_agent id.
 *
 *  Looks up the lookup map first (populated from /api/agent — covers both
 *  active and exited agents). Falls back to a string heuristic so the very
 *  first paint, before listAgents() resolves, still shows *something*.
 */
function resolveRole(
  fromAgent: string,
  lookup: Map<string, string>,
): string {
  const hit = lookup.get(fromAgent);
  if (hit) return hit;
  // agent_ids historically follow either `<cli>-<hash>` or `_<role>_<hash>`.
  // Neither prefix is the role we want, but it's better than the full id.
  const seg = fromAgent.replace(/^_+/, "").split(/[-_]/)[0];
  return seg || "agent";
}

function formatClock(ms: number): string {
  return new Date(ms).toLocaleTimeString([], {
    hour: "2-digit",
    minute: "2-digit",
  });
}

function formatFullStamp(ms: number): string {
  return new Date(ms).toLocaleString();
}

function formatDivider(ms: number): string {
  const now = new Date();
  const d = new Date(ms);
  const sameDay =
    d.getFullYear() === now.getFullYear() &&
    d.getMonth() === now.getMonth() &&
    d.getDate() === now.getDate();
  return sameDay
    ? d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })
    : d.toLocaleString([], {
        month: "short",
        day: "numeric",
        hour: "2-digit",
        minute: "2-digit",
      });
}

interface Row {
  msg: MessageRecord;
  showHeader: boolean; // render avatar + name row?
  showDividerBefore: boolean;
}

function buildRows(items: MessageRecord[]): Row[] {
  const rows: Row[] = [];
  let prev: MessageRecord | null = null;
  for (const msg of items) {
    const gap = prev ? msg.sent_at - prev.sent_at : Infinity;
    const sameSender = prev?.from_agent === msg.from_agent;
    const showDividerBefore = prev !== null && gap > GROUP_GAP_MS;
    const showHeader = !sameSender || showDividerBefore;
    rows.push({ msg, showHeader, showDividerBefore });
    prev = msg;
  }
  return rows;
}

export function MessagesPanel({
  liveMessage,
  liveRead,
  unreadByFrom,
  activeMembers = [],
  allAliveAgents,
  workspaceAgentIds,
  workspaceLabel,
  composerOverride,
  onOpenAgent,
}: Props) {
  const aliveForInference = allAliveAgents ?? activeMembers;
  const { t } = useTranslation();
  const [items, setItems] = useState<MessageRecord[]>([]);
  const [filter, setFilter] = useState("");
  const [filterOpen, setFilterOpen] = useState(false);
  const [body, setBody] = useState("");
  const [inReplyTo, setInReplyTo] = useState<number | null>(null);
  const [sending, setSending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [marking, setMarking] = useState<number | null>(null);
  const [bySenderOpen, setBySenderOpen] = useState(false);

  const listRef = useRef<HTMLDivElement>(null);
  const rowRefs = useRef<Map<number, HTMLDivElement | null>>(new Map());
  const composerRef = useRef<HTMLTextAreaElement>(null);
  const [highlightId, setHighlightId] = useState<number | null>(null);

  // agent_id → role lookup covering exited agents too; needed so historical
  // messages render with the right avatar colour even after agents die.
  const [roleLookup, setRoleLookup] = useState<Map<string, string>>(
    () => new Map(),
  );
  useEffect(() => {
    api
      .listAgents()
      .then((all) => {
        setRoleLookup((prev) => {
          const next = new Map(prev);
          for (const a of all) next.set(a.agent_id, a.role);
          return next;
        });
      })
      .catch(() => {
        /* best-effort; resolveRole falls back to id-prefix heuristic */
      });
  }, []);
  useEffect(() => {
    setRoleLookup((prev) => {
      const next = new Map(prev);
      for (const a of activeMembers) next.set(a.agent_id, a.role);
      return next;
    });
  }, [activeMembers]);

  // ── data loaders ──────────────────────────────────────────────────────
  const refresh = useCallback(async () => {
    try {
      const rows = await api.listMessages({ limit: 200 });
      // server orders DESC by id; reverse to chat-style chronological.
      setItems(rows.slice().reverse());
      setError(null);
    } catch (e) {
      setError((e as Error).message);
    }
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  useEffect(() => {
    if (!liveMessage) return;
    setItems((prev) =>
      prev.some((m) => m.id === liveMessage.id) ? prev : [...prev, liveMessage],
    );
  }, [liveMessage]);

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

  // ── filtering + grouping ──────────────────────────────────────────────
  // workspaceAgentIds 限定当前房间：message 命中 from 或 to 在集合内才显示。
  // user/system 不在 agent 集合里，但与他们配对的另一头一定是 agent_id，所以
  // 单条规则就够，不用为 user/system 开特例。
  const wsSet = useMemo(
    () => (workspaceAgentIds ? new Set(workspaceAgentIds) : null),
    [workspaceAgentIds],
  );
  const visible = useMemo(() => {
    const f = filter.toLowerCase();
    return items.filter((m) => {
      if (wsSet && !(wsSet.has(m.from_agent) || wsSet.has(m.to_agent))) {
        return false;
      }
      if (!f) return true;
      return (
        m.from_agent.toLowerCase().includes(f) ||
        m.to_agent.toLowerCase().includes(f) ||
        m.body.toLowerCase().includes(f)
      );
    });
  }, [items, filter, wsSet]);
  const rows = useMemo(() => buildRows(visible), [visible]);

  // ── pending responder inference (UI/F.2-A) ────────────────────────────
  // Tick every 5s so PENDING_TIMEOUT_MS naturally retires stale indicators
  // even when no new events arrive on /ws/swarm.
  const [tick, setTick] = useState(0);
  useEffect(() => {
    const i = window.setInterval(() => setTick((t) => t + 1), 5000);
    return () => window.clearInterval(i);
  }, []);
  const pendingResponders = useMemo(() => {
    const aliveIds = new Set(
      aliveForInference
        .filter((m) => m.shim_exit == null && m.killed_at == null)
        .map((m) => m.agent_id),
    );
    if (aliveIds.size === 0) return [];
    const now = Date.now();
    const lastSent = new Map<string, number>();
    const lastReceived = new Map<string, MessageRecord>();
    for (const m of items) {
      if (aliveIds.has(m.from_agent)) {
        const prev = lastSent.get(m.from_agent) ?? 0;
        if (m.sent_at > prev) lastSent.set(m.from_agent, m.sent_at);
      }
      if (aliveIds.has(m.to_agent)) {
        const prev = lastReceived.get(m.to_agent);
        if (!prev || m.sent_at > prev.sent_at) {
          lastReceived.set(m.to_agent, m);
        }
      }
    }
    const out: Array<{ agentId: string; trigger: MessageRecord }> = [];
    for (const [agentId, trigger] of lastReceived) {
      const sentAt = lastSent.get(agentId) ?? 0;
      if (trigger.sent_at <= sentAt) continue;
      if (now - trigger.sent_at > PENDING_TIMEOUT_MS) continue;
      out.push({ agentId, trigger });
    }
    // Stable order: earliest-triggered first, so older waiters appear above.
    out.sort((a, b) => a.trigger.sent_at - b.trigger.sent_at);
    return out;
    // tick is purposeful — re-evaluates the "now - sent_at" cutoff over time.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [items, aliveForInference, tick]);

  // Auto-scroll to bottom on new items / live message / new pending bubble.
  useLayoutEffect(() => {
    const el = listRef.current;
    if (!el) return;
    el.scrollTop = el.scrollHeight;
  }, [rows.length, pendingResponders.length]);

  // ── send / reply / mark-read ──────────────────────────────────────────
  const defaultRecipient = useMemo(() => {
    if (activeMembers.length === 0) return null;
    // 1-on-1 → that agent. Multi → first agent for now; future iteration
    // will surface a member-picker chip in the composer.
    return activeMembers[0];
  }, [activeMembers]);

  const send = async () => {
    const trimmed = body.trim();
    if (!trimmed) return;
    if (!composerOverride && !defaultRecipient) return;
    setSending(true);
    try {
      if (composerOverride) {
        // init-only workspace 路径：override 通常会启动 auto-dispatch spell。
        // 先 sendMessage(to="system") 把 user 消息落库，本地立即 echo 出来，
        // 让用户看到自己发出去了；然后 await override 启动 planner。
        const rec = await api.sendMessage({
          from: USER_SENDER,
          to: SYSTEM_SENDER,
          kind: KIND_DEFAULT,
          body: trimmed,
          in_reply_to: inReplyTo ?? undefined,
        });
        setItems((prev) =>
          prev.some((m) => m.id === rec.id) ? prev : [...prev, rec],
        );
        await composerOverride(trimmed);
      } else {
        const rec = await api.sendMessage({
          from: USER_SENDER,
          to: defaultRecipient!.agent_id,
          kind: KIND_DEFAULT,
          body: trimmed,
          in_reply_to: inReplyTo ?? undefined,
        });
        setItems((prev) =>
          prev.some((m) => m.id === rec.id) ? prev : [...prev, rec],
        );
      }
      setBody("");
      setInReplyTo(null);
      setError(null);
      composerRef.current?.focus();
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setSending(false);
    }
  };

  const startReply = (m: MessageRecord) => {
    setInReplyTo(m.id);
    composerRef.current?.focus();
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

  // Auto-grow composer (max ~5 lines).
  const autoGrow = (el: HTMLTextAreaElement | null) => {
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, 140)}px`;
  };

  const onComposerKey = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    // Enter to send; Shift+Enter (or IME composing) inserts newline.
    if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
      e.preventDefault();
      send();
    }
  };

  const senders = Object.entries(unreadByFrom).filter(([, n]) => n > 0);
  const sendDisabled =
    sending || !body.trim() || (!composerOverride && !defaultRecipient);

  const composerPlaceholder = composerOverride
    ? t("messages.composerPlaceholderInit")
    : defaultRecipient
      ? t("messages.composerPlaceholder", {
          room: workspaceLabel ?? defaultRecipient.role,
        })
      : t("messages.composerPlaceholderEmpty");

  return (
    <div className="flex h-full flex-col bg-surface-primary">
      {/* ── slim top bar ─────────────────────────────────────────────── */}
      <div className="flex shrink-0 items-center gap-1 border-b border-border-subtle px-3 py-1.5">
        {filterOpen ? (
          <div className="flex h-7 min-w-0 flex-1 items-center gap-2 rounded-md bg-surface-tertiary px-2.5">
            <Search className="size-3.5 text-foreground-tertiary" />
            <input
              autoFocus
              value={filter}
              onChange={(e) => setFilter(e.target.value)}
              placeholder={t("messages.filter")}
              className="min-w-0 flex-1 bg-transparent text-xs text-foreground-primary placeholder:text-foreground-tertiary focus:outline-none"
            />
            <button
              onClick={() => {
                setFilter("");
                setFilterOpen(false);
              }}
              className="rounded p-0.5 text-foreground-tertiary hover:bg-surface-elevated"
              title={t("messages.cancelReply")}
            >
              <X className="size-3" />
            </button>
          </div>
        ) : (
          <>
            <span className="flex-1" />
            <button
              onClick={() => setFilterOpen(true)}
              className="flex size-7 items-center justify-center rounded-md text-foreground-tertiary hover:bg-surface-tertiary"
              title={t("messages.filter")}
            >
              <Search className="size-3.5" />
            </button>
          </>
        )}
        <div className="relative">
          <button
            onClick={() => setBySenderOpen((v) => !v)}
            className={cn(
              "flex size-7 items-center justify-center rounded-md hover:bg-surface-tertiary",
              senders.length > 0
                ? "text-state-danger"
                : "text-foreground-tertiary",
            )}
            title={t("messages.bySender", { count: senders.length })}
          >
            <Filter className="size-3.5" />
            {senders.length > 0 && (
              <span className="absolute right-1 top-1 size-1.5 rounded-full bg-state-danger" />
            )}
          </button>
          {bySenderOpen && (
            <div className="absolute right-0 top-9 z-10 w-56 rounded-md border border-border-subtle bg-surface-elevated p-2 shadow-lg">
              <p className="mb-1.5 font-caption text-[10px] uppercase tracking-wider text-foreground-tertiary">
                {t("messages.bySender", { count: senders.length })}
              </p>
              {senders.length === 0 ? (
                <p className="font-caption text-[11px] text-foreground-tertiary">
                  {t("messages.bySenderNone")}
                </p>
              ) : (
                <ul className="flex flex-col gap-1">
                  {senders.map(([who, n]) => (
                    <li
                      key={who}
                      className="flex items-center gap-2 rounded px-1.5 py-1 text-[11px] hover:bg-surface-tertiary"
                    >
                      <span className="min-w-0 flex-1 truncate font-mono text-foreground-secondary">
                        {who}
                      </span>
                      <span className="rounded-full bg-state-danger px-1.5 py-0.5 text-[10px] font-semibold text-foreground-on-accent">
                        {n}
                      </span>
                    </li>
                  ))}
                </ul>
              )}
            </div>
          )}
        </div>
        <button
          onClick={refresh}
          className="flex size-7 items-center justify-center rounded-md text-foreground-tertiary hover:bg-surface-tertiary"
          title={t("messages.refresh")}
        >
          <RefreshCw className="size-3.5" />
        </button>
      </div>

      {error && (
        <div className="shrink-0 border-b border-state-danger/30 bg-status-danger-soft px-4 py-1.5 font-caption text-[11px] text-state-danger">
          {error}
        </div>
      )}

      {/* ── bubble list ──────────────────────────────────────────────── */}
      <div ref={listRef} className="min-h-0 flex-1 overflow-y-auto px-4 py-3">
        {rows.length === 0 && (
          <p className="mt-10 text-center font-caption text-xs text-foreground-tertiary">
            {t("messages.empty")}
          </p>
        )}
        <div className="flex flex-col gap-0.5">
          {rows.map(({ msg: m, showHeader, showDividerBefore }) => {
            const isUser = m.from_agent === USER_SENDER;
            const isSystem = m.from_agent === SYSTEM_SENDER;
            const role = resolveRole(m.from_agent, roleLookup);
            const isUnread =
              !isUser && m.read_at === null && m.to_agent === USER_SENDER;
            const highlighted = highlightId === m.id;

            // System messages: centered hairline note, no bubble.
            if (isSystem) {
              return (
                <div
                  key={m.id}
                  ref={(el) => {
                    if (el) rowRefs.current.set(m.id, el);
                    else rowRefs.current.delete(m.id);
                  }}
                  className="my-1.5 flex flex-col items-center gap-0.5"
                >
                  {showDividerBefore && (
                    <TimeDivider ms={m.sent_at} />
                  )}
                  <span
                    className={cn(
                      "rounded-full bg-surface-tertiary px-3 py-0.5 font-caption text-[10px] text-foreground-tertiary",
                      highlighted && "ring-1 ring-accent-primary",
                    )}
                    title={`#${m.id} · ${m.kind} · ${formatFullStamp(m.sent_at)}`}
                  >
                    {m.body}
                  </span>
                </div>
              );
            }

            return (
              <div
                key={m.id}
                className={cn(
                  "flex flex-col",
                  isUser ? "items-end" : "items-start",
                  showHeader && rows[0].msg.id !== m.id && "mt-2",
                )}
              >
                {showDividerBefore && <TimeDivider ms={m.sent_at} />}

                <div
                  className={cn(
                    "flex max-w-[78%] gap-2",
                    isUser ? "flex-row-reverse" : "flex-row",
                  )}
                >
                  {/* avatar column — 28px reserved even when hidden so
                      collapsed messages stay aligned with their header */}
                  <div className="w-7 shrink-0">
                    {showHeader && !isUser && (
                      <button
                        type="button"
                        onClick={() => onOpenAgent?.(m.from_agent)}
                        className={cn(
                          "flex size-7 items-center justify-center rounded-full text-xs font-medium text-foreground-on-accent shadow-sm transition-transform hover:scale-105",
                          roleColor(role),
                        )}
                        title={`${role} · ${m.from_agent}`}
                      >
                        {role.charAt(0).toUpperCase()}
                      </button>
                    )}
                  </div>

                  <div
                    className={cn(
                      "flex min-w-0 flex-col",
                      isUser ? "items-end" : "items-start",
                    )}
                  >
                    {showHeader && (
                      <div
                        className={cn(
                          "mb-0.5 flex items-baseline gap-1.5 px-0.5",
                          isUser && "flex-row-reverse",
                        )}
                      >
                        <span className="font-heading text-[11px] font-semibold text-foreground-secondary">
                          {isUser ? t("messages.you") : role}
                        </span>
                      </div>
                    )}

                    <div
                      ref={(el) => {
                        if (el) rowRefs.current.set(m.id, el);
                        else rowRefs.current.delete(m.id);
                      }}
                      className={cn(
                        "group/bubble relative rounded-2xl px-3 py-1.5 transition-colors",
                        isUser
                          ? "bg-accent-primary text-foreground-on-accent rounded-br-sm"
                          : "bg-surface-elevated border border-border-subtle text-foreground-primary rounded-bl-sm",
                        isUnread && "border-l-2 border-l-state-busy",
                        highlighted &&
                          "ring-2 ring-accent-primary ring-offset-1 ring-offset-surface-primary",
                      )}
                      title={`#${m.id} · ${m.kind} · ${formatFullStamp(m.sent_at)}`}
                    >
                      {m.in_reply_to != null && (
                        <button
                          onClick={() => jumpToParent(m.in_reply_to!)}
                          className={cn(
                            "mb-1 flex items-center gap-0.5 rounded px-1 py-0.5 text-[10px]",
                            isUser
                              ? "bg-accent-primary-deep text-foreground-on-accent/80 hover:bg-accent-primary-deep/80"
                              : "bg-surface-tertiary text-foreground-tertiary hover:bg-surface-secondary",
                          )}
                          title={t("messages.jumpParent")}
                        >
                          <CornerUpLeft className="size-2.5" />#{m.in_reply_to}
                        </button>
                      )}

                      <p className="whitespace-pre-wrap break-words font-body text-[13px] leading-snug">
                        {m.body}
                      </p>

                      <span
                        className={cn(
                          "ml-2 mt-0.5 inline-block font-caption text-[10px] tabular-nums",
                          isUser
                            ? "text-foreground-on-accent/70 float-right"
                            : "text-foreground-tertiary float-right",
                        )}
                      >
                        {formatClock(m.sent_at)}
                      </span>

                      {/* hover-only actions — sit outside the bubble so they
                          don't shift content; positioned above for user
                          messages (right-aligned) and below for agents. */}
                      <div
                        className={cn(
                          "pointer-events-none absolute -top-3 flex items-center gap-1 opacity-0 transition-opacity group-hover/bubble:pointer-events-auto group-hover/bubble:opacity-100",
                          isUser ? "left-0" : "right-0",
                        )}
                      >
                        <button
                          onClick={() => startReply(m)}
                          className="rounded-full border border-border-subtle bg-surface-elevated px-2 py-0.5 text-[10px] text-foreground-secondary shadow-sm hover:bg-surface-tertiary"
                          title={t("messages.reply")}
                        >
                          {t("messages.reply")}
                        </button>
                        {isUnread && (
                          <button
                            onClick={() => markRead(m)}
                            disabled={marking === m.id}
                            className="rounded-full border border-border-subtle bg-surface-elevated px-2 py-0.5 text-[10px] text-foreground-secondary shadow-sm hover:bg-surface-tertiary disabled:opacity-50"
                            title={t("messages.markRead")}
                          >
                            ✓
                          </button>
                        )}
                      </div>
                    </div>
                  </div>
                </div>
              </div>
            );
          })}
          {pendingResponders.map(({ agentId, trigger }) => (
            <PendingBubble
              key={`pending-${agentId}`}
              role={resolveRole(agentId, roleLookup)}
              triggerId={trigger.id}
              label={t("messages.respondingTo", {
                role: resolveRole(agentId, roleLookup),
              })}
              replyHint={t("messages.responding", { id: trigger.id })}
            />
          ))}
        </div>
      </div>

      {/* ── composer ─────────────────────────────────────────────────── */}
      <div className="flex shrink-0 flex-col gap-1.5 border-t border-border-subtle bg-surface-secondary px-3 py-2.5">
        {inReplyTo != null && (
          <div className="flex items-center gap-2 self-start rounded-md bg-accent-primary-soft px-2 py-1 text-[11px] text-accent-primary-deep">
            <CornerUpLeft className="size-3" />
            {t("messages.replying", { id: inReplyTo })}
            <button
              onClick={() => setInReplyTo(null)}
              className="ml-1 rounded hover:bg-surface-elevated"
              title={t("messages.cancelReply")}
            >
              <X className="size-3" />
            </button>
          </div>
        )}
        <div className="flex items-end gap-2">
          {defaultRecipient && (
            <span
              className="inline-flex items-center gap-1 self-start rounded-full bg-surface-tertiary px-2 py-1 font-caption text-[10px] text-foreground-secondary"
              title={defaultRecipient.agent_id}
            >
              <span
                className={cn(
                  "size-3 shrink-0 rounded-full",
                  roleColor(defaultRecipient.role),
                )}
              />
              <span>
                {t("messages.to")} {defaultRecipient.role}
              </span>
            </span>
          )}
          <textarea
            ref={composerRef}
            value={body}
            onChange={(e) => {
              setBody(e.target.value);
              autoGrow(e.target);
            }}
            onKeyDown={onComposerKey}
            placeholder={composerPlaceholder}
            disabled={!composerOverride && !defaultRecipient}
            rows={1}
            className="min-w-0 flex-1 resize-none rounded-2xl border border-border-subtle bg-surface-elevated px-3 py-2 font-body text-[13px] leading-snug text-foreground-primary placeholder:text-foreground-tertiary focus:border-accent-primary focus:outline-none disabled:opacity-60"
          />
          <button
            onClick={send}
            disabled={sendDisabled}
            className="flex size-9 shrink-0 items-center justify-center rounded-full bg-accent-primary text-foreground-on-accent shadow-sm transition-colors hover:bg-accent-primary-deep disabled:opacity-50"
            title={sending ? t("messages.sending") : t("messages.send")}
          >
            <Send className="size-4" />
          </button>
        </div>
        <span className="self-end font-caption text-[10px] text-foreground-tertiary">
          {t("messages.sendHint")}
        </span>
      </div>
    </div>
  );
}

function TimeDivider({ ms }: { ms: number }) {
  return (
    <div className="my-2 flex w-full items-center gap-2">
      <span className="h-px flex-1 bg-border-subtle" />
      <span className="font-caption text-[10px] tabular-nums text-foreground-tertiary">
        {formatDivider(ms)}
      </span>
      <span className="h-px flex-1 bg-border-subtle" />
    </div>
  );
}

/** "X is responding…" placeholder bubble shown while an agent has received a
 *  message but hasn't yet emitted a reply. Pure UI inference for now —
 *  upgraded to real server-side AgentState::Thinking events in UI/F.2-B. */
function PendingBubble({
  role,
  triggerId,
  label,
  replyHint,
}: {
  role: string;
  triggerId: number;
  label: string;
  replyHint: string;
}) {
  return (
    <div className="mt-2 flex flex-col items-start">
      <div className="flex max-w-[78%] gap-2">
        <div className="w-7 shrink-0">
          <div
            className={cn(
              "flex size-7 items-center justify-center rounded-full text-xs font-medium text-foreground-on-accent shadow-sm",
              roleColor(role),
            )}
            title={`${role} · ${replyHint}`}
          >
            {role.charAt(0).toUpperCase()}
          </div>
        </div>
        <div className="flex min-w-0 flex-col items-start">
          <span className="mb-0.5 flex items-center gap-1.5 px-0.5 font-heading text-[11px] font-semibold text-foreground-secondary">
            {role}
            <span className="font-caption text-[10px] font-normal text-foreground-tertiary">
              {label} · ↩#{triggerId}
            </span>
          </span>
          <div
            className="rounded-2xl rounded-bl-sm border border-border-subtle bg-surface-elevated px-3 py-2"
            title={replyHint}
          >
            <span className="flex items-center gap-1">
              <PendingDot delayMs={0} />
              <PendingDot delayMs={150} />
              <PendingDot delayMs={300} />
            </span>
          </div>
        </div>
      </div>
    </div>
  );
}

function PendingDot({ delayMs }: { delayMs: number }) {
  return (
    <span
      className="block size-1.5 rounded-full bg-foreground-tertiary"
      style={{
        animation: "flockmuxTypingDot 1s ease-in-out infinite",
        animationDelay: `${delayMs}ms`,
      }}
    />
  );
}
