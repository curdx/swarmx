/**
 * AgentDrawer — Pencil frame t1JPL (540×920 right-side drawer).
 *
 * Anatomy:
 *   Header           HeaderTopRow (avatar + role + close)
 *                    ActionRow    (focus injector / wake / pause / restart / more)
 *   TabBar           终端 · 录像 · 消息 · 工具 · 上下文
 *   MainContent      one of the five tab panels
 *   InjectBar        prompt input + Send  (writes to /api/message as system→agent)
 *   StatBar          SPAWN · TURN · TOKEN · TOOLS · PTY · HOOK
 *
 * Tab caveats:
 *   - Terminal      : XtermPane via the existing WebGL pool; `visible` follows
 *                     the active tab so other panes can claim a GL context.
 *   - Recordings    : list of casts for this agent; cards navigate to /replays/:id
 *                     (never embedded — 540px is too narrow, see commit 005defc).
 *   - Messages      : mini message list filtered to from/to = this agent. Composer
 *                     intentionally NOT duplicated here — use InjectBar.
 *   - Tools         : placeholder. SwarmEvent has no tool_call type yet; needs a
 *                     shim-level emit. Hidden behind a soft "WIP" plate.
 *   - Context       : blackboard entries this agent wrote; click jumps to /context.
 *
 * Lifecycle:
 *   - Mount when ?agent=<id> appears in the route URL, unmount when removed.
 *   - We do NOT keep XtermPane mounted across opens — sessionStorage-backed
 *     `lastSeq` lets it reconnect with a gap-replay, so reopening is cheap
 *     enough. Keeping it mounted would hold a WebGL slot indefinitely.
 *
 * Inject semantics:
 *   The Pencil mock labels this "注入 prompt", but PTY input is owned exclusively
 *   by XtermPane. Sending the text as a swarm message (kind=note, from=system,
 *   to=agent_id) wakes the agent the same way an in-band human comment would —
 *   close enough to the design's intent without forking a new PTY-write path.
 */

import { useEffect, useMemo, useState } from "react";
import { Link } from "react-router-dom";
import {
  ChevronRight,
  Code2,
  Download,
  FileText,
  Loader2,
  MessageSquare,
  Pause,
  Play,
  RotateCcw,
  Terminal as TerminalIcon,
  Wrench,
  Zap,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import { api } from "../../api/http";
import type {
  AgentInfo,
  BlackboardEntry,
  MessageRecord,
  RecordingInfo,
  SwarmEvent,
} from "../../api/types";
import { XtermPane } from "../XtermPane";
import { useSwarmFeed } from "../../hooks/useSwarmFeed";
import { Button } from "@/components/ui/button";
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from "@/components/ui/sheet";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { cn } from "@/lib/cn";
import { roleColorClass as roleColor } from "@/lib/agent";

type TabId = "terminal" | "recordings" | "messages" | "tools" | "context";

const TABS: { id: TabId; labelKey: string; icon: typeof TerminalIcon }[] = [
  { id: "terminal", labelKey: "agent.tabs.terminal", icon: TerminalIcon },
  { id: "recordings", labelKey: "agent.tabs.recordings", icon: Play },
  { id: "messages", labelKey: "agent.tabs.messages", icon: MessageSquare },
  { id: "tools", labelKey: "agent.tabs.tools", icon: Wrench },
  { id: "context", labelKey: "agent.tabs.context", icon: FileText },
];

function formatDelta(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000));
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  const rem = s - m * 60;
  if (m < 60) return `${m}m${rem}s`;
  const h = Math.floor(m / 60);
  return `${h}h${m - h * 60}m`;
}

function formatTime(ms: number): string {
  return new Date(ms).toLocaleTimeString();
}

interface Props {
  agentId: string;
  onClose: () => void;
}

