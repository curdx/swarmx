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

export default function App() {
  return (
    <BrowserRouter>
      <Routes>
        <Route element={<AppShell />}>
          <Route index element={<Navigate to="/chat" replace />} />
          <Route path="/chat" element={<ChatRoute />} />
          <Route path="/chat/:workspaceId" element={<ChatRoute />} />
          <Route path="/dag" element={<RoutePlaceholder name="协作图" pencilId="Z23h6o" />} />
          <Route path="/dag/:workspaceId" element={<RoutePlaceholder name="协作图" pencilId="Z23h6o" />} />
          <Route path="/replays" element={<ReplaysIndex />} />
          <Route path="/context" element={<ContextRoute />} />
          <Route path="/inbox" element={<RoutePlaceholder name="审批" pencilId="NUCBp" />} />
          <Route path="/notifications" element={<RoutePlaceholder name="通知" pencilId="COJDW" />} />
          <Route path="/settings" element={<RoutePlaceholder name="设置" pencilId="nJqkA" />} />
          <Route path="/settings/:section" element={<RoutePlaceholder name="设置" pencilId="nJqkA" />} />
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
