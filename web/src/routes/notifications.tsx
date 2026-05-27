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

import { useCallback, useEffect, useMemo, useState } from "react";
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
import type { BlackboardEntry, MessageRecord, SwarmEvent } from "../api/types";
import { useSwarmFeed } from "../hooks/useSwarmFeed";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/cn";

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

function classifyMessage(
  m: MessageRecord,
  t: TFunction,
): { kind: NotifKind; title: string } {
  const body = m.body.toLowerCase();
  if (body.includes("error") || body.includes("failed") || body.includes("✗")) {
    return { kind: "error", title: t("notifications.kinds.errorTitle") };
  }
  if (body.includes("✅") || body.includes("passed") || body.includes("done")) {
    return { kind: "completed", title: t("notifications.kinds.completedTitle") };
  }
  if (m.kind === "wake") {
    return {
      kind: "state",
      title: t("notifications.kinds.wakeTitle", {
        from: m.from_agent,
        to: m.to_agent,
      }),
    };
  }
  return { kind: "message", title: `${m.from_agent} → ${m.to_agent}` };
}

export default function NotificationsRoute() {
  const { t } = useTranslation();
  const [items, setItems] = useState<Notif[]>([]);
  const [read, setRead] = useState<Set<string>>(loadRead);
  const [tab, setTab] = useState<TabId>("all");

  const seed = useCallback(async () => {
    try {
      const [msgs, bb] = await Promise.all([
        api.listMessages({ limit: 50 }),
        api.listBlackboard(),
      ]);
      const fromMsg: Notif[] = (msgs as MessageRecord[]).map((m) => {
        const c = classifyMessage(m, t);
        return {
          id: `msg-${m.id}`,
          kind: c.kind,
          agent: m.from_agent,
          title: c.title,
          body: m.body,
          at: m.sent_at,
        };
      });
      const fromBb: Notif[] = (bb as BlackboardEntry[]).map((e) => ({
        id: `bb-${e.path}-${e.at}`,
        kind: "blackboard",
        agent: "blackboard",
        title: `${e.op} ${e.path}`,
        body: `sha256 ${e.sha256.slice(0, 8)}`,
        at: e.at,
      }));
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
          };
          const c = classifyMessage(rec, t);
          next = {
            id: `msg-${ev.id}`,
            kind: c.kind,
            agent: ev.from_agent,
            title: c.title,
            body: ev.body,
            at: ev.sent_at,
          };
        } else if (ev.type === "blackboard_changed") {
          next = {
            id: `bb-${ev.path}-${ev.at}`,
            kind: "blackboard",
            agent: ev.agent_id ?? "blackboard",
            title: `${ev.op} ${ev.path}`,
            body: `sha256 ${ev.sha256.slice(0, 8)}`,
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