export function AgentDrawer({ agentId, onClose }: Props) {
  const [info, setInfo] = useState<AgentInfo | null>(null);
  const [tab, setTab] = useState<TabId>("terminal");
  const [now, setNow] = useState(Date.now());

  const refreshInfo = async () => {
    try {
      const rows = await api.listAgents();
      const found = rows.find((a) => a.agent_id === agentId) ?? null;
      setInfo(found);
    } catch {
      /* best-effort */
    }
  };

  useEffect(() => {
    refreshInfo();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [agentId]);

  // Tick once a second so SPAWN ago + uptime feel alive without re-rendering
  // the children every event.
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, []);

  // Esc 关闭由 Sheet 自身处理（Radix Dialog primitive 内部监听）。

  useSwarmFeed({
    onEvent: (ev: SwarmEvent) => {
      if (ev.type === "agent_state" && ev.agent_id === agentId) {
        refreshInfo();
      }
    },
  });

  const wake = () => api.wakeAgent(agentId).catch(() => {});
  // Pause toggles between interrupt (active → paused) and resume
  // (paused → active). Both refresh the local AgentInfo so the button
  // label flips immediately without waiting for the swarm feed.
  const togglePause = async () => {
    try {
      if (info?.paused) {
        await api.resumeAgent(agentId);
      } else {
        await api.interruptAgent(agentId);
      }
      await refreshInfo();
    } catch (e) {
      // eslint-disable-next-line no-console
      console.warn("toggle pause failed", e);
    }
  };

  return (
    <Sheet
      open
      modal={false}
      onOpenChange={(next) => {
        if (!next) onClose();
      }}
    >
      <SheetContent
        side="right"
        className="flex w-[540px] flex-col gap-0 border-l border-border-subtle bg-surface-elevated p-0 shadow-xl sm:max-w-[540px]"
        // modal=false 时 Radix 不渲染 overlay，但 SheetContent 默认 onInteractOutside
        // 会关 sheet — 我们希望用户点 chat 列表切 agent 不关 drawer，drawer
        // 跟着切新 agent_id 重新 mount 就行。
        onInteractOutside={(e) => e.preventDefault()}
        onPointerDownOutside={(e) => e.preventDefault()}
      >
        <SheetHeader className="sr-only">
          <SheetTitle>{`Agent drawer · ${info?.role ?? agentId}`}</SheetTitle>
          <SheetDescription>{`PTY 终端 / 录像 / 消息 / 工具 / 上下文 for agent ${agentId}`}</SheetDescription>
        </SheetHeader>
        <Header
          info={info}
          agentId={agentId}
          now={now}
          onWake={wake}
          onTogglePause={togglePause}
        />
        <TabBar tab={tab} onChange={setTab} />
        <div className="min-h-0 flex-1 overflow-hidden">
          {/* Mount all tabs concurrently is tempting (keeps terminal alive
              across tab switches), but xterm holds a WebGL slot and a WS
              so we'd burn budget on idle panes. Switch-unmount instead. */}
          {tab === "terminal" && <TerminalTab agentId={agentId} />}
          {tab === "recordings" && (
            <RecordingsTab agentId={agentId} wsId={info?.workspace ? info.workspace.slice(-8) : null} />
          )}
          {tab === "messages" && <MessagesTab agentId={agentId} />}
          {tab === "tools" && <ToolsTab />}
          {tab === "context" && (
            <ContextTab agentId={agentId} wsId={info?.workspace ? info.workspace.slice(-8) : null} />
          )}
        </div>
        <StatBar info={info} now={now} />
      </SheetContent>
    </Sheet>
  );
}

// ── Header ───────────────────────────────────────────────────────────────

