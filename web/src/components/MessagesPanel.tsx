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
  Loader2,
  RefreshCw,
  Search,
  Send,
  Sparkles,
  Undo2,
  X,
} from "lucide-react";
import { api } from "../api/http";
import type { AgentInfo, MessageRecord } from "../api/types";
import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/textarea";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import { cn } from "@/lib/cn";
import { AgentChip } from "@/components/agent/AgentChip";
import { ChatMarkdown } from "@/components/ChatMarkdown";
import { ImageAttachments } from "@/components/ImageAttachments";
import { extractImagePaths, fileUrl, baseName } from "@/lib/imagePaths";
import { roleColorClass as roleColor } from "@/lib/agent";

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
  /** 当前 workspace 的 slug(= workspaces.id 前 8 char)。用于匹配
   *  `to_agent === "system:<slug>"` 的用户消息,让它们只在所属 workspace
   *  显示,不串到别的房间。omit 时退化到老行为(user msg 总显示)。 */
  workspaceSlug?: string;
  /** Active direction (thread) id. When set, a message tagged with a DIFFERENT
   *  direction is hard-hidden — defense-in-depth over the agent-set scope so a
   *  cross-direction leak can't surface. `null`/undefined disables the gate
   *  (legacy / no-thread workspaces). Null-tagged messages are never hidden. */
  activeThreadId?: string | null;
  /** Override for the composer's send action. When provided, the textarea is
   *  enabled even with no recipient and pressing Enter / 发送 calls this
   *  function instead of api.sendMessage. ChatRoute wires this for
   *  init-only workspaces so the user's first message triggers
   *  auto-dispatch (rather than getting swallowed by a STOPped scout).
   *  Note: override receives the trimmed body — it's responsible for any
   *  side effects (running spells, persisting a user message, etc.). */
  onSend?: (body: string) => Promise<void> | void;
  /** Click-handler when the user taps an avatar — typically opens AgentDrawer. */
  onOpenAgent?: (agentId: string) => void;
  /** Parent bumps this counter when the user clicks the "N 未读" badge in
   *  the chat header; we react by scrolling the first unread bubble into
   *  view and flashing it. Initial 0 is the no-op state. */
  jumpUnreadTick?: number;
  /** 渲染在消息列表底部、composer 上方的浮层 — chat 上下文的"AI 正在
   *  干活" inline cards 走这。父组件维护 task state machine，这里只是
   *  视觉插槽。 */
  taskActivityBelow?: React.ReactNode;
}

const KIND_DEFAULT = "note";
const USER_SENDER = "user";
const SYSTEM_SENDER = "system";
const GROUP_GAP_MS = 5 * 60_000; // 5 minutes — same heuristic as Telegram
/** Window during which an unanswered inbound message keeps the "typing"
 *  placeholder alive. Beyond this, the agent is probably stuck/done and
 *  the indicator is more misleading than helpful. */
