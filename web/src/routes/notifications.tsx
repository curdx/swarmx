/**
 * Notification Center — Pencil frame COJDW.
 *
 * Read-only feed assembled from the events flockmux-server already emits
 * via /ws/swarm: messages, blackboard writes, agent state transitions.
 *
 * No new backend endpoint is needed today — when /api/events lands we'll
 * swap the in-memory accumulator for a since-cursor pull, but the surface
 * stays the same.
 *
 * Persistence: only the read-id set lives in localStorage; notification
 * payload itself is ephemeral (lost on refresh; the initial mount pulls a
 * batch of recent messages + blackboard entries to seed the list).
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import type { TFunction } from "i18next";
import {
  AlertTriangle,
  Bell,
  CheckCircle2,
  CircleAlert,
  Inbox,
  MessageSquare,
  RefreshCw,
  Settings as SettingsIcon,
  X,
} from "lucide-react";
import { api } from "../api/http";
import type {
  AgentInfo,
  BlackboardEntry,
  MessageRecord,
  SwarmEvent,
  Workspace,
} from "../api/types";
import { useSwarmFeed } from "../hooks/useSwarmFeed";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/cn";
import { humanizeBlackboard, isHiddenWake } from "@/lib/notif";
import { buildRoleLookup, resolveRole } from "@/lib/agent";

type NotifKind = "message" | "blackboard" | "state" | "error" | "completed";

interface Notif {
  id: string;
  kind: NotifKind;
  agent: string;
  title: string;
  body?: string;
  at: number;
}

const READ_KEY = "flockmux:notif:read:v1";

function loadRead(): Set<string> {
  try {
    const raw = window.localStorage.getItem(READ_KEY);
    if (!raw) return new Set();
    return new Set(JSON.parse(raw));
  } catch {
    return new Set();
  }
}

function saveRead(s: Set<string>) {
  try {
    // Cap at 500 ids so the key doesn't grow unbounded.
    const arr = Array.from(s).slice(-500);
    window.localStorage.setItem(READ_KEY, JSON.stringify(arr));
  } catch {
    /* ignore */
  }
}

function fmtTime(ms: number): string {
  const delta = Date.now() - ms;
  if (delta < 60_000) return "_now_";
  if (delta < 3_600_000) return `_min_${Math.floor(delta / 60_000)}`;
  if (delta < 86_400_000) return `_hour_${Math.floor(delta / 3_600_000)}`;
  return new Date(ms).toLocaleString();
}

// Resolve the sentinel strings returned by fmtTime against the active locale.
function resolveTime(s: string, t: TFunction): string {
  if (s === "_now_") return t("notifications.time.now");
  if (s.startsWith("_min_")) return t("notifications.time.minAgo", { n: s.slice(5) });
  if (s.startsWith("_hour_")) return t("notifications.time.hourAgo", { n: s.slice(6) });
  return s;
}

const TABS = [
  { id: "all", labelKey: "notifications.tabs.all", icon: Bell },
  { id: "message", labelKey: "notifications.tabs.message", icon: MessageSquare },
  { id: "blackboard", labelKey: "notifications.tabs.blackboard", icon: Inbox },
  { id: "state", labelKey: "notifications.tabs.state", icon: SettingsIcon },
  { id: "error", labelKey: "notifications.tabs.error", icon: AlertTriangle },
  { id: "completed", labelKey: "notifications.tabs.completed", icon: CheckCircle2 },
] as const;

type TabId = (typeof TABS)[number]["id"];

// Bucket a message into error / completed / state / message.
//
// The old version did a bare `body.includes("error" | "failed")`, so any
// success summary that merely MENTIONED the word — "0 errors", "error
// handling done", "build passed, no errors" — got filed under 异常/Errors
// with a red alarm icon. That false positive was the worst kind: it makes a
// finished task look broken. We now grade the signal:
//   1. a hard failure glyph (❌ ✗ panic traceback) always wins;
//   2. otherwise an explicit success marker (✅ passed 完成 通过 …) means
//      completed — even if the body also says "error" somewhere;
//   3. only a *soft* failure word with NO success marker counts as an error.
/** A human label for an agent id in a notification title: "你"/"系统" for the
 *  user/system pseudo-agents, else the agent's role ("orchestrator", "Backend
 *  Engineer", …) resolved from /api/agent — never the raw `codex-7508c707`
 *  id, which means nothing to a user. Mirrors the chat's AgentChip. */
