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
  Square,
  TriangleAlert,
  Undo2,
  X,
} from "lucide-react";
import { api } from "../api/http";
import type {
  AgentActivity,
  AgentInfo,
  AgentLiveState,
  MessageRecord,
  ThoughtTraceStep,
} from "../api/types";
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
import { activityVerb } from "@/lib/activityVerb";
import {
  EmptyState,
  type EmptyStateCliReadiness,
} from "@/components/chat/EmptyState";
import { SystemCard } from "@/components/chat/SystemCard";
import { getClientPlatformInfo } from "@/lib/platform";
import {
  buildRows,
  formatClock,
  formatDivider,
  formatElapsed,
  formatFullStamp,
  resolveRole,
} from "../lib/messageRows";
import { useRoleLookup } from "../lib/useRoleLookup";
import { useComposerDraft } from "../lib/useComposerDraft";
import { useScrollMarkRead } from "../lib/useScrollMarkRead";
import { usePendingResponders } from "../lib/usePendingResponders";
import { useInterruptControls } from "../lib/useInterruptControls";
import { useVirtualizer } from "@tanstack/react-virtual";

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
  /** Per-agent live state (state + latest activity) keyed by agent_id, from
   *  `/ws/swarm`. The pending "X 正在响应" placeholder binds to this so a member
   *  whose real state has gone error/exited is dropped immediately instead of
   *  lying with a typing bubble for 60s (P0-3, treats 诊断2 等待期撒谎 root).
   *  It also feeds the pending bubble's honest two-signal activity line. */
  agentLiveStateById?: Record<string, AgentLiveState>;
  /** Live in-flight reasoning steps keyed by agent id, fed by
   *  `thought_trace_event`. The pending bubble shows these growing in real time
   *  during the turn (real captured tool steps only) instead of the summary
   *  appearing only when the reply lands. */
  reasoningById?: Record<string, ReasoningSummary>;
  /** CLI engine readiness — renders the helpful empty-room state (P0-8) with
   *  starter prompts + an honest engine pre-check. Omitted by the legacy /debug
   *  panel ⇒ an empty room falls back to plain "暂无消息". */
  cliReadiness?: EmptyStateCliReadiness;
  /** Rendered in place of the plain "暂无消息" text when the room has no
   *  messages — the parent passes an honest startup checklist or failure card
   *  here when the orchestrator is starting / wedged, so an empty room is never
   *  silent about WHY it's empty. Omitted ⇒ the default empty-state text. */
  emptyStateOverride?: React.ReactNode;
}

const KIND_DEFAULT = "note";
const USER_SENDER = "user";
const SYSTEM_SENDER = "system";
const MAX_REASONING_SUMMARY_MS = 30 * 60_000;
/** A member whose latest tool event hasn't advanced in this long has gone
 *  quiet. The pending bubble degrades to a gray "已 Ns 无活动" and stops the
 *  typing dots — honest about being idle rather than faking motion. This is the
 *  soft文案级 threshold (45s); the hard red/amber stall verdict in
 *  `resolveMemberVisual` keeps its own 300s window (see spec §2.8 ⚖裁决). */
const HEARTBEAT_STALE_MS = 45_000;

