/**
 * WorkspaceShell — 4 个工作空间内 view (chat / dag / replays / context) 的
 * 共享 chrome。React Router 拿它当 layout route：所有 /chat/:wsId/* 都进这
 * 一层，子 view 通过 <Outlet/> 渲染。
 *
 * 解决的核心问题：之前每个子 view 是独立的 top-level route，切 tab → 整页
 * 卸载 + 重画，连工作空间列表都不在了，用户感觉自己"跳了页面"而不是
 * "切了视图"。Shell 化之后切 tab 只重渲染 Outlet：
 *   - 左侧工作空间列表常驻
 *   - Channel header（workspace 名 + 路径 + 未读 + 复制）常驻
 *   - Tab bar 常驻
 *   - swarm event 订阅常驻 → 切走再回来 unread/agent state 不丢
 *
 * Outlet context 把 activeWs / agents / liveMessage 等下发给子 view，
 * 避免每个 view 自己 listAgents / 开 swarm subscription（之前协作图
 * 和录像页各开了一个，重复请求 + 重复 ws 连接）。
 *
 * 拆分（god-file 治理）：侧栏 + 根源树 + 管理对话框搬到 WorkspaceSidebar，
 * tab 栏 + 视图过渡搬到 WorkspaceToolbar，WorkspaceSummary 类型搬到 ./types。
 * 本文件只剩"数据编排 + 布局"。WorkspaceList / WorkspaceSummary 在此 re-export，
 * 既有 import 站点（chat/Home）无需改动。
 */

import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import {
  Outlet,
  useLocation,
  useNavigate,
  useOutletContext,
  useParams,
  useSearchParams,
} from "react-router-dom";
import { useTranslation } from "react-i18next";
import { FolderOpen } from "lucide-react";
import { api } from "../../api/http";
import type {
  AgentInfo,
  MessageRecord,
  SwarmEvent,
  Workspace,
} from "../../api/types";
import { AgentDrawer } from "../../components/agent/AgentDrawer";
import { CreateWizard } from "../../components/workspace/CreateWizard";
import { ErrorBoundary } from "../../components/ErrorBoundary";
import { Welcome } from "../../components/Welcome";
import { useSwarmFeed } from "../../hooks/useSwarmFeed";
import { accentToCssVar, splitWorkspacePath } from "../../lib/workspace";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { WorkspaceSummary } from "./types";
import { WorkspaceList } from "./WorkspaceSidebar";
import { WorkspaceToolbar, ViewTransition, buildTabs } from "./WorkspaceToolbar";

// Re-exported so existing import sites keep working unchanged (chat/Home.tsx
// imports `WorkspaceList` + `WorkspaceSummary` from here; the child views below
// import `useWorkspaceContext`).
export type { WorkspaceSummary } from "./types";
export { WorkspaceList } from "./WorkspaceSidebar";

// ── Outlet context ─────────────────────────────────────────────────────

/** Threaded down to children via <Outlet context={...}/>. Anything a child
 *  view needs that the Shell already computed lives here so we don't run
 *  redundant fetches / subscriptions. */
export interface ShellOutletContext {
  workspace: WorkspaceSummary;
  /** Alive agents in the active workspace (= workspace.members alias). */
  activeMembers: AgentInfo[];
  /** Every alive agent across all workspaces — composer needs it to
   *  resolve cross-workspace mentions ("planner is responding…"). */
  allAliveAgents: AgentInfo[];
  /** Historical id set of agents that ever lived in this workspace
   *  (alive + killed). MessagesPanel filters by it so each workspace
   *  is a self-contained room. */
  workspaceAgentIds: string[];
  /** Latest swarm message event, or null. Child re-broadcasts. */
  liveMessage: MessageRecord | null;
  /** Latest message_read event, or null. */
  liveRead: { ids: number[]; to_agent: string; at: number } | null;
  /** Unread tally, already filtered to this workspace's senders. */
  unreadByFrom: Record<string, number>;
  /** Click → bump this counter, MessagesPanel scrolls to first unread. */
  jumpUnreadTick: number;
  /** Open the right-side AgentDrawer (writes ?agent=<id> into URL). */
  openAgent: (agentId: string) => void;
  /** Imperative refresh handle child views can call after mutations
   *  (e.g. wake-agent button → listAgents() to update spinner state). */
  refreshAgents: () => void;
}