function Header({
  info,
  agentId,
  now,
  onWake,
  onTogglePause,
}: {
  info: AgentInfo | null;
  agentId: string;
  now: number;
  onWake: () => void;
  onTogglePause: () => void;
}) {
  const { t } = useTranslation();
  const role = info?.role ?? "—";
  const cli = info?.cli ?? "—";
  const initial = role.slice(0, 1).toUpperCase();
  const spawnedAt = info?.spawned_at ?? null;
  const live = info && info.killed_at == null && info.shim_exit == null;
  const dotColor = !info
    ? "bg-state-idle"
    : !live
      ? "bg-state-idle"
      : info.shim_ready
        ? "bg-state-success"
        : "bg-state-wake";

  return (
    <header className="flex shrink-0 flex-col border-b border-border-subtle">
      {/* HeaderTopRow */}
      <div className="flex items-center gap-3 px-5 py-4">
        <div className="relative size-10 shrink-0">
          <div
            className={cn(
              "flex size-10 items-center justify-center rounded-full font-heading text-base font-bold text-foreground-on-accent",
              roleColor(role),
            )}
          >
            {initial}
          </div>
          <span
            className={cn(
              "absolute right-0 bottom-0 size-3 rounded-full ring-2 ring-surface-elevated",
              dotColor,
            )}
          />
        </div>
        <div className="flex min-w-0 flex-1 flex-col gap-1">
          <div className="flex items-center gap-2">
            <span className="font-heading text-base font-bold text-foreground-primary">
              {role}
            </span>
            <span className="rounded bg-surface-cool-tint px-1.5 py-0.5 font-mono text-[10px] text-foreground-secondary">
              {cli}
            </span>
            <span className="truncate font-mono text-[10px] text-foreground-tertiary">
              {agentId}
            </span>
          </div>
          <div className="flex items-center gap-1.5 font-caption text-[11px] text-foreground-secondary">
            {live && (
              <Loader2 className="size-3 animate-spin text-accent-primary" />
            )}
            <span>
              {live
                ? info?.shim_ready
                  ? t("agent.status.ready")
                  : t("agent.status.starting")
                : t("agent.status.exited")}
              {spawnedAt && (
                <> · {t("agent.status.uptime", { delta: formatDelta(now - spawnedAt) })}</>
              )}
            </span>
          </div>
        </div>
      </div>

      {/* ActionRow — 之前有「发送消息」按钮指向 drawer 自带的 InjectBar，
          但 chat 主 composer 上的 recipient picker 已经能选这个 agent
          发消息（同一 mailbox 路径，行为完全等价），两套 prompt-inject UI
          是重复，drawer 这边删了。Wake / Pause / Restart 保留，是 agent
          独有的操作。 */}
      <div className="flex items-center gap-2 px-5 pt-1 pb-4">
        <Button size="sm" variant="outline" onClick={onWake}>
          <Zap className="size-3" />
          {t("agent.wake")}
        </Button>
        <Button
          size="sm"
          variant="outline"
          onClick={onTogglePause}
          disabled={!live}
          title={info?.paused ? t("agent.resume") : t("agent.pause")}
        >
          {info?.paused ? (
            <>
              <Play className="size-3" />
              {t("agent.resume")}
            </>
          ) : (
            <>
              <Pause className="size-3" />
              {t("agent.pause")}
            </>
          )}
        </Button>
        <Button size="sm" variant="outline" disabled title={t("agent.notImplemented")}>
          <RotateCcw className="size-3" />
          {t("agent.restart")}
        </Button>
      </div>
    </header>
  );
}

// ── TabBar ───────────────────────────────────────────────────────────────

