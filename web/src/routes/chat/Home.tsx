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
import { Link, useNavigate } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { FolderOpen, Sparkles } from "lucide-react";
import { api } from "../../api/http";
import type { AgentInfo, SwarmEvent } from "../../api/types";
import { useSwarmFeed } from "../../hooks/useSwarmFeed";
import {
  accentToCssVar,
  WORKSPACE_ACCENT_KEY_PREFIX,
  workspaceSlug,
} from "../../lib/workspace";
import { CreateWizard } from "../../components/workspace/CreateWizard";
import { TooltipProvider } from "@/components/ui/tooltip";
import { WorkspaceList, type WorkspaceSummary } from "../workspace/Shell";

const WORKSPACE_NAME_KEY_PREFIX = "workspace.name.";

function splitWorkspacePath(path: string): { name: string; parent: string } {
  if (!path || path === "(no workspace)") return { name: path || "", parent: "" };
  const trimmed = path.replace(/[\\/]+$/, "");
  const idx = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  if (idx < 0) return { name: trimmed, parent: "" };
  return { name: trimmed.slice(idx + 1) || trimmed, parent: trimmed.slice(0, idx) };
}

export default function ChatHome() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const [agents, setAgents] = useState<AgentInfo[]>([]);
  const [workspaceNames, setWorkspaceNames] = useState<Record<string, string>>({});
  const [workspaceAccents, setWorkspaceAccents] = useState<Record<string, string>>({});
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

  const refreshNames = async () => {
    try {
      const entries = await api.listBlackboard();
      const nameEntries = entries.filter((e) =>
        e.path.startsWith(WORKSPACE_NAME_KEY_PREFIX),
      );
      const accentEntries = entries.filter((e) =>
        e.path.startsWith(WORKSPACE_ACCENT_KEY_PREFIX),
      );
      const [namePairs, accentPairs] = await Promise.all([
        Promise.all(
          nameEntries.map(async (e) => {
            const slug = e.path.slice(WORKSPACE_NAME_KEY_PREFIX.length);
            try {
              const snap = await api.readBlackboard(e.path);
              return [slug, snap.content] as const;
            } catch {
              return [slug, ""] as const;
            }
          }),
        ),
        Promise.all(
          accentEntries.map(async (e) => {
            const slug = e.path.slice(WORKSPACE_ACCENT_KEY_PREFIX.length);
            try {
              const snap = await api.readBlackboard(e.path);
              return [slug, snap.content] as const;
            } catch {
              return [slug, ""] as const;
            }
          }),
        ),
      ]);
      setWorkspaceNames(Object.fromEntries(namePairs.filter(([, v]) => v)));
      setWorkspaceAccents(Object.fromEntries(accentPairs.filter(([, v]) => v)));
    } catch {
      /* best-effort */
    }
  };

  useEffect(() => {
    refreshAgents();
    refreshNames();
  }, []);

  useSwarmFeed({
    onEvent: (ev: SwarmEvent) => {
      if (ev.type === "agent_state") refreshAgents();
      if (
        ev.type === "blackboard_changed" &&
        (ev.path.startsWith(WORKSPACE_NAME_KEY_PREFIX) ||
          ev.path.startsWith(WORKSPACE_ACCENT_KEY_PREFIX))
      ) {
        refreshNames();
      }
    },
    onReconnect: () => {
      refreshAgents();
      refreshNames();
    },
  });

  const workspaces = useMemo<WorkspaceSummary[]>(() => {
    const live = agents.filter((a) => a.killed_at == null && a.shim_exit == null);
    const byWs = new Map<string, AgentInfo[]>();
    for (const a of live) {
      const key = a.workspace || "(no workspace)";
      if (!byWs.has(key)) byWs.set(key, []);
      byWs.get(key)!.push(a);
    }
    return Array.from(byWs.entries()).map(([path, members]) => {
      const { name: basename, parent } = splitWorkspacePath(path);
      const slug = workspaceSlug(path);
      const userName = workspaceNames[slug];
      const accentColor = accentToCssVar(workspaceAccents[slug]);
      return {
        path,
        members,
        name: userName || basename,
        parent,
        accentColor,
        id: path.slice(-8) || "default",
      };
    });
  }, [agents, workspaceNames, workspaceAccents]);

  // Auto-redirect once a workspace is available — landing here only makes
  // sense as a transient splash. If multiple are alive we still send the
  // user to the first (the sidebar lets them switch instantly).
  useEffect(() => {
    if (workspaces.length > 0) {
      navigate(`/chat/${workspaces[0].id}`, { replace: true });
    }
  }, [workspaces, navigate]);

  return (
    <TooltipProvider delayDuration={300}>
      <div className="flex h-full min-h-0">
        <WorkspaceList
          workspaces={workspaces}
          activeId={null}
          onOpenWizard={() => setWizardOpen(true)}
        />
        <section className="flex min-w-0 flex-1 flex-col items-center justify-center gap-3 bg-surface-primary text-foreground-tertiary">
          <FolderOpen className="size-12 opacity-30" />
          <p className="font-caption text-sm">
            {workspaces.length === 0
              ? t("chat.emptyStateHint")
              : t("chat.selectWorkspace")}
          </p>
          {workspaces.length === 0 && (
            <Link
              to="#"
              onClick={(e) => {
                e.preventDefault();
                setWizardOpen(true);
              }}
              className="flex items-center gap-1.5 rounded-md bg-accent-primary px-3 py-1.5 text-xs text-foreground-on-accent hover:bg-accent-primary-deep"
            >
              <Sparkles className="size-3.5" />
              {t("chat.emptyStateTitle")}
            </Link>
          )}
        </section>
        <CreateWizard
          open={wizardOpen}
          onClose={() => setWizardOpen(false)}
          onCreated={refreshAgents}
        />
      </div>
    </TooltipProvider>
  );
}