/** Convenience hook so child views don't import the context object. */
export function useWorkspaceContext(): ShellOutletContext {
  return useOutletContext<ShellOutletContext>();
}

// ── Shell ──────────────────────────────────────────────────────────────

export default function WorkspaceShell() {
  const { t } = useTranslation();
  const { wsId } = useParams<{ wsId: string }>();
  const navigate = useNavigate();
  const location = useLocation();
  const [searchParams, setSearchParams] = useSearchParams();

  // Right-side AgentDrawer state lives in URL (?agent=<id>) so the user
  // can deep-link / refresh. Shell owns it so any view can open it.
  const drawerAgentId = searchParams.get("agent");
  const openAgent = useCallback(
    (id: string) => {
      setSearchParams((prev) => {
        const next = new URLSearchParams(prev);
        next.set("agent", id);
        return next;
      });
    },
    [setSearchParams],
  );
  const closeAgent = useCallback(() => {
    setSearchParams((prev) => {
      const next = new URLSearchParams(prev);
      next.delete("agent");
      return next;
    });
  }, [setSearchParams]);

  // CreateWizard opens from sidebar + ⌘K (window event).
  const [wizardOpen, setWizardOpen] = useState(false);
  useEffect(() => {
    const onOpen = () => setWizardOpen(true);
    window.addEventListener("flockmux:open-wizard", onOpen as EventListener);
    return () =>
      window.removeEventListener("flockmux:open-wizard", onOpen as EventListener);
  }, []);

  // ── Shared state (was per-route before, now per-Shell) ──────────────
  const [agents, setAgents] = useState<AgentInfo[]>([]);
  const [workspaceRows, setWorkspaceRows] = useState<Workspace[]>([]);
  const [liveMessage, setLiveMessage] = useState<MessageRecord | null>(null);
  const [liveRead, setLiveRead] = useState<
    { ids: number[]; to_agent: string; at: number } | null
  >(null);
  const [unreadByFrom, setUnreadByFrom] = useState<Record<string, number>>({});
  const [jumpUnreadTick, setJumpUnreadTick] = useState(0);
  const idToFromRef = useRef<Map<number, string>>(new Map());

  const refreshWorkspaces = useCallback(async () => {
    try {
      const items = await api.listWorkspaces();
      setWorkspaceRows(items);
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn("listWorkspaces failed", err);
    }
  }, []);

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
        if (m.read_at === null && m.to_agent === "user") {
          counts[m.from_agent] = (counts[m.from_agent] ?? 0) + 1;
        }
      }
      idToFromRef.current = ids;
      setUnreadByFrom(counts);
    } catch {
      /* best-effort */
    }
  }, []);

  useEffect(() => {
    refreshAgents();
    recomputeUnread();
    refreshWorkspaces();
  }, [refreshAgents, recomputeUnread, refreshWorkspaces]);

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

  useSwarmFeed({
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
          if (ev.to_agent === "user") {
            setUnreadByFrom((prev) => ({
              ...prev,
              [ev.from_agent]: (prev[ev.from_agent] ?? 0) + 1,
            }));
          }
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
        case "blackboard_changed":
          // workspace name / accent now live in the `workspaces` table,
          // not the blackboard, so we don't react to blackboard events
          // for that any more. Member-count changes are picked up via
          // `agent_state` → scheduleRefresh → refreshAgents → recompute.
          break;
      }
    },
    onReconnect: () => {
      scheduleRefresh();
      recomputeUnread();
      refreshWorkspaces();
    },
  });

  // ── Workspaces (server-side, alive only) ────────────────────────────
  // Source of truth: GET /api/workspaces (deleted_at IS NULL only).
  // Agents are grouped onto these via `agent.workspace_id`. The old
  // "group by cwd path" trick is gone — that was the bug.
  const workspaces = useMemo<WorkspaceSummary[]>(() => {
    const aliveByWsId = new Map<string, AgentInfo[]>();
    for (const a of agents) {
      if (a.killed_at != null || a.shim_exit != null) continue;
      if (!a.workspace_id) continue;
      const arr = aliveByWsId.get(a.workspace_id) ?? [];
      arr.push(a);
      aliveByWsId.set(a.workspace_id, arr);
    }
    return workspaceRows.map<WorkspaceSummary>((w) => {
      const { parent } = splitWorkspacePath(w.cwd);
      return {
        id: w.slug,
        workspaceId: w.id,
        path: w.cwd,
        name: w.name,
        parent,
        accentColor: accentToCssVar(w.accent),
        members: aliveByWsId.get(w.id) ?? [],
        roots: w.roots ?? [],
      };
    });
  }, [workspaceRows, agents]);

  const activeWs = useMemo(
    () => workspaces.find((w) => w.id === wsId) ?? null,
    [workspaces, wsId],
  );

  // ── Per-workspace derivations passed down via OutletContext ─────────
  const allAliveAgents = useMemo(
    () => agents.filter((a) => a.killed_at == null && a.shim_exit == null),
    [agents],
  );

  const workspaceAgentIds = useMemo(() => {
    if (!activeWs) return [];
    return agents
      .filter((a) => a.workspace_id === activeWs.workspaceId)
      .map((a) => a.agent_id);
  }, [agents, activeWs]);

  const activeWorkspaceUnread = useMemo(() => {
    if (!activeWs) return {} as Record<string, number>;
    const wsSet = new Set(workspaceAgentIds);
    return Object.fromEntries(
      Object.entries(unreadByFrom).filter(([from]) => wsSet.has(from)),
    );
  }, [unreadByFrom, activeWs, workspaceAgentIds]);
  const totalUnread = Object.values(activeWorkspaceUnread).reduce(
    (a, b) => a + b,
    0,
  );

  // ── Soft-delete a workspace ────────────────────────────────────────
  const handleDeleteWorkspace = useCallback(
    async (workspaceId: string) => {
      // Kill any live agents belonging to this workspace before deleting
      // the row, otherwise their PTYs survive and keep burning tokens
      // with no UI handle to address them. Per-agent failure is logged
      // but doesn't abort the batch (a half-dead PTY shouldn't block
      // the user from removing the workspace).
      try {
        const all = await api.listAgents();
        const live = all.filter(
          (a) =>
            a.workspace_id === workspaceId &&
            a.killed_at == null &&
            a.shim_exit == null,
        );
        await Promise.all(
          live.map((a) =>
            api.killAgent(a.agent_id).catch((e) => {
              // eslint-disable-next-line no-console
              console.warn("killAgent failed", a.agent_id, e);
            }),
          ),
        );
      } catch (err) {
        // eslint-disable-next-line no-console
        console.warn("listAgents before delete failed", err);
      }
      try {
        await api.deleteWorkspace(workspaceId);
      } catch (err) {
        // eslint-disable-next-line no-console
        console.warn("deleteWorkspace failed", err);
        return;
      }
      // Optimistically drop it from local state — next listWorkspaces
      // refresh would catch it anyway but UI shouldn't lag a roundtrip.
      const remaining = workspaceRows.filter((w) => w.id !== workspaceId);
      setWorkspaceRows(remaining);
      // If we just deleted the active workspace, navigate to the first
      // remaining one or back to /chat splash.
      if (activeWs?.workspaceId === workspaceId) {
        const next = remaining[0];
        navigate(next ? `/chat/${next.slug}` : "/chat", { replace: true });
      }
    },
    [workspaceRows, activeWs, navigate],
  );

  // ── ⌘1-4 global shortcut ───────────────────────────────────────────
  useEffect(() => {
    if (!activeWs) return;
    const tabs = buildTabs(activeWs.id);
    const onKey = (e: KeyboardEvent) => {
      if (!(e.metaKey || e.ctrlKey)) return;
      if (e.target instanceof HTMLElement) {
        const tag = e.target.tagName;
        // 别和 IME / 表单组合键冲突 — 输入框里 ⌘1 仍走原生 (浏览器切 tab)。
        if (tag === "INPUT" || tag === "TEXTAREA" || e.target.isContentEditable) {
          return;
        }
      }
      const n = Number.parseInt(e.key, 10);
      if (!Number.isInteger(n) || n < 1 || n > tabs.length) return;
      e.preventDefault();
      navigate(tabs[n - 1].to);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [activeWs, navigate]);

  // ── Redirect a stale / unknown wsId to the first workspace ──────────
  // MUST be an effect, not render-phase. Calling navigate() while rendering
  // triggers React's "Cannot update a component (BrowserRouter) while
  // rendering a different component (WorkspaceShell)" warning and is unsafe
  // under React 18 concurrent rendering. This fires when a bookmark / refresh
  // points at a workspace that was since deleted while others still exist.
  useEffect(() => {
    if (
      !activeWs &&
      workspaces.length > 0 &&
      wsId &&
      !workspaces.some((w) => w.id === wsId)
    ) {
      navigate(`/chat/${workspaces[0].id}`, { replace: true });
    }
  }, [activeWs, workspaces, wsId, navigate]);

  // ── Render ─────────────────────────────────────────────────────────
  if (!activeWs) {
    // wsId 在 URL 但 listAgents 还没回 / 已经 evicted。渲染 sidebar +
    // "找不到工作空间" 提示；真正的跳转由上面的 useEffect 负责（render
    // 阶段不能 navigate）。
    return (
      <TooltipProvider delayDuration={300}>
        <div className="flex h-full min-h-0">
          <WorkspaceList
            workspaces={workspaces}
            activeId={wsId ?? null}
            onOpenWizard={() => setWizardOpen(true)}
            onDelete={handleDeleteWorkspace}
            onRootsChanged={refreshWorkspaces}
          />
          {workspaces.length === 0 ? (
            // 完全空：展示 Welcome 屏，跟 /chat 主入口体验一致。
            <Welcome onCreateWorkspace={() => setWizardOpen(true)} />
          ) : (
            // 有别的 ws 但 URL 指的这个不存在 — 给个安静提示就行，重定向
            // 已经在 useEffect 里发车。
            <section className="flex min-w-0 flex-1 flex-col items-center justify-center gap-3 bg-surface-primary text-foreground-tertiary">
              <FolderOpen className="size-10 opacity-40" />
              <p className="font-caption text-sm">
                {t("chat.selectWorkspace")}
              </p>
            </section>
          )}
          <CreateWizard
            open={wizardOpen}
            onClose={() => setWizardOpen(false)}
            onCreated={(ws) => {
              refreshAgents();
              // Await the workspace refetch BEFORE navigating — otherwise the
              // new slug isn't in `workspaces` yet and the not-found redirect
              // effect bounces us straight back to the previous workspace.
              void refreshWorkspaces().then(() => {
                if (ws) navigate(`/chat/${ws.slug}`);
              });
            }}
          />
        </div>
      </TooltipProvider>
    );
  }

  const ctx: ShellOutletContext = {
    workspace: activeWs,
    activeMembers: activeWs.members,
    allAliveAgents,
    workspaceAgentIds,
    liveMessage,
    liveRead,
    unreadByFrom: activeWorkspaceUnread,
    jumpUnreadTick,
    openAgent,
    refreshAgents,
  };

  return (
    <TooltipProvider delayDuration={300}>
      <div className="flex h-full min-h-0">
        <WorkspaceList
          workspaces={workspaces}
          activeId={activeWs.id}
          onOpenWizard={() => setWizardOpen(true)}
          onDelete={handleDeleteWorkspace}
          onRootsChanged={refreshWorkspaces}
        />
        <section className="flex min-w-0 flex-1 flex-col bg-surface-primary">
          <WorkspaceToolbar
            workspace={activeWs}
            agentCount={activeWs.members.length}
            totalUnread={totalUnread}
            onJumpUnread={() => setJumpUnreadTick((v) => v + 1)}
          />
          <ViewTransition>
            {/* View-level boundary: a crash in one tab (malformed ledger
                markdown, ReactFlow state, …) shows a contained fallback while
                the sidebar + tab bar stay intact. Keyed by wsId+view so a
                tab switch clears a held error. */}
            <ErrorBoundary resetKey={`${activeWs.id}:${location.pathname}`}>
              <Outlet context={ctx} />
            </ErrorBoundary>
          </ViewTransition>
        </section>

        {drawerAgentId && (
          <AgentDrawer agentId={drawerAgentId} onClose={closeAgent} />
        )}
        <CreateWizard
          open={wizardOpen}
          onClose={() => setWizardOpen(false)}
          onCreated={(ws) => {
            refreshAgents();
            // Await the workspace refetch BEFORE navigating — otherwise the
            // new slug isn't in `workspaces` yet and the not-found redirect
            // effect bounces us straight back to the previous workspace.
            void refreshWorkspaces().then(() => {
              if (ws) navigate(`/chat/${ws.slug}`);
            });
          }}
        />
      </div>
    </TooltipProvider>
  );
}