function TabBar({ tab, onChange }: { tab: TabId; onChange: (t: TabId) => void }) {
  const { t: tr } = useTranslation();
  return (
    <Tabs
      value={tab}
      onValueChange={(v) => onChange(v as TabId)}
      className="shrink-0"
    >
      <TabsList className="h-auto w-full justify-start rounded-none border-b border-border-subtle bg-transparent p-0 px-5">
        {TABS.map((item) => {
          const Icon = item.icon;
          return (
            <TabsTrigger
              key={item.id}
              value={item.id}
              className="relative gap-1.5 rounded-none border-0 bg-transparent px-4 py-3 text-xs text-foreground-secondary shadow-none hover:text-foreground-primary data-[state=active]:bg-transparent data-[state=active]:text-foreground-primary data-[state=active]:shadow-none data-[state=active]:after:absolute data-[state=active]:after:inset-x-0 data-[state=active]:after:-bottom-px data-[state=active]:after:h-0.5 data-[state=active]:after:bg-accent-primary"
            >
              <Icon className="size-3.5" />
              {tr(item.labelKey)}
            </TabsTrigger>
          );
        })}
      </TabsList>
    </Tabs>
  );
}

// ── Tab: Terminal ────────────────────────────────────────────────────────

function TerminalTab({ agentId }: { agentId: string }) {
  return (
    <div className="h-full bg-surface-inverse">
      <XtermPane agentId={agentId} visible />
    </div>
  );
}

// ── Tab: Recordings ──────────────────────────────────────────────────────

function RecordingsTab({
  agentId,
  wsId,
}: {
  agentId: string;
  wsId: string | null;
}) {
  const { t } = useTranslation();
  const [items, setItems] = useState<RecordingInfo[]>([]);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    api
      .listRecordings(agentId)
      .then((rows) => {
        if (!cancelled) setItems(rows);
      })
      .catch((e) => {
        if (!cancelled) setError((e as Error).message);
      });
    return () => {
      cancelled = true;
    };
  }, [agentId]);

  if (error) {
    return (
      <div className="p-5 text-xs text-state-danger">{error}</div>
    );
  }
  if (items.length === 0) {
    return (
      <div className="flex h-full items-center justify-center text-foreground-tertiary">
        <span className="font-caption text-sm">{t("agent.noRecordings")}</span>
      </div>
    );
  }
  return (
    <div className="flex h-full flex-col gap-2 overflow-y-auto p-4">
      {items.map((r) => {
        const live = r.finalized_at == null;
        return (
          <Link
            key={r.id}
            // 没拿到 wsId（agent 无 workspace 或还在加载）就退回 chat 主页 —
            // 录像本身没法在 Shell 外播放，给个能登陆的入口比 404 强。
            to={
              wsId
                ? `/chat/${wsId}/replays/${encodeURIComponent(r.id)}`
                : "/chat"
            }
            className="group flex items-center gap-3 rounded-md border border-border-subtle bg-surface-primary p-3 hover:border-accent-primary"
          >
            <span className="flex size-9 shrink-0 items-center justify-center rounded-md bg-term-bg text-term-green">
              <Play className="size-4" />
            </span>
            <div className="min-w-0 flex-1">
              <div className="truncate font-mono text-xs text-foreground-primary">
                {r.id}
              </div>
              <div className="font-caption text-[11px] text-foreground-tertiary">
                {r.cols}×{r.rows}
                {r.duration_ms != null && (
                  <> · {formatDelta(r.duration_ms)}</>
                )}{" "}
                · {formatTime(r.started_at)}
              </div>
            </div>
            <span
              className={cn(
                "rounded-full px-2 py-0.5 font-caption text-[10px]",
                live
                  ? "bg-status-running-soft text-status-running"
                  : "bg-surface-tertiary text-foreground-tertiary",
              )}
            >
              {live ? t("replays.live") : t("replays.completed")}
            </span>
            <a
              href={api.recordingCastUrl(r.id)}
              download={`${r.id}.cast`}
              onClick={(e) => e.stopPropagation()}
              className="flex size-7 items-center justify-center rounded-md text-foreground-tertiary hover:bg-surface-tertiary"
              title={t("agent.downloadCast")}
            >
              <Download className="size-3.5" />
            </a>
            <ChevronRight className="size-4 text-foreground-tertiary" />
          </Link>
        );
      })}
    </div>
  );
}

// ── Tab: Messages ────────────────────────────────────────────────────────

