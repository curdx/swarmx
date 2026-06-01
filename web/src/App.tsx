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

import { BrowserRouter, Navigate, Route, Routes } from "react-router-dom";
import { AppShell } from "./layouts/AppShell";
import DebugRoute from "./routes/debug";
import ChatHome from "./routes/chat/Home";
import WorkspaceShell from "./routes/workspace/Shell";
import ChatView from "./routes/workspace/views/Chat";
import DagView from "./routes/workspace/views/Dag";
import ReplaysView from "./routes/workspace/views/Replays";
import LedgerView from "./routes/workspace/views/Ledger";
import ReplayPlayer from "./routes/replays/player";
import SettingsRoute from "./routes/settings";
import NotificationsRoute from "./routes/notifications";

/** The view tabs nested under a workspace shell. Reused for both the
 *  main-direction URL (`/chat/:wsId/*`) and an explicit direction
 *  (`/chat/:wsId/t/:threadSlug/*`). A fresh fragment per call so the two
 *  mount points don't share element instances. */
function workspaceViewRoutes() {
  return (
    <>
      <Route index element={<ChatView />} />
      <Route path="dag" element={<DagView />} />
      <Route path="ledger" element={<LedgerView />} />
      <Route path="replays" element={<ReplaysView />} />
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
          <Route path="/notifications" element={<NotificationsRoute />} />
          <Route path="/settings" element={<SettingsRoute />} />
          <Route path="/settings/:section" element={<SettingsRoute />} />
        </Route>
        <Route path="/debug" element={<DebugRoute />} />
        {/* Fullscreen surfaces escape AppShell. New canonical path puts the
            player under its workspace so Esc / breadcrumbs land back on
            the right Replays tab. */}
        <Route path="/chat/:wsId/replays/:recId" element={<ReplayPlayer />} />
        <Route path="*" element={<Navigate to="/chat" replace />} />
      </Routes>
    </BrowserRouter>
  );
}