function friendlyAgent(
  id: string,
  roleLookup: Map<string, string>,
  t: TFunction,
): string {
  const r = resolveRole(id, roleLookup);
  if (r === "user") return t("notifications.fromUser");
  if (r === "system") return t("notifications.fromSystem");
  return r;
}

function classifyMessage(
  m: MessageRecord,
  t: TFunction,
  roleLookup: Map<string, string>,
): { kind: NotifKind; title: string } {
  // Structured first: server-stamped `meta.subtype` is ground truth, so we
  // never regex the prose body for messages the server controls (the
  // worker-disband farewell). Agent free-text (meta absent) still falls back
  // to the keyword heuristic below.
  if (m.meta?.subtype === "completion") {
    return {
      kind: "completed",
      title: t("notifications.kinds.completedTitle"),
    };
  }
  if (m.kind === "wake") {
    return {
      kind: "state",
      title: t("notifications.kinds.wakeTitle", {
        from: friendlyAgent(m.from_agent, roleLookup, t),
        to: friendlyAgent(m.to_agent, roleLookup, t),
      }),
    };
  }
  const body = m.body.toLowerCase();
  const error = () => ({
    kind: "error" as NotifKind,
    title: t("notifications.kinds.errorTitle"),
  });
  const completed = () => ({
    kind: "completed" as NotifKind,
    title: t("notifications.kinds.completedTitle"),
  });

  // Classifying free-form agent summaries by keyword is a tar pit — every
  // naive substring trips on its own negation/noun form. So we grade in
  // precedence order, each rule written to dodge the trap that bit the last:
  //
  // 1. HARD failure glyphs — never appear in a success handoff.
  if (/❌|✗|panic|traceback|stack trace|崩溃|报错/.test(body)) return error();

  // 2. NEGATED completion = a real failure verdict (未通过 / 没做好 / not
  //    passed). The negative-lookahead excludes the NOUN form 未完成数 /
  //    未完成项 — "remaining incomplete count" is a feature label, not a
  //    failed task (this is what mis-flagged a working todo-app summary).
  if (
    /(?:未|不|没|未能|没能)\s*(?:通过|完成|做好|做完|跑通|搞定|交付)(?!\s*(?:数|项|度|数量|个|列表|待办))/.test(
      body,
    ) ||
    /not\s+passed|did\s*n'?t\s+pass/.test(body)
  ) {
    return error();
  }

  // 3. Clear completion. Success WINS over a soft failure mention here, so a
  //    summary that merely says "0 errors" or CI config "失败时 upload report"
  //    (on-failure, not an actual failure) stays green instead of alarming.
  if (
    /✅|✔|全绿|搞定|完成|通过|做好|做完|全齐|跑通|已交付|交付完|passed|ready/.test(
      body,
    )
  ) {
    return completed();
  }

  // 4. Soft failure words that survived to here = an actual failure. Exclude
  //    the conditional "失败时/失败后…" (describing failure handling) and bare
  //    "error" (0 errors / error handling) which are not failures themselves.
  if (
    /失败(?!\s*(?:时|后|则|会|就|重试|的话|时候))|failed|failure|exception/.test(
      body,
    )
  ) {
    return error();
  }

  return {
    kind: "message",
    title: `${friendlyAgent(m.from_agent, roleLookup, t)} → ${friendlyAgent(m.to_agent, roleLookup, t)}`,
  };
}

/** Worker heartbeats (`<wsId>/<role>.progress.md`) are written on every
 *  milestone — dozens per task. They're already surfaced in the Ledger 近况
 *  pane; as notifications they bury the real messages. Skip them here. */