function MessagesTab({ agentId }: { agentId: string }) {
  const { t } = useTranslation();
  const [items, setItems] = useState<MessageRecord[]>([]);
  const [error, setError] = useState<string | null>(null);

  const refresh = async () => {
    try {
      // Two pulls, then merge in client. The server's listMessages doesn't
      // support "from=X OR to=X" in one query, so we cheat with two calls.
      // Volume is small per-agent; if this becomes hot, add a server filter.
      const [from, to] = await Promise.all([
        api.listMessages({ from: agentId, limit: 100 }),
        api.listMessages({ to: agentId, limit: 100 }),
      ]);
      const merged = [...from, ...to];
      const seen = new Set<number>();
      const dedup: MessageRecord[] = [];
      for (const m of merged) {
        if (seen.has(m.id)) continue;
        seen.add(m.id);
        dedup.push(m);
      }
      dedup.sort((a, b) => a.sent_at - b.sent_at);
      setItems(dedup);
      setError(null);
    } catch (e) {
      setError((e as Error).message);
    }
  };

  useEffect(() => {
    refresh();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [agentId]);

  useSwarmFeed({
    onEvent: (ev: SwarmEvent) => {
      if (ev.type !== "message") return;
      if (ev.from_agent !== agentId && ev.to_agent !== agentId) return;
      refresh();
    },
  });

  if (error) {
    return <div className="p-5 text-xs text-state-danger">{error}</div>;
  }
  if (items.length === 0) {
    return (
      <div className="flex h-full items-center justify-center text-foreground-tertiary">
        <span className="font-caption text-sm">{t("agent.noMessages")}</span>
      </div>
    );
  }
  return (
    <div className="flex h-full flex-col gap-2 overflow-y-auto p-4">
      {items.map((m) => {
        const outgoing = m.from_agent === agentId;
        return (
          <div
            key={m.id}
            className={cn(
              "rounded-md border p-3 text-xs",
              outgoing
                ? "border-accent-primary-soft bg-surface-accent-tint"
                : "border-border-subtle bg-surface-primary",
            )}
          >
            <div className="mb-1 flex items-center gap-2 font-caption text-[10px] text-foreground-tertiary">
              <span className="font-mono">#{m.id}</span>
              <span className="font-mono">{m.from_agent}</span>
              <ChevronRight className="size-3" />
              <span className="font-mono">{m.to_agent}</span>
              <span className="ml-auto">{formatTime(m.sent_at)}</span>
            </div>
            <div className="whitespace-pre-wrap font-mono text-foreground-primary">
              {m.body}
            </div>
          </div>
        );
      })}
    </div>
  );
}

// ── Tab: Tools (placeholder) ─────────────────────────────────────────────

function ToolsTab() {
  const { t } = useTranslation();
  return (
    <div className="flex h-full flex-col items-center justify-center gap-3 p-6 text-center text-foreground-tertiary">
      <Wrench className="size-10 opacity-40" />
      <div>
        <p className="font-heading text-sm font-semibold text-foreground-secondary">
          {t("agent.toolsWip")}
        </p>
        <p className="mt-1 max-w-xs font-caption text-xs">{t("agent.toolsWipHint")}</p>
      </div>
    </div>
  );
}

// ── Tab: Context ─────────────────────────────────────────────────────────

function ContextTab({
  agentId,
  wsId,
}: {
  agentId: string;
  wsId: string | null;
}) {
  const { t } = useTranslation();
  const [history, setHistory] = useState<
    { path: string; at: number; op: string }[]
  >([]);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const entries: BlackboardEntry[] = await api.listBlackboard();
        // Fan out history for each key, filter by this agent. Cheap on a
        // small board; if the board grows past hundreds of keys we'd want
        // an indexed-by-agent server endpoint.
        const histories = await Promise.all(
          entries.map(async (e) => {
            try {
              const rows = await api.listBlackboardHistory(e.path, 50, false);
              return rows
                .filter((h) => h.agent_id === agentId)
                .map((h) => ({ path: e.path, at: h.at, op: h.op }));
            } catch {
              return [];
            }
          }),
        );
        if (cancelled) return;
        const flat = histories.flat().sort((a, b) => b.at - a.at);
        setHistory(flat);
      } catch (e) {
        if (!cancelled) setError((e as Error).message);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [agentId]);

  if (error) {
    return <div className="p-5 text-xs text-state-danger">{error}</div>;
  }
  if (history.length === 0) {
    return (
      <div className="flex h-full items-center justify-center text-foreground-tertiary">
        <span className="font-caption text-sm">{t("agent.noBlackboard")}</span>
      </div>
    );
  }
  return (
    <div className="flex h-full flex-col gap-1 overflow-y-auto p-4">
      {history.map((h, i) => (
        <Link
          key={`${h.path}-${h.at}-${i}`}
          // 跳到当前 workspace 的 context view 并选中这个 key；wsId 缺失
          // 时回退到 chat 主页 (跳 /context 旧路径已死)。
          to={
            wsId
              ? `/chat/${wsId}/context?key=${encodeURIComponent(h.path)}`
              : "/chat"
          }
          className="flex items-center gap-2 rounded-md px-3 py-2 hover:bg-surface-tertiary"
        >
          <FileText className="size-4 shrink-0 text-foreground-tertiary" />
          <span className="flex-1 truncate font-mono text-xs text-foreground-primary">
            {h.path}
          </span>
          <span className="font-caption text-[10px] text-foreground-tertiary">
            {h.op} · {formatTime(h.at)}
          </span>
        </Link>
      ))}
    </div>
  );
}


