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
 * tab 栏 + 视图过渡搬到 WorkspaceToolbar，WorkspaceSummary 类型搬到 ./types，
 * 数据编排（agents/workspaces/unread + swarm 订阅 + 级联删除）搬到
 * useWorkspaceShellData hook。本文件只剩"布局 + 导航 + Outlet context 组装"。
 * WorkspaceList / WorkspaceSummary 在此 re-export，既有 import 站点
 * （chat/Home）无需改动。
 */

import { useCallback, useEffect, useState } from "react";
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
import type { AgentInfo, MessageRecord, ThreadInfo } from "../../api/types";
import { AgentDrawer } from "../../components/agent/AgentDrawer";
import { CreateWizard } from "../../components/workspace/CreateWizard";
import { ErrorBoundary } from "../../components/ErrorBoundary";
import { Welcome } from "../../components/Welcome";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { WorkspaceSummary } from "./types";
import { WorkspaceList } from "./WorkspaceSidebar";
import { WorkspaceToolbar, ViewTransition, buildTabs } from "./WorkspaceToolbar";
import { useWorkspaceShellData } from "./useWorkspaceShellData";

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
  /** The active direction (thread), resolved from the URL `:threadSlug`
   *  param, defaulting to the workspace's main thread. `null` only for a
   *  legacy/empty workspace with no thread rows. */
  activeThread: ThreadInfo | null;
  /** Resolved slug of the active direction — `"main"` when none. Views build
   *  blackboard keys as `{workspace.workspaceId}/{threadSlug}/…`. */
  threadSlug: string;
  /** Alive agents in the active workspace (= workspace.members alias). */
  activeMembers: AgentInfo[];
  /** Alive agents in the ACTIVE direction (subset of activeMembers). The
   *  members list + DAG render this so each direction is self-contained. */
  threadMembers: AgentInfo[];
  /** Every alive agent across all workspaces — composer needs it to
   *  resolve cross-workspace mentions ("planner is responding…"). */
  allAliveAgents: AgentInfo[];
  /** Historical id set of agents that ever lived in this workspace
   *  (alive + killed). Workspace-wide; kept for any non-thread consumers. */
  workspaceAgentIds: string[];
  /** Historical id set (alive + killed) of agents in the ACTIVE direction.
   *  MessagesPanel filters by it so each direction is a self-contained room. */
  threadAgentIds: string[];
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
  const { wsId, threadSlug: threadSlugParam } = useParams<{
    wsId: string;
    threadSlug?: string;
  }>();
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

  // jumpUnread is a pure UI signal to MessagesPanel (scroll to first unread).
  const [jumpUnreadTick, setJumpUnreadTick] = useState(0);

  // All data orchestration (agents / workspaces / unread, the single swarm
  // subscription, the cascade delete) lives in the hook — Shell stays layout.
  const {
    workspaces,
    activeWs,
    activeThread,
    activeThreadSlug,
    allAliveAgents,
    workspaceAgentIds,
    threadAgentIds,
    threadMembers,
    liveMessage,
    liveRead,
    activeWorkspaceUnread,
    totalUnread,
    refreshAgents,
    refreshWorkspaces,
    deleteWorkspace,
  } = useWorkspaceShellData(wsId, threadSlugParam);

  // deleteWorkspace performs the kill+delete+optimistic-drop and returns where
  // to navigate when the ACTIVE workspace was removed (router stays here).
  const onDeleteWorkspace = useCallback(
    async (workspaceId: string) => {
      const navTo = await deleteWorkspace(workspaceId);
      if (navTo) navigate(navTo, { replace: true });
    },
    [deleteWorkspace, navigate],
  );

  // ── ⌘1-4 global shortcut ───────────────────────────────────────────
  useEffect(() => {
    if (!activeWs) return;
    const tabs = buildTabs(activeWs.id, activeThreadSlug);
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
  }, [activeWs, activeThreadSlug, navigate]);

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
            onDelete={onDeleteWorkspace}
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
    activeThread,
    threadSlug: activeThreadSlug,
    activeMembers: activeWs.members,
    threadMembers,
    allAliveAgents,
    workspaceAgentIds,
    threadAgentIds,
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
          onDelete={onDeleteWorkspace}
          onRootsChanged={refreshWorkspaces}
        />
        <section className="flex min-w-0 flex-1 flex-col bg-surface-primary">
          <WorkspaceToolbar
            workspace={activeWs}
            threadSlug={activeThreadSlug}
            agentCount={threadMembers.length}
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
