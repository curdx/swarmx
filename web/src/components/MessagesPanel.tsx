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
  lazy,
  Suspense,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { useTranslation } from "react-i18next";
import {
  Brain,
  ChevronDown,
  ChevronRight,
  Clock3,
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
import type { AgentActivity, AgentInfo, MessageRecord, ThoughtTraceStep } from "../api/types";
import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/textarea";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import { cn } from "@/lib/cn";
import { AgentChip } from "@/components/agent/AgentChip";
import { ImageAttachments } from "@/components/ImageAttachments";
import { ModelPicker } from "@/components/ModelPicker";
import { extractImagePaths, fileUrl, baseName } from "@/lib/imagePaths";
import { roleColorClass as roleColor } from "@/lib/agent";
import { getClientPlatformInfo } from "@/lib/platform";

const ChatMarkdown = lazy(() =>
  import("@/components/ChatMarkdown").then((m) => ({ default: m.ChatMarkdown })),
);

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
  /** Active direction's model_tier (null = global default). When `onSetModel`
   *  is also provided, the top bar shows a model picker. Omitted by the legacy
   *  /debug SwarmPanel (no direction). */
  modelTier?: string | null;
  /** Active direction's reasoning effort (null = model default). */
  reasoningEffort?: string | null;
  /** Change this direction's model and/or reasoning. Parent persists + restarts
   *  the orchestrator. Sends one knob; parent keeps the other. */
  onSetModel?: (cfg: { tier?: string | null; reasoning?: string | null }) => void;
  /** True while a model/effort switch is applying (picker shows a spinner). */
  modelBusy?: boolean;
  /** Per-agent activity stream from `/ws/swarm`, used to patch late tool events
   *  into an already-rendered reply trace without a manual refresh. */
  agentActivityById?: Record<string, AgentActivity[]>;
}

const KIND_DEFAULT = "note";
const USER_SENDER = "user";
const SYSTEM_SENDER = "system";
const GROUP_GAP_MS = 5 * 60_000; // 5 minutes — same heuristic as Telegram
/** Window during which an unanswered inbound message keeps the "typing"
 *  placeholder alive. Beyond this, the agent is probably stuck/done and
 *  the indicator is more misleading than helpful. */
const PENDING_TIMEOUT_MS = 60_000;
const MAX_REASONING_SUMMARY_MS = 30 * 60_000;

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

function formatElapsed(ms: number): string {
  const sec = Math.max(0, Math.floor(ms / 1000));
  if (sec < 60) return `${sec}s`;
  const min = Math.floor(sec / 60);
  const s = sec % 60;
  if (min < 60) return `${min}m ${String(s).padStart(2, "0")}s`;
  const h = Math.floor(min / 60);
  return `${h}h ${String(min % 60).padStart(2, "0")}m`;
}

function snippet(text: string, max = 42): string {
  const s = text.replace(/\s+/g, " ").trim();
  return s.length > max ? `${s.slice(0, max - 1)}…` : s;
}

interface ReasoningSummary {
  durationMs: number | null;
  steps: string[];
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
  modelTier = null,
  reasoningEffort = null,
  onSetModel,
  modelBusy = false,
  agentActivityById,
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

