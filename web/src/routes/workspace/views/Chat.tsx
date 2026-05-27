/**
 * Chat view — the messages tab inside WorkspaceShell.
 *
 * Reduced from the previous chat.tsx route to just two regions:
 *   - center: MessagesPanel (composer + bubbles)
 *   - right:  members sidebar
 *
 * Workspace state, sidebar, channel header, tab bar all live in the
 * parent Shell. We pull what we need (members, unread, live events,
 * composer override) via useWorkspaceContext().
 */

import { useTranslation } from "react-i18next";
import { api } from "../../../api/http";
import type { AgentInfo } from "../../../api/types";
import { MessagesPanel } from "../../../components/MessagesPanel";
import { Avatar, AvatarFallback } from "@/components/ui/avatar";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { Users, Zap } from "lucide-react";
import { cn } from "@/lib/cn";
import { roleColorClass as roleColor } from "@/lib/agent";
import { useWorkspaceContext } from "../Shell";

function statusDot(a: AgentInfo, t: (k: string) => string) {
  if (a.killed_at) return { className: "bg-state-idle", label: t("chat.exited") };
  if (a.shim_exit != null) return { className: "bg-state-idle", label: t("chat.shimExit") };
  if (!a.shim_ready) return { className: "bg-state-wake", label: t("chat.starting") };
  return { className: "bg-state-success", label: t("chat.online") };
}

export default function ChatView() {
  const { t } = useTranslation();
  const {
    workspace,
    activeMembers,
    allAliveAgents,
    workspaceAgentIds,
    liveMessage,
    liveRead,
    unreadByFrom: activeWorkspaceUnread,
    jumpUnreadTick,
    openAgent,
    composerOverride,
    // unreadByFrom in OutletContext is workspace-filtered; the right-side
    // members list wants raw agent-id → count so it can show the small
    // red badge per row. We re-derive it by indexing into the filtered
    // map (same keys, just used differently).
  } = useWorkspaceContext();

  return (
    <div className="flex min-h-0 flex-1">
      <section className="flex min-w-0 flex-1 flex-col">
        <MessagesPanel
          liveMessage={liveMessage}
          liveRead={liveRead}
          unreadByFrom={activeWorkspaceUnread}
          activeMembers={activeMembers}
          allAliveAgents={allAliveAgents}
          workspaceAgentIds={workspaceAgentIds}
          workspaceLabel={workspace.name}
          composerOverride={composerOverride}
          jumpUnreadTick={jumpUnreadTick}
          onOpenAgent={openAgent}
        />
      </section>

      <aside className="flex w-[340px] shrink-0 flex-col border-l border-border-subtle bg-surface-elevated">
        <div className="flex h-12 shrink-0 items-center gap-2 border-b border-border-subtle px-4">
          <Users className="size-4 text-foreground-tertiary" />
          <h2 className="font-heading text-xs font-semibold uppercase tracking-wider text-foreground-tertiary">
            {t("chat.members")}
          </h2>
          <span className="ml-auto font-caption text-xs text-foreground-tertiary">
            {activeMembers.length}
          </span>
        </div>
        <div className="flex-1 overflow-y-auto px-2 py-2">
          {activeMembers.length === 0 && (
            <p className="px-3 py-2 font-caption text-xs text-foreground-tertiary">
              {t("chat.selectWorkspace")}
            </p>
          )}
          {activeMembers.map((a) => {
            const dot = statusDot(a, t);
            const unread = activeWorkspaceUnread[a.agent_id] ?? 0;
            return (
              <div
                key={a.agent_id}
                onClick={() => openAgent(a.agent_id)}
                className={cn(
                  "flex cursor-pointer items-center gap-3 rounded-md px-3 py-2 hover:bg-surface-tertiary",
                )}
              >
                <Avatar className="size-8 shrink-0" title={a.role}>
                  <AvatarFallback
                    className={cn(
                      "text-xs font-medium text-foreground-on-accent",
                      roleColor(a.role),
                    )}
                  >
                    {a.role.slice(0, 1).toUpperCase()}
                  </AvatarFallback>
                </Avatar>
                <div className="min-w-0 flex-1">
                  <div className="flex items-center gap-2">
                    <span className="truncate font-heading text-sm text-foreground-primary">
                      {a.role}
                    </span>
                    <span
                      className={cn("size-1.5 rounded-full", dot.className)}
                      title={dot.label}
                    />
                  </div>
                  <div className="truncate font-mono text-[10px] text-foreground-tertiary">
                    {a.cli} · {a.agent_id.slice(-8)}
                  </div>
                </div>
                {unread > 0 && (
                  <Badge
                    variant="destructive"
                    className="rounded-full px-1.5 py-0.5 text-[10px]"
                  >
                    {unread}
                  </Badge>
                )}
                <Tooltip>
                  <TooltipTrigger asChild>
                    <Button
                      variant="ghost"
                      size="icon"
                      className="size-7 text-foreground-tertiary hover:text-state-wake"
                      onClick={(e) => {
                        e.stopPropagation();
                        api.wakeAgent(a.agent_id).catch(() => {});
                      }}
                    >
                      <Zap className="size-4" />
                    </Button>
                  </TooltipTrigger>
                  <TooltipContent side="left">{t("chat.wake")}</TooltipContent>
                </Tooltip>
              </div>
            );
          })}
        </div>
      </aside>
    </div>
  );
}
