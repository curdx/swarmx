/**
 * AppShell — top-level chrome shared by every product route.
 *
 * Layout: collapsible left sidebar + thin top bar + outlet.
 *
 * Sidebar matches Linear / Cursor / Discord conventions — icon + label
 * column on the left. Default width is 12rem (192px) so the Chinese
 * labels read; users can collapse to 3.5rem (56px) icon-only when chat's
 * "workspace + messages + members" three-pane layout needs the
 * horizontal room. State persists to localStorage so the user's choice
 * sticks across reloads.
 *
 * /debug renders without AppShell (it owns its own dark chrome).
 */

import { useEffect, useState } from "react";
import { NavLink, Outlet } from "react-router-dom";
import { useTranslation } from "react-i18next";
import {
  Bell,
  Bug,
  FileText,
  GitBranch,
  Inbox,
  MessageSquare,
  PanelLeftClose,
  PanelLeftOpen,
  Play,
  Settings,
  type LucideIcon,
} from "lucide-react";
import { cn } from "@/lib/cn";
import { CommandPalette } from "@/components/CommandPalette";

// Tauri uses titleBarStyle:"Overlay" so the OS draws real traffic lights
// in the window's top-left ~(0,0)→(78,28) region. We hide our painted
// decoration there and reserve that strip with padding so OS lights
// don't cover the brand row. In a plain browser there are no OS lights
// — show our painted ones so the chrome doesn't look bare.
const IS_TAURI =
  typeof window !== "undefined" &&
  (window.location.protocol === "tauri:" ||
    window.location.hostname === "tauri.localhost" ||
    // Tauri 2 injects internals into every webview, dev included.
    "__TAURI_INTERNALS__" in window);

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

const COLLAPSED_KEY = "flockmux:nav-collapsed";

export function AppShell() {
  const { t } = useTranslation();
  const [collapsed, setCollapsed] = useState<boolean>(() => {
    try {
      return window.localStorage.getItem(COLLAPSED_KEY) === "1";
    } catch {
      return false;
    }
  });

  useEffect(() => {
    try {
      window.localStorage.setItem(COLLAPSED_KEY, collapsed ? "1" : "0");
    } catch {
      /* ignore quota errors */
    }
  }, [collapsed]);

  return (
    <div className="flex h-full bg-surface-primary text-foreground-primary">
      <aside
        className={cn(
          "flex shrink-0 flex-col border-r border-border-subtle bg-surface-secondary transition-[width] duration-150",
          collapsed ? "w-14" : "w-48",
        )}
        // Reserve the OS traffic-lights overlay strip in Tauri so logo
        // + label don't collide with the kernel-drawn buttons. Browser
        // gets a small top padding too for visual symmetry.
        style={IS_TAURI ? { paddingTop: 28 } : undefined}
      >
        {/* Brand row — logo dot + (when expanded) the app name. Sized to
            match the top header height so the divider lines up. */}
        <div
          className={cn(
            "flex h-11 shrink-0 items-center border-b border-border-subtle",
            collapsed ? "justify-center" : "px-3",
          )}
        >
          <span className="size-6 rounded-md bg-accent-primary" />
          {!collapsed && (
            <span className="ml-2 font-heading text-sm font-semibold">
              flockmux
            </span>
          )}
        </div>

        <nav
          className={cn(
            "flex flex-1 flex-col gap-0.5 py-2",
            collapsed ? "items-center" : "px-2",
          )}
        >
          {NAV.map((item) => {
            const Icon = item.icon;
            return (
              <NavLink
                key={item.to}
                to={item.to}
                title={collapsed ? t(item.key) : undefined}
                aria-label={t(item.key)}
                className={({ isActive }) =>
                  cn(
                    "flex items-center rounded-lg transition-colors",
                    collapsed
                      ? "size-10 justify-center"
                      : "gap-3 px-3 py-2 text-sm",
                    isActive
                      ? "bg-accent-primary-soft text-accent-primary"
                      : "text-foreground-tertiary hover:bg-surface-tertiary hover:text-foreground-primary",
                  )
                }
              >
                <Icon className="size-[18px] shrink-0" />
                {!collapsed && <span>{t(item.key)}</span>}
              </NavLink>
            );
          })}
        </nav>

        {/* Collapse toggle — bottom of rail so it's out of the way but
            findable. Icon flips based on state. */}
        <button
          onClick={() => setCollapsed((v) => !v)}
          title={collapsed ? t("shell.expand") : t("shell.collapse")}
          aria-label={collapsed ? t("shell.expand") : t("shell.collapse")}
          className={cn(
            "flex shrink-0 items-center gap-2 border-t border-border-subtle text-foreground-tertiary transition-colors hover:bg-surface-tertiary hover:text-foreground-primary",
            collapsed ? "h-10 justify-center" : "h-10 px-4 text-xs",
          )}
        >
          {collapsed ? (
            <PanelLeftOpen className="size-[18px]" />
          ) : (
            <>
              <PanelLeftClose className="size-[18px]" />
              <span>{t("shell.collapse")}</span>
            </>
          )}
        </button>
      </aside>

      <div className="flex min-w-0 flex-1 flex-col">
        <header
          className="flex h-11 shrink-0 items-center gap-3 border-b border-border-subtle bg-surface-secondary px-4"
          // Tauri OS lights overlap the sidebar only; the header starts
          // right of the sidebar so it's clear, but keep small top
          // padding so the header content visually settles below the
          // OS title strip when present.
          style={IS_TAURI ? { paddingTop: 4 } : undefined}
        >
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