  // ── composer draft persistence (per workspace+direction) ──────────────────
  // Switching direction/workspace or reloading must not lose an in-progress
  // message. Keyed by (workspaceSlug, activeThreadId); restored on open,
  // saved on switch + tab-close, cleared on send.
  const draftKey = `flockmux:draft:v1:${workspaceSlug ?? "_"}:${activeThreadId ?? "main"}`;
  const bodyRef = useRef(body);
  bodyRef.current = body;
  const draftKeyRef = useRef(draftKey);
  useEffect(() => {
    // Load the incoming draft. The cleanup (runs on key change + unmount) saves
    // the OUTGOING draft under the key it belonged to (refs still hold the old
    // values at cleanup time — the new effect body updates them afterwards).
    let v = "";
    try {
      v = window.localStorage.getItem(draftKey) ?? "";
    } catch {
      /* ignore */
    }
    draftKeyRef.current = draftKey;
    setBody(v);
    return () => {
      try {
        const k = draftKeyRef.current;
        const val = bodyRef.current;
        if (val && val.trim()) window.localStorage.setItem(k, val);
        else window.localStorage.removeItem(k);
      } catch {
        /* ignore */
      }
    };
  }, [draftKey]);
  useEffect(() => {
    // Hard refresh / tab close doesn't run React cleanup — persist there too.
    const save = () => {
      try {
        const val = bodyRef.current;
        if (val && val.trim()) window.localStorage.setItem(draftKey, val);
      } catch {
        /* ignore */
      }
    };
    window.addEventListener("beforeunload", save);
    return () => window.removeEventListener("beforeunload", save);
  }, [draftKey]);
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
    if (!agentActivityById) return;
    setItems((prev) => {
      let changed = false;
      const next = prev.map((m) => {
        const trace = m.thought_trace;
        if (!trace || m.to_agent !== USER_SENDER) return m;
        const activities = agentActivityById[m.from_agent] ?? [];
        const lateSteps: ThoughtTraceStep[] = activities
          .filter((a) => a.phase === "ok" || a.phase === "error")
          .filter((a) => a.at >= trace.started_at)
          .filter((a) => trace.completed_at == null || a.at <= trace.completed_at + 30_000)
          .map((a) => ({
            phase: a.phase === "error" ? "tool_error" : "tool_ok",
            label: `完成工具: ${a.label}`,
            source: "agent",
            at: a.at,
          }));
        if (lateSteps.length === 0) return m;
        const summary = [...trace.summary];
        let messageChanged = false;
        for (const step of lateSteps) {
          if (
            summary.some(
              (s) =>
                s.phase === step.phase &&
                s.source === step.source &&
                s.label === step.label,
            )
          ) {
            continue;
          }
          summary.push(step);
          messageChanged = true;
        }
        if (!messageChanged) return m;
        changed = true;
        summary.sort((a, b) => a.at - b.at);
        return {
          ...m,
          thought_trace: {
            ...trace,
            summary: summary.slice(-12),
            updated_at: Math.max(trace.updated_at, ...summary.map((s) => s.at)),
          },
        };
      });
      return changed ? next : prev;
    });
  }, [agentActivityById]);

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
  const traceToSummary = useCallback((m: MessageRecord): ReasoningSummary | null => {
    const trace = m.thought_trace;
    if (!trace || trace.summary.length === 0) return null;
    const completedAt = trace.completed_at ?? m.sent_at;
    const duration = completedAt - trace.started_at;
    const durationMs =
      duration > 0 && duration <= MAX_REASONING_SUMMARY_MS ? duration : null;
    return {
      durationMs,
      steps: trace.summary.map((step) => step.label).filter(Boolean),
    };
  }, []);
  const reasoningByMessageId = useMemo(() => {
    const out = new Map<number, ReasoningSummary>();
    for (let i = 0; i < visible.length; i++) {
      const m = visible[i];
      if (
        m.from_agent === USER_SENDER ||
        m.from_agent === SYSTEM_SENDER ||
        m.to_agent !== USER_SENDER
      ) {
        continue;
      }
      const persisted = traceToSummary(m);
      if (persisted) {
        out.set(m.id, persisted);
        continue;
      }
      const priorUserIndex = (() => {
        for (let j = i - 1; j >= 0; j--) {
          if (visible[j].from_agent === USER_SENDER) return j;
        }
        return -1;
      })();
      if (priorUserIndex < 0) continue;
      const priorUser = visible[priorUserIndex];
      const duration = m.sent_at - priorUser.sent_at;
      const durationMs =
        duration > 0 && duration <= MAX_REASONING_SUMMARY_MS ? duration : null;
      const between = visible.slice(priorUserIndex + 1, i);
      const otherAgents = new Set(
        between
          .filter((x) => x.from_agent !== SYSTEM_SENDER && x.from_agent !== m.from_agent)
          .map((x) => resolveRole(x.from_agent, roleLookup)),
      );
      const role = resolveRole(m.from_agent, roleLookup);
      const steps = [
        t("messages.reasoning.stepUnderstood", {
          msg: snippet(priorUser.body),
        }),
        role === "orchestrator"
          ? t("messages.reasoning.stepOrchestrator")
          : t("messages.reasoning.stepWorker", { role }),
      ];
      if (otherAgents.size > 0) {
        steps.push(
          t("messages.reasoning.stepMerged", {
            count: otherAgents.size,
            roles: [...otherAgents].slice(0, 4).join(" · "),
          }),
        );
      }
      if (between.some((x) => x.kind === "reply" || x.kind === "note")) {
        steps.push(t("messages.reasoning.stepCheckedThread"));
      }
      steps.push(t("messages.reasoning.stepAnswered"));
      out.set(m.id, { durationMs, steps });
    }
    return out;
  }, [visible, roleLookup, t, traceToSummary]);

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

  // @mention: a leading/inline `@<role|id-prefix>` token routes the message to
  // THAT live worker instead of the default orchestrator (the token stays
  // visible, Slack-style). Resolution is forgiving: exact role, then id prefix.
  const explicitRecipient = useMemo(() => {
    const m = body.match(/(?:^|\s)@(\S+)/);
    if (!m) return null;
    const tok = m[1].toLowerCase();
    return (
      activeMembers.find((a) => a.role.toLowerCase() === tok) ??
      activeMembers.find((a) => a.agent_id.toLowerCase().startsWith(tok)) ??
      null
    );
  }, [body, activeMembers]);

  // @mention autocomplete: when the token being typed at the END of the input
  // starts with `@`, offer matching members to insert.
  const mentionQuery = useMemo(() => {
    const m = body.match(/(?:^|\s)@(\S*)$/);
    return m ? m[1].toLowerCase() : null;
  }, [body]);
  const mentionMatches = useMemo(() => {
    if (mentionQuery == null) return [];
    return activeMembers
      .filter(
        (a) =>
          a.role.toLowerCase().includes(mentionQuery) ||
          a.agent_id.toLowerCase().startsWith(mentionQuery),
      )
      .slice(0, 6);
  }, [mentionQuery, activeMembers]);
  const pickMention = (a: AgentInfo) => {
    setBody((b) => b.replace(/@(\S*)$/, `@${a.role} `));
    requestAnimationFrame(() => composerRef.current?.focus());
  };

  const send = async () => {
    const trimmed = body.trim();
    if (!trimmed) return;
    // @mention wins over the default orchestrator recipient.
    const recipient = explicitRecipient ?? defaultRecipient;
    // No live recipient (workspace's orchestrator has exited). If the parent
    // wired `onSend`, route the message through it — it spawns the orchestrator
    // and delivers — so the user just types instead of first clicking 唤醒.
    if (!recipient) {
      if (!onSend) return;
      setSending(true);
      try {
        await onSend(trimmed);
        try { window.localStorage.removeItem(draftKey); } catch { /* ignore */ }
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
        to: recipient.agent_id,
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
      api.wakeAgent(recipient.agent_id).catch(() => {
        /* swallow */
      });
      try { window.localStorage.removeItem(draftKey); } catch { /* ignore */ }
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
    // Desktop chat convention: Enter sends, Shift+Enter inserts newline.
    // On touch-first platforms the soft keyboard already exposes an explicit
    // send affordance, so Enter should stay as newline instead of surprise-send.
    const platform = getClientPlatformInfo();
    if (platform.isMobileLike) return;
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
  const platform = getClientPlatformInfo();
  const sendHint = platform.isMobileLike
    ? t("messages.sendHintMobile")
    : t("messages.sendHintDesktop", { enter: platform.enterKeyLabel });

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
          <div className="flex h-8 min-w-0 flex-1 items-center gap-2 rounded-md bg-surface-tertiary px-2.5">
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
              className="size-8 text-foreground-tertiary"
            >
              <X className="size-3" />
            </Button>
          </div>
        ) : (
          <>
            {onSetModel && (
              <ModelPicker
                tier={modelTier}
                reasoning={reasoningEffort}
                onSet={onSetModel}
                busy={modelBusy}
              />
            )}
            <span className="flex-1" />
            <Button
              variant="ghost"
              size="icon"
              onClick={() => setFilterOpen(true)}
              title={t("messages.filter")}
              className="size-8 text-foreground-tertiary"
            >
              <Search className="size-3.5" />
            </Button>
          </>
        )}
        <Popover open={bySenderOpen} onOpenChange={setBySenderOpen}>
          <PopoverTrigger asChild>
            <button
              className={cn(
                "relative flex size-8 items-center justify-center rounded-md hover:bg-surface-tertiary",
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
          className="size-8 text-foreground-tertiary"
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
        <div className="mx-auto flex w-full max-w-[1040px] flex-col gap-0.5">
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
            const reasoning = reasoningByMessageId.get(m.id);
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
                  <div className="flex w-full max-w-[min(82%,780px)] flex-col items-end">
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
                          className="mb-1 flex min-h-8 items-center gap-0.5 rounded bg-white/15 px-2 py-1 text-[10px] text-foreground-on-accent/85 hover:bg-white/25"
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
                          className="min-h-8 rounded-full border border-border-subtle bg-surface-elevated px-2.5 py-1 text-[10px] text-foreground-secondary shadow-sm hover:bg-surface-tertiary"
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
                  <div className="flex w-8 shrink-0 justify-center">
                    {showHeader ? (
                      <button
                        type="button"
                        onClick={() => onOpenAgent?.(m.from_agent)}
                        className={cn(
                          "flex size-8 items-center justify-center rounded-full text-xs font-medium text-foreground-on-accent shadow-sm transition-transform hover:scale-105",
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

                  <div className="flex min-w-0 w-full max-w-[min(82%,820px)] flex-col items-start">
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
                          className="mb-1 flex min-h-8 items-center gap-0.5 rounded bg-surface-tertiary px-2 py-1 text-[10px] text-foreground-tertiary hover:bg-surface-secondary"
                          title={t("messages.jumpParent")}
                        >
                          <CornerUpLeft className="size-2.5" />#{m.in_reply_to}
                        </button>
                      )}
                      {reasoning && (
                        <ReasoningDisclosure
                          summary={reasoning}
                          status="done"
                        />
                      )}
                      {/* Agent output is GFM markdown (headings/lists/code/
                          tables) — render it, don't show literal `##`/```. */}
                      <Suspense
                        fallback={
                          <p className="whitespace-pre-wrap text-foreground-primary">
                            {m.body}
                          </p>
                        }
                      >
                        <ChatMarkdown
                          content={m.body}
                          className="selectable text-foreground-primary"
                        />
                      </Suspense>
                      <ImageAttachments paths={extractImagePaths(m.body)} />

                      {/* hover-only actions — top-right of the turn */}
                      <div className="pointer-events-none absolute -top-2 right-0 flex items-center gap-1 opacity-0 transition-opacity group-hover/bubble:pointer-events-auto group-hover/bubble:opacity-100">
                        <button
                          onClick={() => startReply(m)}
                          className="min-h-8 rounded-full border border-border-subtle bg-surface-elevated px-2.5 py-1 text-[10px] text-foreground-secondary shadow-sm hover:bg-surface-tertiary"
                          title={t("messages.reply")}
                        >
                          {t("messages.reply")}
                        </button>
                        {isUnread && (
                          <button
                            onClick={() => markRead(m)}
                            disabled={marking === m.id}
                            className="min-h-8 rounded-full border border-border-subtle bg-surface-elevated px-2.5 py-1 text-[10px] text-foreground-secondary shadow-sm hover:bg-surface-tertiary disabled:opacity-50"
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
              trigger={trigger}
            />
          ))}
          {/* 之前这里有"<agent> 等你回话"的 ghost line — 删了。
              球在用户手里是默认状态,composer 在那儿本身就是邀请,
              再加文字提示反而冗余、翻译尴尬。awaitingAgents 仍然
              算出来供成员列表等其它地方用。 */}
        </div>
      </div>

      {/* ── Task activity (chat 内联状态卡片，"AI 正在派活...") ─────── */}
      <div className="mx-auto w-full max-w-[1040px] px-4">
        {taskActivityBelow}
      </div>

      {/* ── composer ─────────────────────────────────────────────────── */}
      <div className="flex shrink-0 flex-col gap-1.5 border-t border-border-subtle bg-surface-secondary px-3 py-2.5">
        <div className="mx-auto flex w-full max-w-[1040px] flex-col gap-1.5">
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
        {/* @mention autocomplete — appears while typing `@<token>` at the end
            of the input; selecting routes the message to that worker. */}
        {mentionMatches.length > 0 && (
          <div className="mb-1 overflow-hidden rounded-lg border border-border-subtle bg-surface-elevated shadow-lg">
            {mentionMatches.map((a) => (
              <button
                key={a.agent_id}
                type="button"
                onMouseDown={(e) => {
                  e.preventDefault();
                  pickMention(a);
                }}
                className="flex w-full items-center gap-2 px-3 py-1.5 text-left text-[12px] hover:bg-surface-tertiary"
              >
                <span className="font-medium text-foreground-primary">@{a.role}</span>
                <span className="font-mono text-[10px] text-foreground-tertiary">
                  {a.agent_id.slice(0, 8)}
                </span>
              </button>
            ))}
          </div>
        )}
        <div className="flex items-end gap-2">
          {/* Composer actions live inside the input shell so sending feels like
              a direct continuation of writing, not a detached toolbar action. */}
          <div className="relative min-w-0 flex-1">
            {/* default recipient = orchestrator/scout; an inline `@<role>`
                routes to a specific worker (explicitRecipient wins in send()). */}
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
              aria-label={t("messages.composerLabel")}
              placeholder={composerPlaceholder}
              disabled={!canCompose}
              rows={1}
              className="min-w-0 flex-1 resize-none rounded-2xl px-3 py-2 pr-[7.25rem] pb-12 font-body text-[13px] leading-snug"
            />
            <div className="pointer-events-none absolute inset-x-2 bottom-2 flex items-center justify-end gap-1.5">
              {/* 「优化」 — 次级 ghost action，保留在输入框里但弱于发送。 */}
              <Button
                variant="ghost"
                size="icon"
                onClick={optimize}
                disabled={optimizing || sending || !body.trim() || !canCompose}
                aria-label={t("messages.optimize")}
                title={t("messages.optimizeTooltip")}
                className="pointer-events-auto size-8 shrink-0 rounded-full text-foreground-tertiary transition-colors hover:bg-surface-tertiary hover:text-accent-primary disabled:opacity-40"
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
                aria-label={sending ? t("messages.sending") : t("messages.send")}
                title={sending ? t("messages.sending") : t("messages.send")}
                // 默认 Button disabled 只是 opacity:0.5，accent 色 + 50% 看
                // 起来跟 enabled 几乎一样。这里 disabled 切到灰底+灰图标，
                // enabled 时强制 accent + 阴影，对比一目了然。
                className={cn(
                  "pointer-events-auto size-8 shrink-0 rounded-full transition-colors",
                  sendDisabled
                    ? "!bg-surface-tertiary !text-foreground-tertiary !opacity-100 shadow-none"
                    : "shadow-sm hover:shadow-md",
                )}
              >
                <Send className="size-4" />
              </Button>
            </div>
          </div>
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
            {sendHint}
          </span>
        </div>
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
function ReasoningDisclosure({
  summary,
  status,
}: {
  summary: ReasoningSummary;
  status: "active" | "done";
}) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(status === "active");
  const active = status === "active";
  const elapsed =
    summary.durationMs == null ? null : formatElapsed(summary.durationMs);
  return (
    <div
      className={cn(
        "mb-2 overflow-hidden rounded-xl border text-[11px]",
        active
          ? "border-accent-primary/30 bg-accent-primary-soft/70"
          : "border-border-subtle bg-surface-primary/70",
      )}
    >
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        className="flex min-h-8 w-full items-center gap-2 px-2.5 py-1.5 text-left"
        aria-expanded={open}
      >
        {open ? (
          <ChevronDown className="size-3.5 shrink-0 text-foreground-tertiary" />
        ) : (
          <ChevronRight className="size-3.5 shrink-0 text-foreground-tertiary" />
        )}
        <Brain
          className={cn(
            "size-3.5 shrink-0",
            active ? "text-accent-primary" : "text-foreground-tertiary",
          )}
        />
        <span className="min-w-0 flex-1 truncate font-caption font-medium text-foreground-secondary">
          {active
            ? t("messages.reasoning.thinking")
            : t("messages.reasoning.summary")}
        </span>
        {elapsed && (
          <span className="inline-flex shrink-0 items-center gap-1 rounded-full bg-surface-elevated px-1.5 py-0.5 font-mono text-[10px] text-foreground-tertiary">
            <Clock3 className="size-3" />
            {elapsed}
          </span>
        )}
      </button>
      {open && (
        <ol className="space-y-1 border-t border-border-subtle/70 px-3 py-2 text-foreground-secondary">
          {summary.steps.map((step, idx) => (
            <li key={`${idx}-${step}`} className="flex gap-2 leading-snug">
              <span className="mt-0.5 size-1.5 shrink-0 rounded-full bg-accent-primary/70" />
              <span className="min-w-0">{step}</span>
            </li>
          ))}
        </ol>
      )}
    </div>
  );
}

function PendingBubble({
  role,
  label,
  replyHint,
  trigger,
}: {
  role: string;
  label: string;
  replyHint: string;
  trigger: MessageRecord;
}) {
  const { t } = useTranslation();
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    const id = window.setInterval(() => setNow(Date.now()), 500);
    return () => window.clearInterval(id);
  }, []);
  const trace = trigger.thought_trace;
  const elapsed = Math.max(0, now - (trace?.started_at ?? trigger.sent_at));
  const summary: ReasoningSummary =
    trace && trace.summary.length > 0
      ? {
          durationMs: elapsed,
          steps: trace.summary.map((step) => step.label).filter(Boolean),
        }
      : {
          durationMs: elapsed,
          steps: [
            t("messages.reasoning.stepUnderstood", {
              msg: snippet(trigger.body),
            }),
            role === "orchestrator"
              ? t("messages.reasoning.stepOrchestrator")
              : t("messages.reasoning.stepWorker", { role }),
            elapsed > 12_000
              ? t("messages.reasoning.stepExecuting")
              : t("messages.reasoning.stepPlanning"),
          ],
        };
  return (
    <div className="mt-3 flex gap-3">
      <div className="flex w-8 shrink-0 justify-center">
        <div
          className={cn(
            "flex size-8 items-center justify-center rounded-full text-xs font-medium text-foreground-on-accent shadow-sm",
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
        <div className="w-[min(82vw,520px)] rounded-2xl rounded-bl-sm border border-border-subtle bg-surface-secondary px-2.5 py-2 shadow-sm">
          <ReasoningDisclosure summary={summary} status="active" />
          <span
            className="flex items-center gap-1 px-1"
            title={replyHint}
          >
            <PendingDot delayMs={0} />
            <PendingDot delayMs={150} />
            <PendingDot delayMs={300} />
          </span>
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