const PENDING_TIMEOUT_MS = 60_000;

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
  workspaceSlug,
  activeThreadId,
  onOpenAgent,
  onSend,
  jumpUnreadTick = 0,
  taskActivityBelow,
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
  // 「优化」 button: the rewrite is reversible (preOptimize holds the pre-rewrite
  // draft for one-click undo); optimizeNote shows a transient "already clear".
  const [optimizing, setOptimizing] = useState(false);
  const [preOptimize, setPreOptimize] = useState<string | null>(null);
  const [optimizeNote, setOptimizeNote] = useState<string | null>(null);
  // Pasted/dropped clipboard images upload to /api/attachment; their saved path
  // is appended to the draft (agents read images by path).
  const [uploadingImage, setUploadingImage] = useState(false);
  const [marking, setMarking] = useState<number | null>(null);
  const [bySenderOpen, setBySenderOpen] = useState(false);

  const listRef = useRef<HTMLDivElement>(null);
  const rowRefs = useRef<Map<number, HTMLDivElement | null>>(new Map());
  const composerRef = useRef<HTMLTextAreaElement>(null);
  const [highlightId, setHighlightId] = useState<number | null>(null);

  // F5 auto-mark-read: ids whose bubble has scrolled into view (foregrounded)
  // and are pending a batched mark-read POST, plus the debounce timer.
  const pendingReadRef = useRef<Set<number>>(new Set());
  const flushTimerRef = useRef<number | null>(null);

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

  // ── F5: auto-mark-read on actual view ─────────────────────────────────
  // The panel deliberately does NOT treat "opened" as "read". But a bubble
  // that has scrolled into the viewport while the tab is foregrounded HAS
  // plausibly been seen by a human, so we mark it read then — clearing the
  // unread badge as the user browses instead of forcing a manual click. The
  // parent's per-sender tally decrements via the `message_read` WS broadcast
  // that POST /api/message/read emits, so we only touch local `items` here.
  const flushAutoRead = useCallback(() => {
    flushTimerRef.current = null;
    const ids = [...pendingReadRef.current];
    pendingReadRef.current.clear();
    if (ids.length === 0) return;
    // All collected ids are to_agent === "user" (see the observer filter).
    api
      .markMessagesRead(USER_SENDER, ids)
      .then((res) => {
        if (res.marked.length === 0) return;
        const marked = new Set(res.marked);
        setItems((prev) =>
          prev.map((m) => (marked.has(m.id) ? { ...m, read_at: res.at } : m)),
        );
      })
      .catch(() => {
        /* best-effort — the bubble stays observed and retries next intersect */
      });
  }, []);

  useEffect(() => {
    const root = listRef.current;
    if (!root || typeof IntersectionObserver === "undefined") return;
    const elToId = new Map<Element, number>();
    const io = new IntersectionObserver(
      (entries) => {
        // Foreground-only: a backgrounded tab scrolling (e.g. via anchor)
        // isn't a human reading. Honors the original "opened ≠ read" caveat.
        if (document.visibilityState !== "visible") return;
        let added = false;
        for (const e of entries) {
          if (!e.isIntersecting) continue;
          const id = elToId.get(e.target);
          if (id == null) continue;
          pendingReadRef.current.add(id);
          added = true;
        }
        if (added && flushTimerRef.current == null) {
          flushTimerRef.current = window.setTimeout(flushAutoRead, 400);
        }
      },
      { root, threshold: 0 },
    );
    for (const m of items) {
      if (m.to_agent !== USER_SENDER || m.read_at !== null) continue;
      const el = rowRefs.current.get(m.id);
      if (el) {
        elToId.set(el, m.id);
        io.observe(el);
      }
    }
    return () => io.disconnect();
  }, [items, flushAutoRead]);

  // Cancel any pending flush on unmount.
  useEffect(
    () => () => {
      if (flushTimerRef.current != null) window.clearTimeout(flushTimerRef.current);
    },
    [],
  );

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
      // 内部协调噪音 — 这些消息是给 LLM 看的 prompt,英文,普通用户看到
      // 一堆 "manual wake from operator —" / "blackboard X updated; please
      // check" 很乱。activity panel ≠ chat thread (业界 2026 共识):chat
      // 留给真对话,内部信令归后台 / Ledger 视图 / sqlite。
      //   - kind=wake          — server 的 manual wake 注入
      //   - from=system + body 提 blackboard updated  — swarm watcher 推的黑板通知
      // farewell (worker 临别) 不算噪音,保留;普通 note/reply 全保留。
      if (m.kind === "wake") return false;
      if (
        m.from_agent === "system" &&
        m.body.startsWith("blackboard ")
      ) {
        return false;
      }
      // 用户消息(from=user)走 to=scout(concierge),命中 wsSet。
      // user→system:<slug> 路径已经废除,但保留 filter 以兼容历史 DB 行:
      // 老消息可能还残留这种,slug 不匹配当前 ws 直接隐藏避免串房间。
      if (m.from_agent === USER_SENDER && m.to_agent.startsWith("system:")) {
        const targetSlug = m.to_agent.slice("system:".length);
        if (workspaceSlug && targetSlug !== workspaceSlug) {
          return false;
        }
      } else if (
        wsSet &&
        !(wsSet.has(m.from_agent) || wsSet.has(m.to_agent))
      ) {
        return false;
      }
      // Hard thread gate (defense-in-depth over the agent-set scope above):
      // a message tagged with a DIFFERENT direction never shows here. Untagged
      // (null — legacy rows, or a main-folded agent) is never hidden, so old
      // chat history and the main direction keep rendering. Mirrors
      // agentInThread(): main allows null|main-id, a direction allows only its
      // own id (cross-direction null is already dropped by wsSet above).
      if (
        activeThreadId != null &&
        m.thread_id != null &&
        m.thread_id !== activeThreadId
      ) {
        return false;
      }
      if (!f) return true;
      return (
        m.from_agent.toLowerCase().includes(f) ||
        m.to_agent.toLowerCase().includes(f) ||
        m.body.toLowerCase().includes(f)
      );
    });
  }, [items, filter, wsSet, workspaceSlug, activeThreadId]);
  const rows = useMemo(() => buildRows(visible), [visible]);

  // First agent→user unread message + total count. Drives the Slack-style
  // "N 条新消息" divider that replaces the old per-message amber left
  // border (which read as a cheap hack and made a many-unread thread noisy).
  // The divider is inserted once, before the first unread; everything below
  // it is implicitly new, so individual messages don't need their own mark.
  const { firstUnreadId, unreadCount } = useMemo(() => {
    let firstId: number | null = null;
    let count = 0;
    for (const m of visible) {
      const isUnreadAgent =
        m.from_agent !== USER_SENDER &&
        m.from_agent !== SYSTEM_SENDER &&
        m.read_at === null &&
        m.to_agent === USER_SENDER;
      if (isUnreadAgent) {
        if (firstId === null) firstId = m.id;
        count += 1;
      }
    }
    return { firstUnreadId: firstId, unreadCount: count };
  }, [visible]);

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
  // Magentic-One 重构后用户视角永远只跟"一个 AI 接待员对话":这个角色就是
  // orchestrator。收件人解析顺序 orchestrator > 第一个 alive(为兼容老
  // workspace 里的 scout 等历史 role 兜底)。
  const defaultRecipient = useMemo(() => {
    if (activeMembers.length === 0) return null;
    return (
      activeMembers.find((m) => m.role === "orchestrator") ??
      activeMembers.find((m) => m.role === "scout") ??
      activeMembers[0]
    );
  }, [activeMembers]);

  const send = async () => {
    const trimmed = body.trim();
    if (!trimmed) return;
    // No live recipient (workspace's orchestrator has exited). If the parent
    // wired `onSend`, route the message through it — it spawns the orchestrator
    // and delivers — so the user just types instead of first clicking 唤醒.
    if (!defaultRecipient) {
      if (!onSend) return;
      setSending(true);
      try {
        await onSend(trimmed);
        setBody("");
        setInReplyTo(null);
        setError(null);
        setPreOptimize(null);
        setOptimizeNote(null);
        composerRef.current?.focus();
      } catch (e) {
        setError((e as Error).message);
      } finally {
        setSending(false);
      }
      return;
    }
    setSending(true);
    try {
      const rec = await api.sendMessage({
        from: USER_SENDER,
        to: defaultRecipient.agent_id,
        kind: KIND_DEFAULT,
        body: trimmed,
        in_reply_to: inReplyTo ?? undefined,
      });
      setItems((prev) =>
        prev.some((m) => m.id === rec.id) ? prev : [...prev, rec],
      );
      // 主动 wake scout —— flockmux 现状是 sendMessage 只 push mailbox 不
      // wake recipient,而 scout 已经 STOP 在那等。fire 一发 manual wake
      // 让它去 swarm_list_messages 处理这条新消息。best-effort,失败也
      // 不阻塞 UI(下次 BlackboardChanged / Stop hook 自然会消化)。
      api.wakeAgent(defaultRecipient.agent_id).catch(() => {
        /* swallow */
      });
      setBody("");
      setInReplyTo(null);
      setError(null);
      setPreOptimize(null);
      setOptimizeNote(null);
      composerRef.current?.focus();
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setSending(false);
    }
  };

  // 「优化」 — server rewrites the draft (claude -p, fast tier) into a clearer
  // instruction. Replace-in-place WITH undo (the researched gold standard: a
  // proposal you can edit, never a silent overwrite). preOptimize keeps the
  // original so one click / typing restores it.
  const optimize = async () => {
    const trimmed = body.trim();
    if (!trimmed || optimizing || sending) return;
    setOptimizing(true);
    setError(null);
    setOptimizeNote(null);
    try {
      const res = await api.optimizePrompt(trimmed);
      if (res.changed && res.optimized && res.optimized !== body) {
        setPreOptimize(body);
        setBody(res.optimized);
        requestAnimationFrame(() => autoGrow(composerRef.current));
      } else {
        // Already clear — tell the user nothing changed (don't fake an edit).
        setOptimizeNote(t("messages.optimizeNoChange"));
        window.setTimeout(() => setOptimizeNote(null), 2600);
      }
      composerRef.current?.focus();
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setOptimizing(false);
    }
  };

  const undoOptimize = () => {
    if (preOptimize == null) return;
    setBody(preOptimize);
    setPreOptimize(null);
    setOptimizeNote(null);
    requestAnimationFrame(() => autoGrow(composerRef.current));
    composerRef.current?.focus();
  };

  const appendPath = (path: string) => {
    setBody((prev) => {
      if (!prev) return path;
      return prev + (/\s$/.test(prev) ? "" : "\n") + path;
    });
    requestAnimationFrame(() => autoGrow(composerRef.current));
  };

  // Paste/drop a clipboard image → upload → append its saved path to the draft.
  // (Pasting a path string is just normal text; this handles raw bitmaps.)
  const handleImageFiles = async (files: File[]) => {
    const imgs = files.filter((f) => f.type.startsWith("image/"));
    if (imgs.length === 0) return false;
    setUploadingImage(true);
    setError(null);
    try {
      for (const f of imgs) {
        const guessExt = (f.type.split("/")[1] || "png").replace("jpeg", "jpg");
        const { path } = await api.uploadAttachment(f, f.name || `pasted.${guessExt}`);
        appendPath(path);
      }
      composerRef.current?.focus();
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setUploadingImage(false);
    }
    return true;
  };

  const onComposerPaste = (e: React.ClipboardEvent<HTMLTextAreaElement>) => {
    const items = e.clipboardData?.items;
    if (!items) return;
    const files: File[] = [];
    for (const it of items) {
      if (it.kind === "file" && it.type.startsWith("image/")) {
        const f = it.getAsFile();
        if (f) files.push(f);
      }
    }
    if (files.length === 0) return; // plain text paste — let it through
    e.preventDefault();
    void handleImageFiles(files);
  };

  const onComposerDrop = (e: React.DragEvent<HTMLTextAreaElement>) => {
    const files = Array.from(e.dataTransfer?.files ?? []);
    if (files.some((f) => f.type.startsWith("image/"))) {
      e.preventDefault();
      void handleImageFiles(files);
    }
  };

  // Remove an image path token from the draft (the ✕ on a composer thumbnail).
  const removeComposerImage = (path: string) => {
    setBody((prev) =>
      prev
        .split(path)
        .join("")
        .replace(/`+/g, (m) => (m.length >= 2 ? "" : m))
        .replace(/\n{3,}/g, "\n\n")
        .replace(/[ \t]+\n/g, "\n")
        .trim(),
    );
    requestAnimationFrame(() => autoGrow(composerRef.current));
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

  // 用户点顶栏 "N 未读" badge → bump jumpUnreadTick → 滚到第一条未读
  // 并闪一下高亮。初始 0 不触发 (依赖数组改变才进 effect)。
  useEffect(() => {
    if (!jumpUnreadTick) return;
    const firstUnread = items.find(
      (m) => m.read_at === null && m.to_agent === USER_SENDER,
    );
    if (!firstUnread) return;
    const el = rowRefs.current.get(firstUnread.id);
    if (!el) return;
    el.scrollIntoView({ behavior: "smooth", block: "center" });
    setHighlightId(firstUnread.id);
    window.setTimeout(
      () => setHighlightId((cur) => (cur === firstUnread.id ? null : cur)),
      1200,
    );
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [jumpUnreadTick]);

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
  // Total unread MESSAGES (not sender count) — must match the toolbar's "N 未读"
  // badge so the two numbers never appear to contradict (F5: 4 messages from 3
  // senders previously showed as "4 未读" vs "(3)"). Sender count is secondary.
  const unreadTotal = senders.reduce((sum, [, n]) => sum + n, 0);
  // Composer is usable when there's a live recipient OR the parent wired
  // `onSend` (which spawns one on send) — so an exited orchestrator no longer
  // dead-ends the input behind a manual 唤醒 click.
  const canCompose = !!defaultRecipient || !!onSend;
  const sendDisabled = sending || !body.trim() || !canCompose;
  // Image paths currently in the draft → small removable thumbnails above the
  // input, so the user sees the screenshot they referenced/pasted.
  const composerImages = useMemo(() => extractImagePaths(body), [body]);
  // 所有 workspace 用同一句 placeholder —— 用户视角永远是"跟 AI 说话",
  // 不再区分 init-only / 普通 ws。无活成员但能 onSend 唤醒时，提示发消息即上线。
  const composerPlaceholder = defaultRecipient
    ? t("messages.composerPlaceholder")
    : onSend
      ? t("messages.composerPlaceholderRevive")
      : t("messages.composerPlaceholderEmpty");

  return (
    <div className="flex h-full flex-col bg-surface-primary">
      {/* ── slim top bar ─────────────────────────────────────────────── */}
      {/* No own border-b: the WorkspaceToolbar's divider directly above is
          the canonical header separator. A second hairline 36px below it
          just bracketed a near-empty action strip into a redundant band.
          Dropping it lets these chat-thread actions (search / by-sender /
          refresh) read as one quiet toolbar under the tabs. */}
      <div className="flex shrink-0 items-center gap-1 px-3 py-1">
        {filterOpen ? (
          <div className="flex h-7 min-w-0 flex-1 items-center gap-2 rounded-md bg-surface-tertiary px-2.5">
            <Search className="size-3.5 text-foreground-tertiary" />
            <input
              autoFocus
              name="message-filter"
              value={filter}
              onChange={(e) => setFilter(e.target.value)}
              placeholder={t("messages.filter")}
              className="min-w-0 flex-1 bg-transparent text-xs text-foreground-primary placeholder:text-foreground-tertiary focus:outline-none"
            />
            <Button
              variant="ghost"
              size="icon"
              onClick={() => {
                setFilter("");
                setFilterOpen(false);
              }}
              title={t("messages.clearFilter")}
              className="size-6 text-foreground-tertiary"
            >
              <X className="size-3" />
            </Button>
          </div>
        ) : (
          <>
            <span className="flex-1" />
            <Button
              variant="ghost"
              size="icon"
              onClick={() => setFilterOpen(true)}
              title={t("messages.filter")}
              className="size-7 text-foreground-tertiary"
            >
              <Search className="size-3.5" />
            </Button>
          </>
        )}
        <Popover open={bySenderOpen} onOpenChange={setBySenderOpen}>
          <PopoverTrigger asChild>
            <button
              className={cn(
                "relative flex size-7 items-center justify-center rounded-md hover:bg-surface-tertiary",
                senders.length > 0
                  ? "text-state-danger"
                  : "text-foreground-tertiary",
              )}
              title={t("messages.bySender", { total: unreadTotal, senders: senders.length })}
            >
              <Filter className="size-3.5" />
              {senders.length > 0 && (
                <span className="absolute right-1 top-1 size-1.5 rounded-full bg-state-danger" />
              )}
            </button>
          </PopoverTrigger>
          <PopoverContent
            align="end"
            sideOffset={6}
            className="w-60 p-2"
          >
            <p className="mb-1.5 font-caption text-[10px] uppercase tracking-wider text-foreground-tertiary">
              {t("messages.bySender", { total: unreadTotal, senders: senders.length })}
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
                    <AgentChip
                      agentId={who}
                      roleLookup={roleLookup}
                      size="xs"
                      className="min-w-0 flex-1"
                    />
                    <span className="rounded-full bg-state-danger px-1.5 py-0.5 text-[10px] font-semibold text-foreground-on-accent">
                      {n}
                    </span>
                  </li>
                ))}
              </ul>
            )}
          </PopoverContent>
        </Popover>
        <Button
          variant="ghost"
          size="icon"
          onClick={refresh}
          title={t("messages.refresh")}
          className="size-7 text-foreground-tertiary"
        >
          <RefreshCw className="size-3.5" />
        </Button>
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
              !isUser &&
              !isSystem &&
              m.read_at === null &&
              m.to_agent === USER_SENDER;
            const highlighted = highlightId === m.id;
            const isFirstRow = rows[0].msg.id === m.id;
            // Slack-style "new messages" divider, rendered once before the
            // first unread agent turn instead of marking each one.
            const newDivider =
              m.id === firstUnreadId && unreadCount > 0 ? (
                <NewMessagesDivider
                  label={t("messages.newMessages", { count: unreadCount })}
                />
              ) : null;

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
                  {showDividerBefore && <TimeDivider ms={m.sent_at} />}
                  {newDivider}
                  <span
                    className={cn(
                      "selectable rounded-full bg-surface-tertiary px-3 py-0.5 font-caption text-[10px] text-foreground-tertiary",
                      highlighted && "ring-1 ring-accent-primary",
                    )}
                    title={`#${m.id} · ${m.kind} · ${formatFullStamp(m.sent_at)}`}
                  >
                    {m.body}
                  </span>
                </div>
              );
            }

            // ── User turn: right-aligned, solid accent bubble ────────────
            // Accent-blue + white text (iMessage/Linear convention). The
            // earlier neutral-grey experiment failed: two pale grey bubbles on
            // a white canvas read as muddy and low-contrast. A confident accent
            // fill makes "mine vs theirs" unmistakable at a glance and is the
            // higher-contrast, more polished choice — colour is reinforced by
            // alignment + "我" label + tail shape, never the only signal.
            if (isUser) {
              return (
                <div
                  key={m.id}
                  className={cn(
                    "flex flex-col items-end",
                    !isFirstRow && (showHeader ? "mt-3" : "mt-2"),
                  )}
                >
                  {showDividerBefore && <TimeDivider ms={m.sent_at} />}
                  {newDivider}
                  <div className="flex max-w-[80%] flex-col items-end">
                    {showHeader && (
                      <span className="mb-0.5 px-1 font-heading text-[11px] font-semibold text-foreground-tertiary">
                        {t("messages.you")}
                      </span>
                    )}
                    <div
                      ref={(el) => {
                        if (el) rowRefs.current.set(m.id, el);
                        else rowRefs.current.delete(m.id);
                      }}
                      className={cn(
                        "group/bubble relative rounded-2xl rounded-br-sm bg-accent-primary px-3 py-1.5 text-foreground-on-accent shadow-sm transition-colors",
                        highlighted &&
                          "ring-2 ring-accent-primary ring-offset-1 ring-offset-surface-primary",
                      )}
                      title={`#${m.id} · ${m.kind} · ${formatFullStamp(m.sent_at)}`}
                    >
                      {m.in_reply_to != null && (
                        <button
                          onClick={() => jumpToParent(m.in_reply_to!)}
                          className="mb-1 flex items-center gap-0.5 rounded bg-white/15 px-1 py-0.5 text-[10px] text-foreground-on-accent/85 hover:bg-white/25"
                          title={t("messages.jumpParent")}
                        >
                          <CornerUpLeft className="size-2.5" />#{m.in_reply_to}
                        </button>
                      )}
                      <p className="selectable whitespace-pre-wrap break-words font-body text-[13px] leading-snug">
                        {m.body}
                      </p>
                      <ImageAttachments paths={extractImagePaths(m.body)} />
                      <span className="float-right ml-2 mt-0.5 inline-block font-caption text-[10px] tabular-nums text-foreground-on-accent/70">
                        {formatClock(m.sent_at)}
                      </span>
                      <div className="pointer-events-none absolute -top-3 left-0 flex items-center gap-1 opacity-0 transition-opacity group-hover/bubble:pointer-events-auto group-hover/bubble:opacity-100">
                        <button
                          onClick={() => startReply(m)}
                          className="rounded-full border border-border-subtle bg-surface-elevated px-2 py-0.5 text-[10px] text-foreground-secondary shadow-sm hover:bg-surface-tertiary"
                          title={t("messages.reply")}
                        >
                          {t("messages.reply")}
                        </button>
                      </div>
                    </div>
                  </div>
                </div>
              );
            }

            // ── Agent turn: left-aligned, light contained bubble ─────────
            // Messenger pairing with the user's neutral bubble — agent gets a
            // light surface-secondary bubble + hairline border + bottom-left
            // tail. Authorship by alignment + bubble shade + a role header
            // sitting ABOVE the bubble (mirrors the user's "我" label). Unread
            // is the in-header dot + the once-per-thread "new messages"
            // divider — never a per-row coloured border.
            return (
              <div
                key={m.id}
                className={cn(
                  "flex flex-col",
                  !isFirstRow && (showHeader ? "mt-3" : "mt-2"),
                )}
              >
                {showDividerBefore && <TimeDivider ms={m.sent_at} />}
                {newDivider}
                <div className="group/msg flex gap-3">
                  {/* 28px gutter: role avatar on the group head, a hover-only
                      timestamp on collapsed follow-ups (Slack pattern). */}
                  <div className="flex w-7 shrink-0 justify-center">
                    {showHeader ? (
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
                    ) : (
                      <span className="mt-1 select-none font-caption text-[9px] leading-none tabular-nums text-foreground-tertiary opacity-0 transition-opacity group-hover/msg:opacity-100">
                        {formatClock(m.sent_at)}
                      </span>
                    )}
                  </div>

                  <div className="flex min-w-0 max-w-2xl flex-col items-start">
                    {showHeader && (
                      <div className="mb-0.5 flex items-baseline gap-2 px-0.5">
                        <span className="font-heading text-[13px] font-semibold text-foreground-primary">
                          {role}
                        </span>
                        <span className="font-caption text-[10px] tabular-nums text-foreground-tertiary">
                          {formatClock(m.sent_at)}
                        </span>
                        {isUnread && (
                          <span
                            className="size-1.5 rounded-full bg-accent-primary"
                            aria-hidden
                          />
                        )}
                      </div>
                    )}

                    <div
                      ref={(el) => {
                        if (el) rowRefs.current.set(m.id, el);
                        else rowRefs.current.delete(m.id);
                      }}
                      className={cn(
                        "group/bubble relative min-w-0 rounded-2xl rounded-bl-sm border border-border-subtle bg-surface-secondary px-3 py-2 shadow-sm transition-colors",
                        highlighted &&
                          "ring-2 ring-accent-primary ring-offset-1 ring-offset-surface-primary",
                      )}
                      title={`#${m.id} · ${m.kind} · ${formatFullStamp(m.sent_at)}`}
                    >
                      {m.in_reply_to != null && (
                        <button
                          onClick={() => jumpToParent(m.in_reply_to!)}
                          className="mb-1 flex items-center gap-0.5 rounded bg-surface-tertiary px-1 py-0.5 text-[10px] text-foreground-tertiary hover:bg-surface-secondary"
                          title={t("messages.jumpParent")}
                        >
                          <CornerUpLeft className="size-2.5" />#{m.in_reply_to}
                        </button>
                      )}
                      {/* Agent output is GFM markdown (headings/lists/code/
                          tables) — render it, don't show literal `##`/```. */}
                      <ChatMarkdown
                        content={m.body}
                        className="selectable text-foreground-primary"
                      />
                      <ImageAttachments paths={extractImagePaths(m.body)} />

                      {/* hover-only actions — top-right of the turn */}
                      <div className="pointer-events-none absolute -top-2 right-0 flex items-center gap-1 opacity-0 transition-opacity group-hover/bubble:pointer-events-auto group-hover/bubble:opacity-100">
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
              label={t("messages.respondingTo", {
                role: resolveRole(agentId, roleLookup),
              })}
              replyHint={t("messages.responding", { id: trigger.id })}
            />
          ))}
          {/* 之前这里有"<agent> 等你回话"的 ghost line — 删了。
              球在用户手里是默认状态,composer 在那儿本身就是邀请,
              再加文字提示反而冗余、翻译尴尬。awaitingAgents 仍然
              算出来供成员列表等其它地方用。 */}
        </div>
      </div>

      {/* ── Task activity (chat 内联状态卡片，"AI 正在派活...") ─────── */}
      {taskActivityBelow}

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
        {(composerImages.length > 0 || uploadingImage) && (
          <div className="flex flex-wrap items-center gap-2">
            {composerImages.map((p) => (
              <ComposerThumb
                key={p}
                path={p}
                onRemove={() => removeComposerImage(p)}
                removeLabel={t("messages.removeImage")}
              />
            ))}
            {uploadingImage && (
              <span className="flex h-16 w-16 items-center justify-center rounded-md border border-dashed border-border-subtle">
                <Loader2 className="size-4 animate-spin text-foreground-tertiary" />
              </span>
            )}
          </div>
        )}
        <div className="flex items-end gap-2">
          {/* picker(发给 XX ▼ + 换一个发送对象)已经去掉 —— 用户视角是
              "跟一个 AI 接待员对话",所有消息默认发给 scout(它会自己
              判断闲聊/转发/派活)。需要直接 ping 某个业务 agent 的高级
              场景未来通过 @mention 实现。 */}
          <Textarea
            ref={composerRef}
            name="composer"
            value={body}
            onChange={(e) => {
              setBody(e.target.value);
              autoGrow(e.target);
              // User edited the draft — the prior rewrite's undo no longer
              // applies; drop the affordances so they don't go stale.
              if (preOptimize !== null) setPreOptimize(null);
              if (optimizeNote !== null) setOptimizeNote(null);
            }}
            onKeyDown={onComposerKey}
            onPaste={onComposerPaste}
            onDrop={onComposerDrop}
            placeholder={composerPlaceholder}
            disabled={!canCompose}
            rows={1}
            className="min-w-0 flex-1 resize-none rounded-2xl px-3 py-2 font-body text-[13px] leading-snug"
          />
          {/* 「优化」 — ghost wand, left of Send so it reads as a draft helper,
              not a second send. Icon swaps to a spinner while rewriting. */}
          <Button
            variant="ghost"
            size="icon"
            onClick={optimize}
            disabled={optimizing || sending || !body.trim() || !canCompose}
            aria-label={t("messages.optimize")}
            title={t("messages.optimizeTooltip")}
            className="size-9 shrink-0 rounded-full text-foreground-tertiary transition-colors hover:bg-surface-tertiary hover:text-accent-primary disabled:opacity-40"
          >
            {optimizing ? (
              <Loader2 className="size-4 animate-spin" />
            ) : (
              <Sparkles className="size-4" />
            )}
          </Button>
          <Button
            size="icon"
            onClick={send}
            disabled={sendDisabled}
            title={sending ? t("messages.sending") : t("messages.send")}
            // 默认 Button disabled 只是 opacity:0.5，accent 色 + 50% 看起来
            // 跟 enabled 几乎一样 — 用户分不清是否能按。这里 disabled 切
            // 到灰底+灰图标，enabled 时强制 accent + 阴影，对比一目了然。
            className={cn(
              "size-9 shrink-0 rounded-full transition-colors",
              sendDisabled
                ? "!bg-surface-tertiary !text-foreground-tertiary !opacity-100 shadow-none"
                : "shadow-sm hover:shadow-md",
            )}
          >
            <Send className="size-4" />
          </Button>
        </div>
        {/* Hint row: left carries the optimize undo / "no change" feedback,
            right keeps the Enter-to-send hint. */}
        <div className="flex items-center justify-between gap-2">
          <div className="flex min-w-0 items-center gap-2">
            {preOptimize !== null && (
              <button
                type="button"
                onClick={undoOptimize}
                className="inline-flex shrink-0 items-center gap-1 rounded-full bg-accent-primary-soft px-2 py-0.5 font-caption text-[10px] text-accent-primary-deep transition-colors hover:bg-accent-primary-soft/70"
              >
                <Undo2 className="size-3" />
                {t("messages.optimizeUndo")}
              </button>
            )}
            {optimizeNote && (
              <span className="truncate font-caption text-[10px] text-foreground-tertiary">
                {optimizeNote}
              </span>
            )}
          </div>
          <span className="shrink-0 font-caption text-[10px] text-foreground-tertiary">
            {t("messages.sendHint")}
          </span>
        </div>
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