export interface ReasoningSummary {
  durationMs: number | null;
  steps: string[];
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
  agentLiveStateById,
  reasoningById,
  cliReadiness,
  emptyStateOverride,
}: Props) {
  const aliveForInference = allAliveAgents ?? activeMembers;
  const { t } = useTranslation();
  const [items, setItems] = useState<MessageRecord[]>([]);
  const [filter, setFilter] = useState("");
  const [filterOpen, setFilterOpen] = useState(false);
  const [inReplyTo, setInReplyTo] = useState<number | null>(null);
  const [sending, setSending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // ── composer draft persistence (per workspace+direction) ──────────────────
  // Switching direction/workspace or reloading must not lose an in-progress
  // message. Keyed by (workspaceSlug, activeThreadId); restored on open,
  // saved on switch + tab-close, cleared on send. The state + persistence
  // effects live in useComposerDraft; the key is computed here because send
  // (below) also clears it.
  const draftKey = `flockmux:draft:v1:${workspaceSlug ?? "_"}:${activeThreadId ?? "main"}`;
  const [body, setBody] = useComposerDraft(draftKey);
  // 「优化」 button: the rewrite is reversible (preOptimize holds the pre-rewrite
  // draft for one-click undo); optimizeNote shows a transient "already clear".
  const [optimizing, setOptimizing] = useState(false);
  const [preOptimize, setPreOptimize] = useState<string | null>(null);
  const [optimizeNote, setOptimizeNote] = useState<string | null>(null);
  // Pasted/dropped clipboard images upload to /api/attachment; their saved path
  // is appended to the draft (agents read images by path).
  const [uploadingImage, setUploadingImage] = useState(false);
  // P0-11 附件失败回滚：上传失败的图不写进 body(避免发出一个并不存在的路径),
  // 而是单独挂红框「未上传·重试」缩略图,并禁用发送直到重试成功或主动移除。
  const [failedAttachments, setFailedAttachments] = useState<
    { id: string; name: string; file: File }[]
  >([]);
  const attachIdRef = useRef(0);
  const [marking, setMarking] = useState<number | null>(null);
  const [bySenderOpen, setBySenderOpen] = useState(false);
  // P0-9 排队提示 chip（send-side）：「打断」菜单 + interruptingId 已搬进
  // useInterruptControls；queuedHint 的 writer 在 send() 里，所以留这。
  const [queuedHint, setQueuedHint] = useState(false);

  const listRef = useRef<HTMLDivElement>(null);
  const rowRefs = useRef<Map<number, HTMLDivElement | null>>(new Map());
  const composerRef = useRef<HTMLTextAreaElement>(null);
  const [highlightId, setHighlightId] = useState<number | null>(null);

  // agent_id → role lookup covering exited agents too, so historical messages
  // render with the right avatar colour even after agents die.
  const roleLookup = useRoleLookup(activeMembers);

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

  // ── virtualization (P2-3) ─────────────────────────────────────────────
  // Only on-screen rows render — off-screen ones leave the DOM entirely. Row
  // heights are dynamic (markdown / reasoning / images), so each row measures
  // itself via measureElement. idToIndex maps message id → row index for
  // jump-to-parent / jump-to-unread, which can no longer read a DOM ref.
  const idToIndex = useMemo(() => {
    const m = new Map<number, number>();
    rows.forEach((r, i) => m.set(r.msg.id, i));
    return m;
  }, [rows]);
  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => listRef.current,
    estimateSize: () => 72,
    getItemKey: (i) => rows[i].msg.id,
    overscan: 6,
  });

  // F5 auto-mark-read (see useScrollMarkRead): a bubble scrolled into the
  // foregrounded viewport HAS plausibly been seen → debounced mark-read POST.
  // revision = the virtualizer's visible range, so the IntersectionObserver
  // re-subscribes over the rows currently mounted as the user scrolls.
  const vRange = virtualizer.range;
  const markReadRevision = vRange
    ? `${vRange.startIndex}-${vRange.endIndex}`
    : "";
  useScrollMarkRead({
    listRef,
    rowRefs,
    items,
    setItems,
    revision: markReadRevision,
  });
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
      // Only surface a reasoning summary backed by a REAL thought_trace. Was:
      // when traceToSummary returned null this FELL THROUGH and heuristically
      // FABRICATED ~5 plausible "理解了「…」/ 派给 worker / 汇总 N 个 agent /
      // 整理成最终回复" steps, then rendered them status="done" as if they were
      // the agent's actual completed thinking — a lie the backend never
      // produced (the reported "思考摘要" that's sometimes fake). Removed: no
      // real trace → no disclosure, matching the live PendingBubble's own
      // stated principle ("nothing yet → no invented steps").
      const persisted = traceToSummary(m);
      if (persisted) out.set(m.id, persisted);
    }
    return out;
  }, [visible, traceToSummary]);

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

  // ── pending responder / vanished turn inference (UI/F.2-A) ────────────
  // The "正在响应" bubble + "消失的回合" card state machine — interruptedTriggers,
  // the 5s give-up tick, both derivations (pure lib/pendingResponders), the
  // once-per-turn vanished console.warn, and markInterrupted — all live in
  // usePendingResponders now. The component just consumes the three returns
  // (runningMembers / stop-controls / send / onComposerKey / JSX below).
  const { pendingResponders, vanishedTurns, markInterrupted } =
    usePendingResponders({ items, aliveForInference, agentLiveStateById });

  // Auto-scroll to bottom on new items / live message / new pending bubble.
  // Virtualized: scroll the last row into view (align: end) rather than poking
  // scrollTop, so it lands against the measured total size, not an estimate.
  useLayoutEffect(() => {
    if (rows.length > 0) {
      virtualizer.scrollToIndex(rows.length - 1, { align: "end" });
    }
  }, [rows.length, pendingResponders.length, vanishedTurns.length, virtualizer]);

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

  // ── P0-9 可操控：在跑成员 + 打断 ────────────────────────────────────────
  // 「在跑」= ① 真实 swarm state 在 thinking/spawning 或有 running 工具活动
  //         (worker 走这条,被 tail 上报真实态);
  //         ② 正在响应 (pendingResponders 同一信号驱动「正在响应」气泡)——
  //         orchestrator 不被 tail、不上报 thinking,但它 mid-turn 时这里能算出,
  //         否则用户最常见的「队长独自干活」场景反而停不了。
  // 二者都基于已展示给用户的事实,菜单只在确有成员在跑时出现,不挂幽灵停止键。
  const runningMembers = useMemo(() => {
    const pendingIds = new Set(pendingResponders.map((p) => p.agentId));
    return activeMembers.filter((m) => {
      if (m.killed_at != null || m.shim_exit != null) return false;
      const live = agentLiveStateById?.[m.agent_id];
      return (
        live?.state === "thinking" ||
        live?.state === "spawning" ||
        live?.activity?.phase === "running" ||
        pendingIds.has(m.agent_id)
      );
    });
  }, [activeMembers, agentLiveStateById, pendingResponders]);

  const {
    stopMenuOpen,
    setStopMenuOpen,
    interruptingId,
    stopMember,
    stopAllRunning,
  } = useInterruptControls({ markInterrupted, runningMembers });

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
    // P0-11: never fire while an attachment failed to upload (Enter bypasses the
    // disabled send button, so guard here too).
    if (failedAttachments.length > 0) return;
    // @mention wins over the default orchestrator recipient.
    const recipient = explicitRecipient ?? defaultRecipient;
    // P0-9: if the captain is mid-turn, this message queues to its mailbox and
    // is read when the turn ends — surface that honestly with a transient chip
    // instead of letting it look like it vanished. The captain isn't tailed so
    // it never reports state="thinking"; fall back to the same mid-response
    // signal that drives the "正在响应" bubble (pendingResponders).
    const recipientBusy =
      recipient != null &&
      (agentLiveStateById?.[recipient.agent_id]?.state === "thinking" ||
        pendingResponders.some((p) => p.agentId === recipient.agent_id));
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
      if (recipientBusy) {
        setQueuedHint(true);
        window.setTimeout(() => setQueuedHint(false), 4000);
      }
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

  // 「重新发送」走独立通道,刻意**不碰输入框**:不读 explicitRecipient(@提及)/
  // inReplyTo / 附件,也绝不 setBody("") —— 否则会冲掉用户在输入框里还没发的草稿
  // (用户反馈)。直接把原文投给当前默认收件人(活队长直送;没有则经 onSend 拉起
  // 新队长投递),和原消息当初的去向一致。
  const resend = async (text: string) => {
    const trimmed = text.trim();
    if (!trimmed || sending) return;
    setSending(true);
    try {
      if (!defaultRecipient) {
        if (onSend) await onSend(trimmed);
      } else {
        const rec = await api.sendMessage({
          from: USER_SENDER,
          to: defaultRecipient.agent_id,
          kind: KIND_DEFAULT,
          body: trimmed,
        });
        setItems((prev) =>
          prev.some((m) => m.id === rec.id) ? prev : [...prev, rec],
        );
        api.wakeAgent(defaultRecipient.agent_id).catch(() => {
          /* swallow */
        });
      }
      setError(null);
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

  // Upload one image; throws on failure so the caller can decide whether to
  // append the path (success) or surface a retry (failure).
  const uploadOneImage = async (f: File): Promise<void> => {
    const guessExt = (f.type.split("/")[1] || "png").replace("jpeg", "jpg");
    const { path } = await api.uploadAttachment(f, f.name || `pasted.${guessExt}`);
    appendPath(path);
  };

  // Paste/drop a clipboard image → upload → append its saved path to the draft.
  // (Pasting a path string is just normal text; this handles raw bitmaps.)
  // P0-11: each file is uploaded independently. A failure no longer aborts the
  // batch or writes a phantom path — it parks the file as a retryable failed
  // attachment that blocks send until resolved.
  const handleImageFiles = async (files: File[]) => {
    const imgs = files.filter((f) => f.type.startsWith("image/"));
    if (imgs.length === 0) return false;
    setUploadingImage(true);
    setError(null);
    try {
      for (const f of imgs) {
        try {
          await uploadOneImage(f);
        } catch {
          setFailedAttachments((prev) => [
            ...prev,
            { id: String(++attachIdRef.current), name: f.name || "image", file: f },
          ]);
        }
      }
      composerRef.current?.focus();
    } finally {
      setUploadingImage(false);
    }
    return true;
  };

  const retryAttachment = async (id: string) => {
    const item = failedAttachments.find((a) => a.id === id);
    if (!item) return;
    try {
      await uploadOneImage(item.file);
      setFailedAttachments((prev) => prev.filter((a) => a.id !== id));
    } catch {
      /* still failing — keep the red thumb so the user can retry again */
    }
  };

  const dismissFailedAttachment = (id: string) => {
    setFailedAttachments((prev) => prev.filter((a) => a.id !== id));
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
    const idx = idToIndex.get(parentId);
    if (idx == null) return;
    virtualizer.scrollToIndex(idx, { align: "center", behavior: "smooth" });
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
    const idx = idToIndex.get(firstUnread.id);
    if (idx == null) return;
    virtualizer.scrollToIndex(idx, { align: "center", behavior: "smooth" });
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

  // P0-9: ⌘/Ctrl+Enter while the captain is mid-reply = interrupt it, then send
  // this message as a new instruction (a deliberate course-correction, distinct
  // from Enter which just queues). Only fires when the captain is actually
  // running, so it never surprise-kills an idle turn.
  const interruptThenSend = async (agentId: string) => {
    markInterrupted(agentId); // optimistic clear of the cancelled turn
    try {
      await api.interruptAgent(agentId);
    } catch {
      /* best-effort — still deliver the redirect */
    }
    await send();
  };

  const onComposerKey = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    // Desktop chat convention: Enter sends, Shift+Enter inserts newline.
    // On touch-first platforms the soft keyboard already exposes an explicit
    // send affordance, so Enter should stay as newline instead of surprise-send.
    const platform = getClientPlatformInfo();
    if (platform.isMobileLike) return;
    if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
      e.preventDefault();
      if (e.metaKey || e.ctrlKey) {
        const captain = explicitRecipient ?? defaultRecipient;
        const captainBusy =
          captain != null &&
          pendingResponders.some((p) => p.agentId === captain.agent_id);
        if (captain && captainBusy) {
          void interruptThenSend(captain.agent_id);
          return;
        }
      }
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
  // P0-11: a failed attachment blocks send so the user can't accidentally fire
  // a message that references an image which never uploaded.
  const hasFailedAttachment = failedAttachments.length > 0;
  const sendDisabled =
    sending || !body.trim() || !canCompose || hasFailedAttachment;
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
    // flex-1 + min-h-0 (not h-full): MessagesPanel must take the *remaining*
    // height after preceding siblings in the chat column (status strip +
    // PlanStickyCard), and be allowed to shrink so its internal scroll area
    // — not the page — absorbs overflow. `h-full` forced 100% of the parent
    // regardless of siblings, pushing the composer below the viewport once
    // the plan card appeared (the "no input box" bug).
    <div className="flex min-h-0 flex-1 flex-col bg-surface-primary">
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
        {rows.length === 0 &&
          (emptyStateOverride ??
            (cliReadiness ? (
              <EmptyState
                cliReadiness={cliReadiness}
                onPickStarter={(text) => {
                  setBody(text);
                  requestAnimationFrame(() => {
                    autoGrow(composerRef.current);
                    composerRef.current?.focus();
                  });
                }}
              />
            ) : (
              <p className="mt-10 text-center font-caption text-xs text-foreground-tertiary">
                {t("messages.empty")}
              </p>
            )))}
        <div
          className="relative mx-auto w-full max-w-[1040px]"
          style={{ height: virtualizer.getTotalSize() }}
        >
          {virtualizer.getVirtualItems().map((vi) => {
            const { msg: m, showHeader, showDividerBefore } = rows[vi.index];
            const isUser = m.from_agent === USER_SENDER;
            const isSystem = m.from_agent === SYSTEM_SENDER;
            // A worker's farewell/completion (from=worker, meta.subtype=
            // "completion") renders as a delivery card via SystemCard, not a
            // normal bubble — so "X 交付完成" reads as a structured event.
            const isDelivery = m.meta?.subtype === "completion";
            const role = resolveRole(m.from_agent, roleLookup);
            const isUnread =
              !isUser &&
              !isSystem &&
              !isDelivery &&
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

            // System events + worker delivery cards: centered structured card,
            // no bubble. Dispatched by meta.subtype inside SystemCard.
            if (isSystem || isDelivery) {
              return (
                <div
                  key={vi.key}
                  data-index={vi.index}
                  ref={virtualizer.measureElement}
                  className="absolute left-0 top-0 w-full"
                  style={{ transform: `translateY(${vi.start}px)` }}
                >
                  <div
                    ref={(el) => {
                      if (el) rowRefs.current.set(m.id, el);
                      else rowRefs.current.delete(m.id);
                    }}
                    className="py-1.5 flex flex-col items-center gap-0.5"
                  >
                    {showDividerBefore && <TimeDivider ms={m.sent_at} />}
                    {newDivider}
                    <span
                      className={cn(
                        highlighted && "rounded-lg ring-1 ring-accent-primary",
                      )}
                      title={`#${m.id} · ${m.kind} · ${formatFullStamp(m.sent_at)}`}
                    >
                      <SystemCard
                        message={m}
                        fromRole={role}
                        onOpenAgent={onOpenAgent}
                      />
                    </span>
                  </div>
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
                  key={vi.key}
                  data-index={vi.index}
                  ref={virtualizer.measureElement}
                  className="absolute left-0 top-0 w-full"
                  style={{ transform: `translateY(${vi.start}px)` }}
                >
                  <div
                    className={cn(
                      "flex flex-col items-end",
                      !isFirstRow && (showHeader ? "pt-3" : "pt-2"),
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
                key={vi.key}
                data-index={vi.index}
                ref={virtualizer.measureElement}
                className="absolute left-0 top-0 w-full"
                style={{ transform: `translateY(${vi.start}px)` }}
              >
                <div
                  className={cn(
                    "flex flex-col",
                    !isFirstRow && (showHeader ? "pt-3" : "pt-2"),
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
              </div>
            );
          })}
        </div>
        <div className="mx-auto flex w-full max-w-[1040px] flex-col gap-0.5">
          {pendingResponders.map(({ agentId, trigger }) => (
            <PendingBubble
              key={`pending-${agentId}`}
              role={resolveRole(agentId, roleLookup)}
              label={t("messages.respondingTo", {
                role: resolveRole(agentId, roleLookup),
              })}
              replyHint={t("messages.responding", { id: trigger.id })}
              trigger={trigger}
              live={agentLiveStateById?.[agentId]}
              liveReasoning={reasoningById?.[agentId]}
            />
          ))}
          {vanishedTurns.map((v) => (
            <VanishedTurnCard
              key={`vanished-${v.agentId}-${v.trigger.id}`}
              role={resolveRole(v.agentId, roleLookup)}
              reason={v.reason}
              sending={sending}
              // 直发原消息(独立通道,不碰输入框、不动你的草稿)。发出后它成为最新
              // 一条用户消息,上面 memo 的 latestUserTriggerAt 判定会让这张卡自动消失。
              onResend={() => {
                void resend(v.trigger.body);
              }}
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
        {(composerImages.length > 0 ||
          uploadingImage ||
          failedAttachments.length > 0) && (
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
            {/* P0-11: failed uploads — red thumb, retry, dismiss. Path never
                entered the draft, so it can't be sent by accident. */}
            {failedAttachments.map((a) => (
              <div key={a.id} className="group/att relative">
                <button
                  type="button"
                  onClick={() => retryAttachment(a.id)}
                  title={t("messages.attachFailedAria", {
                    name: a.name,
                    defaultValue: "{{name}} 上传失败，点击重试",
                  })}
                  className="flex h-16 w-16 flex-col items-center justify-center gap-1 rounded-md border border-status-danger/50 bg-status-danger-soft p-1 text-center transition-colors hover:bg-status-danger/15"
                >
                  <TriangleAlert className="size-4 shrink-0 text-status-danger" />
                  <span className="font-caption text-[9px] leading-tight text-status-danger">
                    {t("messages.attachRetry", "未上传 · 重试")}
                  </span>
                </button>
                <button
                  type="button"
                  onClick={() => dismissFailedAttachment(a.id)}
                  aria-label={t("messages.removeImage")}
                  title={t("messages.removeImage")}
                  className="absolute -right-1.5 -top-1.5 inline-flex size-5 items-center justify-center rounded-full border border-border-subtle bg-surface-elevated text-foreground-secondary shadow-sm transition hover:text-state-danger"
                >
                  <X className="size-3" />
                </button>
              </div>
            ))}
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
            {/* P0-9 可操控：打断在跑成员（仅在确有成员在跑时出现）。 */}
            {runningMembers.length > 0 && (
              <Popover open={stopMenuOpen} onOpenChange={setStopMenuOpen}>
                <PopoverTrigger asChild>
                  <button
                    type="button"
                    className="inline-flex shrink-0 items-center gap-1 rounded-full border border-border-subtle bg-surface-elevated px-2 py-0.5 font-caption text-[10px] text-foreground-secondary transition-colors hover:bg-surface-tertiary hover:text-state-danger"
                    title={t("messages.stopMenuLabel", "打断在跑的成员")}
                  >
                    <Square className="size-2.5" />
                    {t("messages.stopMenuShort", "打断")}
                    <span className="tabular-nums text-foreground-tertiary">
                      {runningMembers.length}
                    </span>
                  </button>
                </PopoverTrigger>
                <PopoverContent align="start" sideOffset={6} className="w-56 p-2">
                  <p className="mb-1.5 font-caption text-[10px] uppercase tracking-wider text-foreground-tertiary">
                    {t("messages.stopMenuHeading", "在跑成员")}
                  </p>
                  <ul className="flex flex-col gap-1">
                    {runningMembers.map((m) => (
                      <li
                        key={m.agent_id}
                        className="flex items-center gap-2 rounded px-1.5 py-1 text-[11px]"
                      >
                        <span
                          className={cn("size-2 shrink-0 rounded-full", roleColor(m.role))}
                        />
                        <span className="min-w-0 flex-1 truncate text-foreground-primary">
                          {m.role}
                        </span>
                        <button
                          type="button"
                          onClick={() => stopMember(m.agent_id)}
                          disabled={interruptingId === m.agent_id}
                          className="inline-flex shrink-0 items-center gap-1 rounded-full border border-border-subtle px-2 py-0.5 text-[10px] text-state-danger transition-colors hover:bg-status-danger-soft disabled:opacity-50"
                        >
                          {interruptingId === m.agent_id ? (
                            <Loader2 className="size-2.5 animate-spin" />
                          ) : (
                            <Square className="size-2.5" />
                          )}
                          {t("messages.stopMember", "停")}
                        </button>
                      </li>
                    ))}
                  </ul>
                  {runningMembers.length > 1 && (
                    <button
                      type="button"
                      onClick={stopAllRunning}
                      className="mt-1.5 w-full rounded-md border border-border-subtle px-2 py-1 text-center font-caption text-[11px] text-state-danger transition-colors hover:bg-status-danger-soft"
                    >
                      {t("messages.stopAll", "全部打断（{{count}}）", {
                        count: runningMembers.length,
                      })}
                    </button>
                  )}
                </PopoverContent>
              </Popover>
            )}
            {queuedHint && (
              <span className="inline-flex shrink-0 items-center gap-1 rounded-full bg-surface-tertiary px-2 py-0.5 font-caption text-[10px] text-foreground-secondary">
                <Clock3 className="size-3" />
                {t("messages.queuedToCaptain", "已排队 · 队长接手后送达")}
              </span>
            )}
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

/** "X 正在响应…" placeholder shown while a member has received a message but
 *  hasn't replied yet. Honesty rewrite (P0-2): instead of synthesizing a
 *  plausible "理解了… / 派给… / 执行中" reasoning trace (a lie when no real
 *  thought_trace exists), it binds to the member's REAL latest tool event:
 *    - a persisted `thought_trace` → show its real steps (never invented)
 *    - a running tool → two-signal line "正在 <白话动词> · <elapsed>", the
 *      elapsed counter ticking up = proof of life (verb via activityVerb,
 *      jargon-stripped)
 *    - quiet >45s → degrade to a gray "已 Ns 无活动" and STOP the dots
 *    - nothing yet → bare typing dots, no invented steps
 *  Death (state=error/exited) is handled upstream in `pendingResponders`,
 *  which drops the member so this never renders for a dead agent. */
function PendingBubble({
  role,
  label,
  replyHint,
  trigger,
  live,
  liveReasoning,
}: {
  role: string;
  label: string;
  replyHint: string;
  trigger: MessageRecord;
  live?: AgentLiveState;
  liveReasoning?: ReasoningSummary;
}) {
  const { t } = useTranslation();
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    const id = window.setInterval(() => setNow(Date.now()), 500);
    return () => window.clearInterval(id);
  }, []);

  // Only show a reasoning summary backed by REAL steps — no synthesized ones.
  // LIVE steps (streamed via thought_trace_event during the turn) win, so the
  // list grows in real time; the persisted trace is the fallback (e.g. on a
  // cold load before the live feed catches up). Neither → null → bare dots +
  // cumulative timer (the honest "thinking, nothing to show yet" floor).
  const trace = trigger.thought_trace;
  const persisted: ReasoningSummary | null =
    trace && trace.summary.length > 0
      ? {
          durationMs: Math.max(0, now - (trace.started_at ?? trigger.sent_at)),
          steps: trace.summary.map((step) => step.label).filter(Boolean),
        }
      : null;
  const realSummary: ReasoningSummary | null =
    liveReasoning && liveReasoning.steps.length > 0
      ? {
          steps: liveReasoning.steps,
          durationMs: Math.max(0, now - trigger.sent_at),
        }
      : persisted;

  // Real latest tool event drives the "what it's doing right now" verb line.
  const act = live?.activity;
  const verb = act ? activityVerb(act.label, act.kind) : null;
  const sinceActivityMs = act ? Math.max(0, now - act.at) : 0;
  const stale =
    act != null && act.phase === "running" && sinceActivityMs >= HEARTBEAT_STALE_MS;
  // The counter shows the TRUE cumulative wait — how long since the user's
  // message — not the latest single tool event's duration (which reset every
  // event and sat at "0s" for fast ops, the reported bug). This ticks up
  // honestly the whole turn = "队长已为你这条消息忙了 Ns". `now` refreshes every
  // 500ms (interval above). clamp ≥0 for client/server clock skew.
  const elapsedSinceTrigger = Math.max(0, now - trigger.sent_at);

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
          {realSummary && (
            <ReasoningDisclosure summary={realSummary} status="active" />
          )}
          {verb && !stale ? (
            <div
              className="flex items-center gap-2 px-1"
              role="status"
              aria-live="polite"
              title={replyHint}
            >
              <span className="inline-flex items-center gap-0.5">
                <PendingDot delayMs={0} />
                <PendingDot delayMs={150} />
                <PendingDot delayMs={300} />
              </span>
              <span className="min-w-0 flex-1 truncate font-body text-[12px] text-foreground-secondary">
                {t(verb.key, { ...verb.params, defaultValue: verb.fallback })}
              </span>
              <span className="shrink-0 font-mono text-[10px] tabular-nums text-foreground-tertiary">
                {formatElapsed(elapsedSinceTrigger)}
              </span>
            </div>
          ) : stale ? (
            // Neutral, not alarmed: a tool quiet for ≥45s might be genuinely
            // stuck OR just legitimately long (a big build/test emits no events
            // for minutes). We can't tell which, so we state the observable fact
            // — "已 Ns 无活动" — in calm gray and let the elapsed count speak,
            // rather than editorializing a fault (orange + alert triangle) we
            // haven't verified. Matches this component's own honesty docstring.
            <div
              className="flex items-center gap-1.5 px-1 font-caption text-[11px] text-foreground-secondary"
              role="status"
              aria-live="polite"
            >
              <Clock3 className="size-3 shrink-0" />
              <span className="truncate">
                {t("chat.live.memberStalled", {
                  secs: Math.floor(sinceActivityMs / 1000),
                  defaultValue: `已 ${Math.floor(sinceActivityMs / 1000)}s 无活动`,
                })}
              </span>
            </div>
          ) : (
            // No tool event yet (the captain is thinking before its first
            // action). Still show the honest cumulative counter next to the
            // dots so the user sees real motion from the moment they hit send —
            // not a frozen "0s" — and knows how long it's genuinely been working.
            <span
              className="flex items-center gap-2 px-1"
              role="status"
              aria-live="polite"
              title={replyHint}
            >
              <span className="inline-flex items-center gap-0.5">
                <PendingDot delayMs={0} />
                <PendingDot delayMs={150} />
                <PendingDot delayMs={300} />
              </span>
              <span className="shrink-0 font-mono text-[10px] tabular-nums text-foreground-tertiary">
                {formatElapsed(elapsedSinceTrigger)}
              </span>
            </span>
          )}
        </div>
      </div>
    </div>
  );
}

/** 入流律:队长收到任务却没回复就退出了 —— 把"正在响应…然后突然消失"换成一张
 *  诚实、可操作的卡(说明本轮没送达 + 一键把原消息填回输入框重发),而不是让
 *  气泡凭空消失。`reason` 仅在死因是显式错误(未登录 / 卡死等)时才有。 */
function VanishedTurnCard({
  role,
  reason,
  onResend,
  sending,
}: {
  role: string;
  reason: string | null;
  onResend: () => void;
  sending: boolean;
}) {
  const { t } = useTranslation();
  return (
    <div className="mt-3 flex gap-3">
      <div className="flex w-8 shrink-0 justify-center">
        <div className="flex size-8 items-center justify-center rounded-full bg-surface-tertiary text-state-warning shadow-sm">
          <TriangleAlert className="size-4" />
        </div>
      </div>
      <div className="flex min-w-0 flex-col items-start gap-1">
        <span className="px-0.5 font-heading text-[13px] font-semibold text-foreground-primary">
          {role}
        </span>
        <div className="w-[min(82vw,520px)] rounded-2xl rounded-bl-sm border border-state-warning/30 bg-status-warning-soft/50 px-3 py-2 shadow-sm">
          <p className="font-body text-[12px] leading-5 text-foreground-secondary">
            {t("chat.vanishedTurn.body", {
              role,
              defaultValue:
                "{{role}}本轮没有产出回复就退出了 —— 可能是登录失效、被重启或异常中断,你的上一条消息没能送达。",
            })}
          </p>
          {reason && (
            <p className="mt-1 break-words font-mono text-[11px] text-state-warning">
              {reason}
            </p>
          )}
          <button
            type="button"
            onClick={onResend}
            disabled={sending}
            className="mt-2 inline-flex items-center gap-1.5 rounded-md border border-border-subtle bg-surface-elevated px-2.5 py-1 font-caption text-[11px] text-foreground-secondary transition-colors hover:bg-surface-tertiary disabled:opacity-50"
          >
            <RefreshCw className={cn("size-3.5", sending && "animate-spin")} />
            {t("chat.vanishedTurn.resend", "重新发送这条消息")}
          </button>
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