function isNoisyBlackboard(path: string): boolean {
  return path.endsWith(".progress.md");
}

/** Extract the numeric message id from a `msg-<N>` notification id, or null
 *  for non-message notifs (blackboard / state). Used to sync notification
 *  read-state down to the server's message.read_at so the chat unread badge
 *  clears too — notification "read" used to be localStorage-only, leaving
 *  /chat showing the same messages as unread after a reload. */
function msgIdOf(notifId: string): number | null {
  if (!notifId.startsWith("msg-")) return null;
  const n = Number(notifId.slice(4));
  return Number.isFinite(n) ? n : null;
}

export default function NotificationsRoute() {
  const { t } = useTranslation();
  const [items, setItems] = useState<Notif[]>([]);
  const [read, setRead] = useState<Set<string>>(loadRead);
  const [tab, setTab] = useState<TabId>("all");
  // Workspaces + their directions resolve a blackboard key's `{ws-id}/{slug}`
  // prefix into "{workspace} · {direction}" instead of a raw 32-hex UUID. Held
  // in a ref (not state) so the live SwarmFeed callback — a stable closure —
  // sees the latest list without re-subscribing; nothing renders off it
  // directly (titles are baked into each Notif when it's built).
  const wsRef = useRef<Workspace[]>([]);
  // agent_id → role label, for friendly "{role} → {role}" titles + source
  // chips (covers exited agents too — /api/agent returns them). Same ref
  // pattern as wsRef so the live SwarmFeed closure sees the latest map.
  const roleRef = useRef<Map<string, string>>(new Map());

  const seed = useCallback(async () => {
    try {
      const [msgs, bb, wss, agents] = await Promise.all([
        api.listMessages({ limit: 50 }),
        api.listBlackboard(),
        api.listWorkspaces().catch(() => [] as Workspace[]),
        api.listAgents().catch(() => [] as AgentInfo[]),
      ]);
      wsRef.current = wss;
      roleRef.current = buildRoleLookup(agents);
      const fromMsg: Notif[] = (msgs as MessageRecord[])
        .filter((m) => !isHiddenWake(m))
        .map((m) => {
          const c = classifyMessage(m, t, roleRef.current);
          return {
            id: `msg-${m.id}`,
            kind: c.kind,
            agent: friendlyAgent(m.from_agent, roleRef.current, t),
            title: c.title,
            body: m.body,
            at: m.sent_at,
          };
        });
      const fromBb: Notif[] = (bb as BlackboardEntry[])
        .filter((e) => !isNoisyBlackboard(e.path))
        .map((e) => {
          const h = humanizeBlackboard(e.path, wss, t);
          return {
            id: `bb-${e.path}-${e.at}`,
            kind: "blackboard" as const,
            agent: t("notifications.tabs.blackboard"),
            title: h.title,
            body: h.context ?? `sha256 ${e.sha256.slice(0, 8)}`,
            at: e.at,
          };
        });
      const all = [...fromMsg, ...fromBb].sort((a, b) => b.at - a.at);
      setItems(all);
    } catch {
      /* best-effort */
    }
  }, []);

  useEffect(() => {
    seed();
  }, [seed]);

  useSwarmFeed({
    onEvent: (ev: SwarmEvent) => {
      setItems((prev) => {
        let next: Notif | null = null;
        if (ev.type === "message") {
          if (isHiddenWake(ev)) return prev;
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
            meta: ev.meta,
          };
          const c = classifyMessage(rec, t, roleRef.current);
          next = {
            id: `msg-${ev.id}`,
            kind: c.kind,
            agent: friendlyAgent(ev.from_agent, roleRef.current, t),
            title: c.title,
            body: ev.body,
            at: ev.sent_at,
          };
        } else if (ev.type === "blackboard_changed") {
          if (isNoisyBlackboard(ev.path)) return prev;
          const h = humanizeBlackboard(ev.path, wsRef.current, t);
          next = {
            id: `bb-${ev.path}-${ev.at}`,
            kind: "blackboard",
            agent: ev.agent_id ?? t("notifications.tabs.blackboard"),
            title: h.title,
            body: h.context ?? `sha256 ${ev.sha256.slice(0, 8)}`,
            at: ev.at,
          };
        } else if (ev.type === "agent_state") {
          next = {
            id: `state-${ev.agent_id}-${ev.state}-${Date.now()}`,
            kind: ev.state === "exited" ? "completed" : "state",
            agent: ev.agent_id,
            title: `${ev.agent_id} → ${ev.state}`,
            at: Date.now(),
          };
        }
        if (!next) return prev;
        // Avoid duplicates by id (initial seed could race with live).
        if (prev.some((p) => p.id === next!.id)) return prev;
        return [next, ...prev].slice(0, 200);
      });
    },
    onReconnect: () => seed(),
  });

  const markRead = (id: string) => {
    setRead((prev) => {
      const next = new Set(prev);
      next.add(id);
      saveRead(next);
      return next;
    });
    // Also clear the server-side message read state for message notifs so the
    // chat unread badge stays in sync (to="user" only marks agent→user rows —
    // the ones chat counts; other ids no-op server-side). Best-effort.
    const mid = msgIdOf(id);
    if (mid != null) api.markMessagesRead("user", [mid]).catch(() => {});
  };
  const markAllRead = () => {
    // 防误触：100 条 unread 一下全 mark read 是不可逆操作（read 状态
    // 只存 localStorage，没法 undo）。当 unread 数 > 5 时弹一个 confirm
    // 让用户确认；少量 (≤5) 直接执行省点击。
    const unreadCount = items.filter((n) => !read.has(n.id)).length;
    if (unreadCount > 5) {
      const ok = window.confirm(
        t("notifications.markAllReadConfirm", { count: unreadCount }),
      );
      if (!ok) return;
    }
    setRead((prev) => {
      const next = new Set(prev);
      for (const n of items) next.add(n.id);
      saveRead(next);
      return next;
    });
    // Sync server message read state in one batch (see markRead) so reloading
    // /chat no longer re-surfaces these as unread.
    const mids = items
      .map((n) => msgIdOf(n.id))
      .filter((x): x is number => x != null);
    if (mids.length > 0) api.markMessagesRead("user", mids).catch(() => {});
  };

  const filtered = useMemo(
    () => (tab === "all" ? items : items.filter((n) => n.kind === tab)),
    [items, tab],
  );

  const totalUnread = items.filter((n) => !read.has(n.id)).length;
  const countBy = (k: TabId) =>
    items.filter((n) => k === "all" || n.kind === k).filter((n) => !read.has(n.id)).length;

  return (
    <div className="flex h-full flex-col bg-surface-primary">
      {/* Head */}
      <header className="flex h-14 shrink-0 items-center gap-3 border-b border-border-subtle bg-surface-elevated px-5">
        <span className="flex size-8 items-center justify-center rounded-md bg-accent-primary-soft">
          <Bell className="size-4 text-accent-primary-deep" />
        </span>
        <div className="flex flex-col">
          <h1 className="font-heading text-sm font-semibold text-foreground-primary">
            {t("notifications.title")}
          </h1>
          <span className="font-caption text-[10px] text-foreground-tertiary">
            {t("notifications.subtitle", { count: totalUnread })}
          </span>
        </div>
        <span className="flex-1" />
        <Button variant="outline" size="sm" onClick={markAllRead}>
          {t("notifications.markAllRead")}
        </Button>
        <Button
          variant="ghost"
          size="icon"
          onClick={seed}
          title={t("common.refresh")}
          className="size-8"
        >
          <RefreshCw className="size-4" />
        </Button>
      </header>

      {/* Tab bar */}
      <div className="flex h-11 shrink-0 items-center gap-1.5 border-b border-border-subtle bg-surface-secondary px-5">
        {TABS.map((tabItem) => {
          const Icon = tabItem.icon;
          const active = tabItem.id === tab;
          const c = countBy(tabItem.id);
          return (
            <Button
              key={tabItem.id}
              variant={active ? "default" : "outline"}
              size="sm"
              onClick={() => setTab(tabItem.id)}
              className="h-7 rounded-full"
            >
              <Icon className="size-3" />
              {t(tabItem.labelKey)}
              {c > 0 && (
                <Badge
                  variant={active ? "secondary" : "outline"}
                  className={cn(
                    "rounded-full px-1.5 py-0.5 text-[9px]",
                    active
                      ? "bg-foreground-on-accent text-accent-primary"
                      : "bg-surface-tertiary text-foreground-tertiary",
                  )}
                >
                  {c}
                </Badge>
              )}
            </Button>
          );
        })}
      </div>

      {/* List */}
      <div className="min-h-0 flex-1 overflow-y-auto px-3 py-3">
        {filtered.length === 0 ? (
          <div className="flex h-full flex-col items-center justify-center gap-3 text-foreground-tertiary">
            <Bell className="size-10 opacity-40" />
            <p className="font-caption text-sm">{t("notifications.empty")}</p>
          </div>
        ) : (
          <ul className="flex flex-col gap-1.5">
            {filtered.map((n) => {
              const isRead = read.has(n.id);
              const kindBg: Record<NotifKind, string> = {
                message: "bg-state-info/15 text-state-info",
                blackboard: "bg-accent-primary-soft text-accent-primary-deep",
                state: "bg-state-wake/20 text-state-wake",
                error: "bg-status-danger-soft text-status-danger",
                completed: "bg-status-success-soft text-status-success",
              };
              const kindIcon: Record<NotifKind, typeof Bell> = {
                message: MessageSquare,
                blackboard: Inbox,
                state: SettingsIcon,
                error: CircleAlert,
                completed: CheckCircle2,
              };
              const Icon = kindIcon[n.kind];
              return (
                <li
                  key={n.id}
                  className={cn(
                    "flex items-start gap-3 rounded-lg border border-border-subtle bg-surface-elevated px-3 py-2.5",
                    // unread 之前用整张 bg-surface-accent-tint — light 下还
                    // 行，dark 下解析成 blue-900 整片蓝海，视觉太重。改成
                    // Slack / Mail / Notion 同款 "左侧 accent bar" 风：卡片
                    // 主背景保持 surface-elevated（跟 read 一致），unread 只
                    // 用左边 2px 蓝条标识 + 右上 dot，subtle 但能扫到。
                    !isRead && "border-l-2 border-l-accent-primary",
                  )}
                >
                  <span
                    className={cn(
                      "flex size-7 shrink-0 items-center justify-center rounded-md",
                      kindBg[n.kind],
                    )}
                  >
                    <Icon className="size-3.5" />
                  </span>
                  <div className="min-w-0 flex-1">
                    <div className="flex items-center gap-2">
                      <span className="truncate font-heading text-sm font-semibold text-foreground-primary">
                        {n.title}
                      </span>
                      <span className="ml-auto font-caption text-[10px] text-foreground-tertiary">
                        {resolveTime(fmtTime(n.at), t)}
                      </span>
                    </div>
                    {n.body && (
                      <p className="mt-1 line-clamp-2 font-caption text-[11px] text-foreground-secondary">
                        {n.body}
                      </p>
                    )}
                    <div className="mt-1 flex items-center gap-2 font-caption text-[10px] text-foreground-tertiary">
                      <span className="font-mono">{n.agent}</span>
                      {!isRead && (
                        <span className="size-1.5 rounded-full bg-accent-primary" />
                      )}
                    </div>
                  </div>
                  {!isRead && (
                    <Button
                      variant="ghost"
                      size="icon"
                      onClick={() => markRead(n.id)}
                      title={t("notifications.markRead")}
                      className="size-6 text-foreground-tertiary hover:text-foreground-primary"
                    >
                      <X className="size-3" />
                    </Button>
                  )}
                </li>
              );
            })}
          </ul>
        )}
      </div>
    </div>
  );
}
