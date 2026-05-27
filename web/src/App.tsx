/**
 * Router root.
 *
 * Layout-level routes:
 *   /chat                              ChatHome (no workspace selected)
 *   /chat/:wsId                        WorkspaceShell — common chrome
 *     index                              ChatView
 *     dag                                DagView
 *     replays                            ReplaysView
 *     context                            ContextView
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
import ContextView from "./routes/workspace/views/Context";
import ReplayPlayer from "./routes/replays/player";
import SettingsRoute from "./routes/settings";
import NotificationsRoute from "./routes/notifications";

export default function App() {
  return (
    <BrowserRouter>
      <Routes>
        <Route element={<AppShell />}>
          <Route index element={<Navigate to="/chat" replace />} />
          <Route path="/chat" element={<ChatHome />} />
          <Route path="/chat/:wsId" element={<WorkspaceShell />}>
            <Route index element={<ChatView />} />
            <Route path="dag" element={<DagView />} />
            <Route path="replays" element={<ReplaysView />} />
            <Route path="context" element={<ContextView />} />
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
