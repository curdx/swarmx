/**
 * Workspace chrome: the per-view tab bar (chat / dag / ledger / replays) with
 * its LIVE + unread-jump actions, plus the Outlet cross-fade wrapper. Extracted
 * from Shell.tsx so the layout route stays focused on data orchestration.
 *
 * `buildTabs` is exported because Shell registers the ⌘1-4 global shortcut that
 * navigates to these same tab targets — one definition, two consumers.
 */

import { type ReactNode } from "react";
import { NavLink, useLocation } from "react-router-dom";
import { useTranslation } from "react-i18next";
import {
  ClipboardList,
  GitBranch,
  MessageSquare,
  Play,
} from "lucide-react";
import type { WorkspaceSummary } from "./types";
import { Badge } from "@/components/ui/badge";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { cn } from "@/lib/cn";

interface TabDef {
  to: string;
  labelKey: string;
  icon: typeof MessageSquare;
  // ⌘1 / ⌘2 / ⌘3 / ⌘4 shortcut (1-based). Shell registers a global
  // keydown handler that maps Meta/Ctrl + digit → navigate(tab.to).
  shortcut: number;
}

export function buildTabs(wsId: string): TabDef[] {
  return [
    { to: `/chat/${wsId}`, labelKey: "chat.tabs.chat", icon: MessageSquare, shortcut: 1 },
    { to: `/chat/${wsId}/dag`, labelKey: "chat.tabs.dag", icon: GitBranch, shortcut: 2 },
    { to: `/chat/${wsId}/ledger`, labelKey: "chat.tabs.ledger", icon: ClipboardList, shortcut: 3 },
    { to: `/chat/${wsId}/replays`, labelKey: "chat.tabs.replays", icon: Play, shortcut: 4 },
  ];
}

export function WorkspaceToolbar({
  workspace,
  agentCount,
  totalUnread,
  onJumpUnread,
}: {
  workspace: WorkspaceSummary;
  agentCount: number;
  totalUnread: number;
  onJumpUnread: () => void;
}) {
  const { t } = useTranslation();
  const tabs = buildTabs(workspace.id);
  const isMac =
    typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform);
  const modKey = isMac ? "⌘" : "Ctrl";

  return (
    <nav className="flex h-10 shrink-0 items-center gap-1 border-b border-border-subtle px-3">
      {tabs.map((tab) => {
        const Icon = tab.icon;
        return (
          <NavLink
            key={tab.to}
            to={tab.to}
            // index route 必须 end，否则 /chat/:wsId 在 /chat/:wsId/dag 时
            // 也算 active。其他 tab 路径足够独特，end 无所谓但保持一致。
            end
            className={({ isActive }) =>
              cn(
                "relative flex items-center gap-1.5 px-3 py-2 text-xs transition-colors",
                isActive
                  ? "text-foreground-primary after:absolute after:inset-x-0 after:-bottom-px after:h-0.5 after:bg-accent-primary"
                  : "text-foreground-secondary hover:text-foreground-primary",
              )
            }
            title={`${t(tab.labelKey)}  ${modKey}${tab.shortcut}`}
          >
            <Icon className="size-3.5" />
            {t(tab.labelKey)}
          </NavLink>
        );
      })}

      <span className="flex-1" />

      {/* workspace actions — 全部 shrink-0 + 小尺寸，跟 tab 行高保持一致 */}
      {agentCount > 0 && (
        <Tooltip>
          <TooltipTrigger asChild>
            <span
              className="flex h-5 shrink-0 items-center gap-1 rounded-full bg-status-running-soft px-2 font-caption text-[10px] font-semibold uppercase tracking-wide text-status-running"
              title={t("chat.memberCount", { count: agentCount })}
            >
              <span
                className="size-1.5 rounded-full bg-status-running"
                aria-hidden
              />
              {t("common.live")}
            </span>
          </TooltipTrigger>
          <TooltipContent side="bottom">
            {t("chat.memberCount", { count: agentCount })}
          </TooltipContent>
        </Tooltip>
      )}

      {totalUnread > 0 && (
        <button
          type="button"
          onClick={onJumpUnread}
          title={t("chat.jumpUnread")}
          className="flex shrink-0 cursor-pointer items-center"
        >
          <Badge className="rounded-full px-2 py-0.5 text-[10px] transition-transform hover:scale-105">
            {t("chat.unread", { count: totalUnread })}
          </Badge>
        </button>
      )}
    </nav>
  );
}

/** 60-80ms cross-fade on Outlet child swap. Long enough to feel soft, short
 *  enough that quick tab-juggling doesn't stack delays. The `key` ties the
 *  fade to the location, so navigating to the same path doesn't replay. */
export function ViewTransition({ children }: { children: ReactNode }) {
  const location = useLocation();
  return (
    <div
      key={location.pathname}
      className="flex h-full min-h-0 flex-1 flex-col animate-in fade-in duration-75"
    >
      {children}
    </div>
  );
}
