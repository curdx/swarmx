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
  // M6: distinguish "loaded, you have no workspaces" from "the fetch FAILED".
  // Was: the error was swallowed (.catch(()=>{})) and `ready` flipped true in
  // .finally regardless, so a failed load rendered an empty-but-ready list that
  // reads as "你没有工作区". Now a failure is surfaced so pages can say so.
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    api
      .listWorkspaces()
      .then((ws) => {
        if (!alive) return;
        setWorkspaces(ws);
        setWsId((cur) => (cur ? cur : pickDefaultWorkspace(ws, getActiveWorkspaceId())));
        setError(null);
        setReady(true);
      })
      .catch((e) => {
        if (!alive) return;
        setError((e as Error)?.message || "加载工作区失败");
        setReady(true); // resolved (with an error) — pages branch on `error`
      });
    return () => {
      alive = false;
    };
  }, []);

  return { workspaces, wsId, setWsId, ready, error };
}