/** Slack-style "new messages" separator inserted once, before the first
 *  unread agent turn. Accent-tinted so it reads as "everything below is
 *  new" — the per-message amber border it replaces was both noisy (every
 *  unread row striped) and read as an AI-generated hack. */
function NewMessagesDivider({ label }: { label: string }) {
  return (
    <div
      className="my-3 flex w-full items-center gap-2"
      role="separator"
      aria-label={label}
    >
      <span className="h-px flex-1 bg-accent-primary/30" />
      <span className="rounded-full bg-accent-primary-soft px-2.5 py-0.5 font-caption text-[10px] font-semibold uppercase tracking-wide text-accent-primary">
        {label}
      </span>
      <span className="h-px flex-1 bg-accent-primary/30" />
    </div>
  );
}

/** "X is responding…" placeholder bubble shown while an agent has received a
 *  message but hasn't yet emitted a reply. Pure UI inference for now —
 *  upgraded to real server-side AgentState::Thinking events in UI/F.2-B. */
function PendingBubble({
  role,
  label,
  replyHint,
}: {
  role: string;
  label: string;
  replyHint: string;
}) {
  return (
    <div className="mt-3 flex gap-3">
      <div className="flex w-7 shrink-0 justify-center">
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
        <span className="mb-0.5 flex items-baseline gap-2 px-0.5 font-heading text-[13px] font-semibold text-foreground-primary">
          {role}
          <span className="font-caption text-[10px] font-normal text-foreground-tertiary">
            {label}
          </span>
        </span>
        <span
          className="flex items-center gap-1 rounded-2xl rounded-bl-sm border border-border-subtle bg-surface-secondary px-3 py-2 shadow-sm"
          title={replyHint}
        >
          <PendingDot delayMs={0} />
          <PendingDot delayMs={150} />
          <PendingDot delayMs={300} />
        </span>
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

/** Small removable image thumbnail shown above the composer for each image path
 *  in the draft (pasted/typed). Falls back to a filename chip if the file is
 *  gone, so the ✕-to-remove affordance still works. */
function ComposerThumb({
  path,
  onRemove,
  removeLabel,
}: {
  path: string;
  onRemove: () => void;
  removeLabel: string;
}) {
  const [failed, setFailed] = useState(false);
  return (
    <div className="group/att relative">
      {failed ? (
        <div
          className="flex h-16 w-16 items-center justify-center rounded-md border border-border-subtle bg-surface-tertiary p-1 text-center font-mono text-[8px] leading-tight text-foreground-tertiary"
          title={path}
        >
          {baseName(path)}
        </div>
      ) : (
        <img
          src={fileUrl(path)}
          alt={baseName(path)}
          onError={() => setFailed(true)}
          className="h-16 w-16 rounded-md border border-border-subtle object-cover"
          title={path}
        />
      )}
      <button
        type="button"
        onClick={onRemove}
        aria-label={removeLabel}
        title={removeLabel}
        className="absolute -right-1.5 -top-1.5 inline-flex size-5 items-center justify-center rounded-full border border-border-subtle bg-surface-elevated text-foreground-secondary shadow-sm transition hover:text-state-danger"
      >
        <X className="size-3" />
      </button>
    </div>
  );
}
