/**
 * /chat (no workspace selected) — Pencil ChatHome.
 *
 * Shows the workspace list on the left + a center pane telling the user
 * to pick one. Auto-redirects to the first workspace once any agent is
 * alive — most users land here right after spawn and want the chat
 * immediately, not a "pick something" splash. The splash only persists
 * when nothing's alive (or the wizard hasn't been run yet).
 */

import { useEffect, useMemo, useState } from "react";
import { useNavigate } from "react-router-dom";
import { api } from "../../api/http";
import type { AgentInfo, SwarmEvent, Workspace } from "../../api/types";
import { useSwarmFeed } from "../../hooks/useSwarmFeed";
import { accentToCssVar } from "../../lib/workspace";
import { CreateWizard } from "../../components/workspace/CreateWizard";
import { Welcome } from "../../components/Welcome";
import { TooltipProvider } from "@/components/ui/tooltip";
import { WorkspaceList, type WorkspaceSummary } from "../workspace/Shell";

function splitWorkspacePath(path: string): { name: string; parent: string } {
  if (!path || path === "(no workspace)") return { name: path || "", parent: "" };
  const trimmed = path.replace(/[\\/]+$/, "");
  const idx = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  if (idx < 0) return { name: trimmed, parent: "" };
  return { name: trimmed.slice(idx + 1) || trimmed, parent: trimmed.slice(0, idx) };
}

export default function ChatHome() {
  const navigate = useNavigate();
  const [agents, setAgents] = useState<AgentInfo[]>([]);
  const [workspaceRows, setWorkspaceRows] = useState<Workspace[]>([]);
  const [wizardOpen, setWizardOpen] = useState(false);

  // CommandPalette → 新建 workspace 走 window event。
  useEffect(() => {
    const onOpen = () => setWizardOpen(true);
    window.addEventListener("flockmux:open-wizard", onOpen as EventListener);
    return () =>
      window.removeEventListener("flockmux:open-wizard", onOpen as EventListener);
  }, []);

  const refreshAgents = async () => {
    try {
      setAgents(await api.listAgents());
    } catch {
      /* best-effort */
    }
  };

  const refreshWorkspaces = async () => {
    try {
      setWorkspaceRows(await api.listWorkspaces());
    } catch {
      /* best-effort */
    }
  };

  useEffect(() => {
    refreshAgents();
    refreshWorkspaces();
  }, []);

  useSwarmFeed({
    onEvent: (ev: SwarmEvent) => {
      if (ev.type === "agent_state") refreshAgents();
      // workspace name / accent / membership now derived from the
      // workspaces table + agents.workspace_id, so we don't need to
      // listen for blackboard events here any more.
    },
    onReconnect: () => {
      refreshAgents();
      refreshWorkspaces();
    },
  });

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
        threads: w.threads ?? [],
      };
    });
  }, [workspaceRows, agents]);

  // Auto-redirect once a workspace is available — landing here only makes
  // sense as a transient splash. If multiple are alive we still send the
  // user to the first (the sidebar lets them switch instantly).
  useEffect(() => {
    if (workspaces.length > 0) {
      navigate(`/chat/${workspaces[0].id}`, { replace: true });
    }
  }, [workspaces, navigate]);

  const handleDeleteWorkspace = async (workspaceId: string) => {
    // Kill any live agents that belonged to this workspace BEFORE we
    // soft-delete the row. Otherwise the agents linger as orphan PTYs
    // attached to a workspace that no longer exists in the UI, eating
    // tokens with no way to address them. Per-agent failures don't
    // abort the batch — a half-dead PTY shouldn't block deletion.
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
    setWorkspaceRows((prev) => prev.filter((w) => w.id !== workspaceId));
  };

  return (
    <TooltipProvider delayDuration={300}>
      <div className="flex h-full min-h-0">
        <WorkspaceList
          workspaces={workspaces}
          activeId={null}
          onOpenWizard={() => setWizardOpen(true)}
          onDelete={handleDeleteWorkspace}
          onRootsChanged={refreshWorkspaces}
        />
        {/* 主战场就一个 — Welcome 屏。删了之前左 sidebar 大卡片 + 中
         *  间又一个 "新建工作空间" 按钮的双 CTA，sidebar 那边 empty 现
         *  在只画一行小提示 (WorkspaceList 内部已经做了)，主屏负责讲
         *  清楚 flockmux 是干啥的 + 主 CTA。 */}
        <Welcome onCreateWorkspace={() => setWizardOpen(true)} />
        <CreateWizard
          open={wizardOpen}
          onClose={() => setWizardOpen(false)}
          onCreated={(ws) => {
            refreshAgents();
            // Await the refetch before navigating so the destination Shell
            // already has the new slug in its workspace list (otherwise its
            // not-found redirect bounces back to the first workspace).
            void refreshWorkspaces().then(() => {
              if (ws) navigate(`/chat/${ws.slug}`);
            });
          }}
        />
      </div>
    </TooltipProvider>
  );
}
