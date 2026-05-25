/**
 * Router root. Each path corresponds to one top-level frame in
 * untitled.pen. /debug hosts the legacy M2 dashboard outside AppShell
 * (its dark chrome doesn't mix with the new product surfaces).
 */

import { BrowserRouter, Navigate, Route, Routes } from "react-router-dom";
import { AppShell } from "./layouts/AppShell";
import { RoutePlaceholder } from "./components/RoutePlaceholder";
import DebugRoute from "./routes/debug";
import ChatRoute from "./routes/chat";
import ReplaysIndex from "./routes/replays/index";
import ReplayPlayer from "./routes/replays/player";
import ContextRoute from "./routes/context";
import DagRoute from "./routes/dag";
import SettingsRoute from "./routes/settings";
import NotificationsRoute from "./routes/notifications";

export default function App() {
  return (
    <BrowserRouter>
      <Routes>
        <Route element={<AppShell />}>
          <Route index element={<Navigate to="/chat" replace />} />
          <Route path="/chat" element={<ChatRoute />} />
          <Route path="/chat/:workspaceId" element={<ChatRoute />} />
          <Route path="/dag" element={<DagRoute />} />
          <Route path="/replays" element={<ReplaysIndex />} />
          <Route path="/context" element={<ContextRoute />} />
          <Route path="/inbox" element={<RoutePlaceholder name="审批" pencilId="NUCBp" />} />
          <Route path="/notifications" element={<NotificationsRoute />} />
          <Route path="/settings" element={<SettingsRoute />} />
          <Route path="/settings/:section" element={<SettingsRoute />} />
        </Route>
        <Route path="/debug" element={<DebugRoute />} />
        {/* Fullscreen surfaces escape AppShell — the bright TitleBar
            clashes with the dark player chrome (Pencil v1radc is full
            dark all the way up). */}
        <Route path="/replays/:id" element={<ReplayPlayer />} />
        <Route path="*" element={<Navigate to="/chat" replace />} />
      </Routes>
    </BrowserRouter>
  );
}
