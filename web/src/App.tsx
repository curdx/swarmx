/**
 * Router root.
 *
 * Layout-level routes:
 *   /chat                              ChatHome (no workspace selected)
 *   /chat/:wsId                        WorkspaceShell — common chrome
 *     index                              ChatView
 *     dag                                DagView
 *     ledger                             LedgerView (Magentic-One 双台账)
 *     replays                            ReplaysView
 *
 * Fullscreen / outside-shell routes:
 *   /chat/:wsId/replays/:recId         ReplayPlayer (dark, chromeless,
 *                                        Esc returns to /chat/:wsId/replays)
 *   /debug                             DebugRoute (legacy M2 dashboard)
 *
 * Why nested: /chat/:wsId/{dag,replays,context} all share the same
 * workspace chrome (sidebar + channel header + tab bar + swarm
 * subscription). Tab switches re-render only the <Outlet/>; nothing
 * else unmounts. This is the difference between "I switched view" and
 * "I jumped to another page" — see WorkspaceShell module comment.
 */

import { lazy, Suspense, type ReactElement } from "react";
import { BrowserRouter, Navigate, Route, Routes } from "react-router-dom";
import { AppShell } from "./layouts/AppShell";
import ChatHome from "./routes/chat/Home";
import WorkspaceShell from "./routes/workspace/Shell";
import ChatView from "./routes/workspace/views/Chat";

// Route-level code-splitting (R2-006): the heavy, not-initial views — DAG
// (dagre + @xyflow/react), Replays / player (asciinema-player), settings,
// notifications, mcp, the debug dashboard — load their own chunk on demand
// instead of bloating the initial chat bundle (~478KB gz was one monolith).
// Chat + the shell stay eager so first paint is unaffected. Each module has a
// default export, which React.lazy requires.
const DagView = lazy(() => import("./routes/workspace/views/Dag"));
const LedgerView = lazy(() => import("./routes/workspace/views/Ledger"));
const ReplaysView = lazy(() => import("./routes/workspace/views/Replays"));
const ReplayPlayer = lazy(() => import("./routes/replays/player"));
const SettingsRoute = lazy(() => import("./routes/settings"));
const NotificationsRoute = lazy(() => import("./routes/notifications"));
const McpRoute = lazy(() => import("./routes/mcp"));
const UsageRoute = lazy(() => import("./routes/usage"));
const TasksRoute = lazy(() => import("./routes/tasks"));
const FilesRoute = lazy(() => import("./routes/files"));
const TerminalRoute = lazy(() => import("./routes/terminal"));
const DebugRoute = lazy(() => import("./routes/debug"));

/** Suspense wrapper for a lazily-loaded route element. Keeps the surrounding
 *  chrome (shell / sidebar / tab bar) mounted while the view's chunk loads. */
function lazyView(el: ReactElement) {
  return (
    <Suspense
      fallback={
        <div className="flex h-full min-h-0 flex-1 items-center justify-center">
          <span className="font-caption text-xs text-foreground-tertiary">
            加载中…
          </span>
        </div>
      }
    >
      {el}
    </Suspense>
  );
}

/** The view tabs nested under a workspace shell. Reused for both the
 *  main-direction URL (`/chat/:wsId/*`) and an explicit direction
 *  (`/chat/:wsId/t/:threadSlug/*`). A fresh fragment per call so the two
 *  mount points don't share element instances. */
function workspaceViewRoutes() {
  return (
    <>
      <Route index element={<ChatView />} />
      <Route path="dag" element={lazyView(<DagView />)} />
      <Route path="ledger" element={lazyView(<LedgerView />)} />
      <Route path="replays" element={lazyView(<ReplaysView />)} />
      {/* `context` 路径保留(老书签) — redirect 到新 ledger 视图。 */}
      <Route path="context" element={<Navigate to="../ledger" replace />} />
    </>
  );
}

export default function App() {
  return (
    <BrowserRouter>
      <Routes>
        <Route element={<AppShell />}>
          <Route index element={<Navigate to="/chat" replace />} />
          <Route path="/chat" element={<ChatHome />} />
          <Route path="/chat/:wsId" element={<WorkspaceShell />}>
            {workspaceViewRoutes()}
            {/* Explicit direction (thread). Element-less group: only adds the
                `t/:threadSlug` path segment — WorkspaceShell stays the single
                layout and resolves the active direction from useParams. */}
            <Route path="t/:threadSlug">{workspaceViewRoutes()}</Route>
          </Route>
          <Route path="/mcp" element={lazyView(<McpRoute />)} />
          <Route path="/usage" element={lazyView(<UsageRoute />)} />
          <Route path="/tasks" element={lazyView(<TasksRoute />)} />
          <Route path="/files" element={lazyView(<FilesRoute />)} />
          <Route path="/terminal" element={lazyView(<TerminalRoute />)} />
          <Route path="/notifications" element={lazyView(<NotificationsRoute />)} />
          <Route path="/settings" element={lazyView(<SettingsRoute />)} />
          <Route path="/settings/:section" element={lazyView(<SettingsRoute />)} />
        </Route>
        <Route path="/debug" element={lazyView(<DebugRoute />)} />
        {/* Fullscreen surfaces escape AppShell. New canonical path puts the
            player under its workspace so Esc / breadcrumbs land back on
            the right Replays tab. */}
        <Route path="/chat/:wsId/replays/:recId" element={lazyView(<ReplayPlayer />)} />
        <Route path="*" element={<Navigate to="/chat" replace />} />
      </Routes>
    </BrowserRouter>
  );
}