// ── StatBar ──────────────────────────────────────────────────────────────

function StatBar({ info, now }: { info: AgentInfo | null; now: number }) {
  const { t } = useTranslation();
  const spawnedAt = info?.spawned_at ?? null;
  const live = info && info.killed_at == null && info.shim_exit == null;

  const stats = useMemo(
    () =>
      [
        {
          label: t("agent.stat.spawn"),
          value: spawnedAt
            ? t("agent.stat.ago", { delta: formatDelta(now - spawnedAt) })
            : "—",
        },
        // TURN / TOKEN / TOOLS are placeholders — the server doesn't expose
        // these yet. Pencil mock has them prominently so we keep slots
        // visible (— shows where real data will land) instead of dropping
        // them and re-flowing the bar later.
        { label: t("agent.stat.turn"), value: "—" },
        { label: t("agent.stat.token"), value: "—" },
        { label: t("agent.stat.tools"), value: "—" },
        {
          label: t("agent.stat.pty"),
          value: live ? t("agent.stat.live") : t("agent.stat.off"),
          color: live ? "text-state-success" : "text-foreground-tertiary",
        },
        {
          label: t("agent.stat.hook"),
          value: info?.shim_ready
            ? t("agent.stat.active")
            : info
              ? t("agent.stat.wait")
              : "—",
          color: info?.shim_ready
            ? "text-state-success"
            : "text-foreground-tertiary",
        },
      ] as const,
    [info, live, spawnedAt, now, t],
  );

  return (
    <footer className="flex shrink-0 items-center gap-5 border-t border-border-subtle bg-surface-secondary px-5 py-3">
      {stats.map((s) => (
        <div key={s.label} className="flex flex-col gap-0.5">
          <span className="font-caption text-[9px] tracking-[0.08em] text-foreground-tertiary">
            {s.label}
          </span>
          <span
            className={cn(
              "font-mono text-xs font-bold",
              "color" in s ? s.color : "text-foreground-primary",
            )}
          >
            {s.value}
          </span>
        </div>
      ))}
      <span className="ml-auto">
        <Code2 className="size-4 text-foreground-tertiary" />
      </span>
    </footer>
  );
}
