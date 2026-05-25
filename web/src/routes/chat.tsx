/**
 * Chat Main Window — corresponds to Pencil frame u6kF7Z.
 *
 * Three-column layout:
 *   GroupSidebar 264px | ChatMain (fill) | MembersSidebar 340px
 *
 * Subscription model mirrors SwarmPanel (the legacy /debug counterpart):
 * one /ws/swarm subscription per route, downstream MessagesPanel gets the
 * live message / read / unread tally via props. Members list pulls
 * /api/agent on mount and on every agent_state event (debounced 200ms,
 * same playbook as DebugRoute).
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { Plus, Sparkles, Users, Zap } from "lucide-react";
import { api } from "../api/http";
import type {
  AgentInfo,
  MessageRecord,
  SwarmEvent,
} from "../api/types";
import { MessagesPanel } from "../components/MessagesPanel";
import { useSwarmFeed } from "../hooks/useSwarmFeed";
import { cn } from "@/lib/cn";

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

function statusDot(a: AgentInfo) {
  if (a.killed_at) return { className: "bg-state-idle", label: "已退出" };
  if (a.shim_exit != null) return { className: "bg-state-idle", label: "shim 退出" };
  if (!a.shim_ready) return { className: "bg-state-wake", label: "启动中" };
  return { className: "bg-state-success", label: "在线" };
}

export default function ChatRoute() {
  const { workspaceId } = useParams<{ workspaceId?: string }>();
  const navigate = useNavigate();

  const [agents, setAgents] = useState<AgentInfo[]>([]);
  const [liveMessage, setLiveMessage] = useState<MessageRecord | null>(null);
  const [liveRead, setLiveRead] = useState<
    { ids: number[]; to_agent: string; at: number } | null
  >(null);
  const [unreadByFrom, setUnreadByFrom] = useState<Record<string, number>>({});
  const idToFromRef = useRef<Map<number, string>>(new Map());

  const refreshAgents = useCallback(async () => {
    try {
      const items = await api.listAgents();
      setAgents(items);
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn("listAgents failed", err);
    }
  }, []);

  const recomputeUnread = useCallback(async () => {
    try {
      const rows = await api.listMessages({ limit: 200 });
      const counts: Record<string, number> = {};
      const ids = new Map<number, string>();
      for (const m of rows) {
        ids.set(m.id, m.from_agent);
        if (m.read_at === null) {
          counts[m.from_agent] = (counts[m.from_agent] ?? 0) + 1;
        }
      }
      idToFromRef.current = ids;
      setUnreadByFrom(counts);
    } catch {
      // best-effort
    }
  }, []);

  useEffect(() => {
    refreshAgents();
    recomputeUnread();
  }, [refreshAgents, recomputeUnread]);

  // Debounced agent refresh on agent_state, same playbook as DebugRoute.
  const refreshTimerRef = useRef<number | null>(null);
  const scheduleRefresh = useCallback(() => {
    if (refreshTimerRef.current != null) {
      window.clearTimeout(refreshTimerRef.current);
    }
    refreshTimerRef.current = window.setTimeout(() => {
      refreshTimerRef.current = null;
      refreshAgents();
    }, 200);
  }, [refreshAgents]);

  const status = useSwarmFeed({
    onEvent: (ev: SwarmEvent) => {
      switch (ev.type) {
        case "agent_state":
          scheduleRefresh();
          break;
        case "message": {
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
          setLiveMessage(rec);
          idToFromRef.current.set(ev.id, ev.from_agent);
          setUnreadByFrom((prev) => ({
            ...prev,
            [ev.from_agent]: (prev[ev.from_agent] ?? 0) + 1,
          }));
          break;
        }
        case "message_read":
          setLiveRead({ ids: ev.ids, to_agent: ev.to_agent, at: ev.at });
          setUnreadByFrom((prev) => {
            const next = { ...prev };
            for (const id of ev.ids) {
              const from = idToFromRef.current.get(id);
              if (!from) continue;
              const cur = next[from] ?? 0;
              const dec = Math.max(0, cur - 1);
              if (dec === 0) delete next[from];
              else next[from] = dec;
            }
            return next;
          });
          break;
      }
    },
    onReconnect: () => {
      scheduleRefresh();
      recomputeUnread();
    },
  });

  // Group agents by workspace path. The chat-room metaphor maps cleanly:
  // one workspace = one room, members = agents sharing that workspace_dir.
  const workspaces = useMemo(() => {
    const live = agents.filter((a) => a.killed_at == null && a.shim_exit == null);
    const byWs = new Map<string, AgentInfo[]>();
    for (const a of live) {
      const key = a.workspace || "(no workspace)";
      if (!byWs.has(key)) byWs.set(key, []);
      byWs.get(key)!.push(a);
    }
    return Array.from(byWs.entries()).map(([path, members]) => ({
      path,
      members,
      // Stable id = last 8 chars of path; good enough for URL routing.
      id: path.slice(-8) || "default",
    }));
  }, [agents]);

  const activeWs =
    workspaces.find((w) => w.id === workspaceId) ?? workspaces[0] ?? null;
  const activeMembers = activeWs?.members ?? [];

  const totalUnread = Object.values(unreadByFrom).reduce((a, b) => a + b, 0);

  return (
    <div className="flex h-full min-h-0">
      {/* ── Left: workspace / group list ─────────────────────────── */}
      <aside className="flex w-[264px] shrink-0 flex-col gap-3 border-r border-border-subtle bg-surface-secondary px-2 py-3">
        <div className="flex items-center justify-between px-2">
          <h2 className="font-heading text-xs font-semibold uppercase tracking-wider text-foreground-tertiary">
            工作空间
          </h2>
          <button
            type="button"
            className="rounded-md p-1 text-foreground-tertiary hover:bg-surface-tertiary hover:text-foreground-primary"
            title="新建工作空间 (TODO: Wizard)"
          >
            <Plus className="size-4" />
          </button>
        </div>
        <nav className="flex flex-col gap-0.5 overflow-y-auto">
          {workspaces.length === 0 && (
            <span className="px-3 py-2 font-caption text-xs text-foreground-tertiary">
              暂无活跃 agent · 去 <a className="text-accent-peach hover:underline" href="/debug">/debug</a> 启动一个
            </span>
          )}
          {workspaces.map((ws) => {
            const active = ws.id === activeWs?.id;
            return (
              <button
                key={ws.id}
                onClick={() => navigate(`/chat/${ws.id}`)}
                className={cn(
                  "group flex items-center gap-2 rounded-md px-2 py-1.5 text-left text-sm transition-colors",
                  active
                    ? "bg-accent-peach-soft text-foreground-primary"
                    : "text-foreground-secondary hover:bg-surface-tertiary",
                )}
              >
                <span className="size-2 shrink-0 rounded-full bg-state-success" />
                <span className="flex-1 truncate font-mono text-xs">
                  {ws.path.split("/").slice(-2).join("/") || ws.path}
                </span>
                <span className="font-caption text-[10px] text-foreground-tertiary">
                  {ws.members.length}
                </span>
              </button>
            );
          })}
        </nav>
        <div className="mt-auto px-2 pt-3">
          <button className="flex w-full items-center justify-center gap-2 rounded-md bg-accent-peach px-3 py-2 text-xs font-medium text-foreground-on-accent hover:bg-accent-peach-deep">
            <Sparkles className="size-4" />
            运行配方
          </button>
        </div>
      </aside>

      {/* ── Center: messages ─────────────────────────────────────── */}
      <section className="flex min-w-0 flex-1 flex-col bg-surface-primary">
        <div className="flex h-12 shrink-0 items-center gap-3 border-b border-border-subtle px-4">
          <h1 className="font-heading text-sm font-semibold text-foreground-primary">
            {activeWs
              ? activeWs.path.split("/").slice(-2).join("/")
              : "暂无对话"}
          </h1>
          <span className="font-caption text-xs text-foreground-tertiary">
            {activeMembers.length} 个 agent · WS {status}
          </span>
          <span className="flex-1" />
          {totalUnread > 0 && (
            <span className="rounded-full bg-accent-peach px-2 py-0.5 text-[10px] font-semibold text-foreground-on-accent">
              {totalUnread} 未读
            </span>
          )}
        </div>
        <div className="min-h-0 flex-1 overflow-hidden">
          {/* MessagesPanel keeps its legacy inline styles; wrap to bound it. */}
          <div className="h-full">
            <MessagesPanel
              liveMessage={liveMessage}
              liveRead={liveRead}
              unreadByFrom={unreadByFrom}
            />
          </div>
        </div>
      </section>

      {/* ── Right: members ───────────────────────────────────────── */}
      <aside className="flex w-[340px] shrink-0 flex-col border-l border-border-subtle bg-surface-elevated">
        <div className="flex h-12 shrink-0 items-center gap-2 border-b border-border-subtle px-4">
          <Users className="size-4 text-foreground-tertiary" />
          <h2 className="font-heading text-xs font-semibold uppercase tracking-wider text-foreground-tertiary">
            成员
          </h2>
          <span className="ml-auto font-caption text-xs text-foreground-tertiary">
            {activeMembers.length}
          </span>
        </div>
        <div className="flex-1 overflow-y-auto px-2 py-2">
          {activeMembers.length === 0 && (
            <p className="px-3 py-2 font-caption text-xs text-foreground-tertiary">
              选中一个工作空间查看成员
            </p>
          )}
          {activeMembers.map((a) => {
            const dot = statusDot(a);
            const unread = unreadByFrom[a.agent_id] ?? 0;
            return (
              <div
                key={a.agent_id}
                className="flex items-center gap-3 rounded-md px-3 py-2 hover:bg-surface-tertiary"
              >
                <span
                  className={cn(
                    "flex size-8 shrink-0 items-center justify-center rounded-full text-xs font-medium text-foreground-on-accent",
                    roleColor(a.role),
                  )}
                  title={a.role}
                >
                  {a.role.slice(0, 1).toUpperCase()}
                </span>
                <div className="min-w-0 flex-1">
                  <div className="flex items-center gap-2">
                    <span className="truncate font-heading text-sm text-foreground-primary">
                      {a.role}
                    </span>
                    <span
                      className={cn("size-1.5 rounded-full", dot.className)}
                      title={dot.label}
                    />
                  </div>
                  <div className="truncate font-mono text-[10px] text-foreground-tertiary">
                    {a.cli} · {a.agent_id.slice(-8)}
                  </div>
                </div>
                {unread > 0 && (
                  <span className="rounded-full bg-state-danger px-1.5 py-0.5 text-[10px] font-semibold text-foreground-on-accent">
                    {unread}
                  </span>
                )}
                <button
                  className="rounded-md p-1.5 text-foreground-tertiary hover:bg-surface-secondary hover:text-state-wake"
                  title="手动唤醒"
                  onClick={() => api.wakeAgent(a.agent_id).catch(() => {})}
                >
                  <Zap className="size-4" />
                </button>
              </div>
            );
          })}
        </div>
      </aside>
    </div>
  );
}
