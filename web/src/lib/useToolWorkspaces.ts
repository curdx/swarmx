/**
 * Shared workspace state for the global tool pages (files / terminal / tasks /
 * usage). Fetches the workspace list once and seeds the selection from the
 * last-active workspace (see `activeWorkspace`) so every tool page defaults to
 * the workspace you were just working in. Returns the list + the selected id +
 * a setter; the page decides whether "all workspaces" ("") is meaningful.
 */
import { useEffect, useState } from "react";
import { api } from "@/api/http";
import type { Workspace } from "@/api/types";
import { getActiveWorkspaceId, pickDefaultWorkspace } from "@/lib/activeWorkspace";

export function useToolWorkspaces() {
  const [workspaces, setWorkspaces] = useState<Workspace[]>([]);
  const [wsId, setWsId] = useState<string>("");
  const [ready, setReady] = useState(false);

  useEffect(() => {
    let alive = true;
    api
      .listWorkspaces()
      .then((ws) => {
        if (!alive) return;
        setWorkspaces(ws);
        setWsId((cur) => (cur ? cur : pickDefaultWorkspace(ws, getActiveWorkspaceId())));
      })
      .catch(() => {})
      .finally(() => alive && setReady(true));
    return () => {
      alive = false;
    };
  }, []);

  return { workspaces, wsId, setWsId, ready };
}
