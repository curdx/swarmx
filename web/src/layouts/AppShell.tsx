/**
 * AppShell — top-level chrome shared by every product route.
 *
 * Layout: 56px icon-only sidebar on the left + thin top bar + outlet.
 * Matches the convention of Linear / Cursor / Discord / VS Code — a
 * single column of route icons keeps horizontal space for chat's three
 * panes and DAG's legend, while still giving every route a one-click
 * jump to its siblings. Hover surfaces the localized label as a native
 * tooltip; cmdk (⌘K) is the keyboard alternative.
 *
 * /debug renders without AppShell (it owns its own dark chrome).
 */

import { NavLink, Outlet } from "react-router-dom";
import { useTranslation } from "react-i18next";
import {
  Bell,
  Bug,
  FileText,
  GitBranch,
  Inbox,
  MessageSquare,
  Play,
  Settings,
  type LucideIcon,
} from "lucide-react";
import { cn } from "@/lib/cn";
import { CommandPalette } from "@/components/CommandPalette";

interface NavItem {
  to: string;
  key: string;
  icon: LucideIcon;
}

const NAV: readonly NavItem[] = [
  { to: "/chat", key: "nav.chat", icon: MessageSquare },
  { to: "/dag", key: "nav.dag", icon: GitBranch },
  { to: "/replays", key: "nav.replays", icon: Play },
  { to: "/context", key: "nav.context", icon: FileText },
  { to: "/inbox", key: "nav.inbox", icon: Inbox },
  { to: "/notifications", key: "nav.notifications", icon: Bell },
  { to: "/settings", key: "nav.settings", icon: Settings },
  { to: "/debug", key: "nav.debug", icon: Bug },
] as const;

function TrafficLights() {
  return (
    <div className="flex items-center gap-2">
      <span className="size-3 rounded-full bg-[#FF5F57]" />
      <span className="size-3 rounded-full bg-[#FEBC2E]" />
      <span className="size-3 rounded-full bg-[#28C840]" />
    </div>
  );
}

export function AppShell() {
  const { t } = useTranslation();
  return (
    <div className="flex h-full bg-surface-primary text-foreground-primary">
      {/* Left rail — 56px, icon-only nav. Active route gets the accent
          tint; hover gives a subtle surface fill + opens the native title
          tooltip for the label. */}
      <aside className="flex w-14 shrink-0 flex-col items-center border-r border-border-subtle bg-surface-secondary py-2">
        <div className="mb-2 flex h-9 w-9 items-center justify-center">
          <span className="size-6 rounded-md bg-accent-primary" />
        </div>
        <nav className="flex w-full flex-col items-center gap-1">
          {NAV.map((item) => {
            const Icon = item.icon;
            return (
              <NavLink
                key={item.to}
                to={item.to}
                title={t(item.key)}
                aria-label={t(item.key)}
                className={({ isActive }) =>
                  cn(
                    "flex size-10 items-center justify-center rounded-lg transition-colors",
                    isActive
                      ? "bg-accent-primary-soft text-accent-primary"
                      : "text-foreground-tertiary hover:bg-surface-tertiary hover:text-foreground-primary",
                  )
                }
              >
                <Icon className="size-[18px]" />
              </NavLink>
            );
          })}
        </nav>
      </aside>
      <div className="flex min-w-0 flex-1 flex-col">
        <header className="flex h-11 shrink-0 items-center gap-3 border-b border-border-subtle bg-surface-secondary pl-4 pr-4">
          <TrafficLights />
          <span className="font-heading text-sm font-semibold">flockmux</span>
          <div className="flex-1" />
          <kbd
            className="rounded border border-border-subtle bg-surface-elevated px-1.5 py-0.5 font-mono text-[10px] text-foreground-tertiary"
            title={t("shell.cmdkHint")}
          >
            ⌘K
          </kbd>
          <span className="font-caption text-xs text-foreground-tertiary">
            127.0.0.1:7777
          </span>
        </header>
        <main className="min-h-0 flex-1 overflow-hidden">
          <Outlet />
        </main>
      </div>
      <CommandPalette />
    </div>
  );
}
