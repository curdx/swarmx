/**
 * AppShell — top-level chrome shared by every product route.
 *
 * Layout: thin top bar (brand left + actions right) + outlet fills below.
 *
 * The previous design had a 192px / 56px collapsible left nav with
 * 4 items (Chat / Notifications / Settings / Debug). After the workspace
 * Shell refactor, "Chat" was the only first-class entry; the other 3
 * are account-level / dev-tool surfaces that don't deserve always-on
 * sidebar real estate. We follow the Slack / Linear / Notion playbook
 * now: brand top-left, app menu top-right (notification bell + ⌘K +
 * dropdown for settings / theme / debug). The Outlet gets the entire
 * horizontal width — important because the workspace Shell already
 * owns a 264px sidebar of its own.
 *
 * Sources: Slack's "Meet your simpler, more streamlined sidebar"
 * blog + Linear "How we redesigned the Linear UI" — both moved
 * notifications/settings out of the primary nav and into the user
 * menu / top right.
 *
 * /debug renders without AppShell (it owns its own dark chrome).
 */

import { Link, useLocation, useNavigate, NavLink, Outlet } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { useEffect } from "react";
import {
  Bell,
  Boxes,
  Bug,
  ChevronDown,
  Info,
  Moon,
  Settings,
  Sun,
  SunMoon,
} from "lucide-react";
import { cn } from "@/lib/cn";
import { CommandPalette } from "@/components/CommandPalette";
import { useNotificationBadge } from "@/hooks/useNotificationBadge";
import { setTheme, type ThemeMode } from "@/lib/theme";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";

// Tauri uses titleBarStyle:"Overlay" so the OS draws real traffic lights
// in the window's top-left ~(0,0)→(78,28) region. The header reserves a
// matching left padding so the brand logo doesn't collide with them.
const IS_TAURI =
  typeof window !== "undefined" &&
  (window.location.protocol === "tauri:" ||
    window.location.hostname === "tauri.localhost" ||
    "__TAURI_INTERNALS__" in window);

const STORAGE_KEY = "flockmux:settings:v1";
function persistTheme(mode: ThemeMode) {
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    const parsed = raw ? JSON.parse(raw) : {};
    parsed.themeMode = mode;
    window.localStorage.setItem(STORAGE_KEY, JSON.stringify(parsed));
  } catch {
    /* ignore */
  }
  setTheme(mode);
}

export function AppShell() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const location = useLocation();
  const { hasUnseen, markSeen } = useNotificationBadge();

  // 进入 /notifications 时把 badge 标记 seen — 用户已经在看了，红点应该
  // 消失。其他路由变化不动 seenAt。
  useEffect(() => {
    if (location.pathname.startsWith("/notifications")) markSeen();
  }, [location.pathname, markSeen]);

  const goNotifications = () => navigate("/notifications");

  const isMac =
    typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform);
  const modKey = isMac ? "⌘" : "Ctrl";

  return (
    <TooltipProvider delayDuration={300}>
      <div className="flex h-full flex-col bg-surface-primary text-foreground-primary">
        <header
          className="flex h-11 shrink-0 items-center gap-3 border-b border-border-subtle bg-surface-secondary px-3"
          // Tauri OS lights live in the top-left ~78px strip; pad past them.
          style={IS_TAURI ? { paddingLeft: 88 } : undefined}
        >
          {/* Brand — top-left in browser, right after OS traffic lights in Tauri. */}
          <Link
            to="/chat"
            className="group flex items-center gap-2 rounded-md px-1.5 py-1 transition-colors hover:bg-surface-tertiary"
            title={t("nav.chat")}
          >
            <span className="flex size-7 items-center justify-center rounded-lg bg-accent-primary text-foreground-on-accent shadow-sm">
              <Boxes className="size-4" strokeWidth={2.25} />
            </span>
            <span className="font-mono text-[15px] font-semibold tracking-tight text-foreground-primary">
              flockmux
            </span>
          </Link>

          <span className="flex-1" />

          {/* ⌘K hint — interactive: clicking dispatches the same keyboard
              event so users who haven't memorized the shortcut still
              discover the palette. */}
          <button
            type="button"
            onClick={() => {
              const ev = new KeyboardEvent("keydown", {
                key: "k",
                code: "KeyK",
                metaKey: isMac,
                ctrlKey: !isMac,
                bubbles: true,
              });
              window.dispatchEvent(ev);
            }}
            className="flex h-7 items-center gap-1.5 rounded-md border border-border-subtle bg-surface-elevated px-2 text-[11px] text-foreground-tertiary transition-colors hover:bg-surface-tertiary hover:text-foreground-primary"
            title={t("shell.cmdkHint")}
          >
            <span className="font-mono text-[10px]">{modKey}K</span>
          </button>

          {/* Notification bell — red dot when there's anything new since
              the last visit to /notifications. */}
          <Tooltip>
            <TooltipTrigger asChild>
              <button
                type="button"
                onClick={goNotifications}
                className={cn(
                  "relative flex size-7 items-center justify-center rounded-md transition-colors hover:bg-surface-tertiary",
                  location.pathname.startsWith("/notifications")
                    ? "text-foreground-primary"
                    : "text-foreground-tertiary hover:text-foreground-primary",
                )}
                aria-label={t("nav.notifications")}
              >
                <Bell className="size-4" />
                {hasUnseen && (
                  <span
                    className="absolute right-1 top-1 size-1.5 rounded-full bg-state-danger"
                    aria-label={t("nav.notifications")}
                  />
                )}
              </button>
            </TooltipTrigger>
            <TooltipContent side="bottom">{t("nav.notifications")}</TooltipContent>
          </Tooltip>

          {/* App menu — settings / theme / debug / about. Replaces the
              old left-nav items for everything that isn't "chat". */}
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <button
                type="button"
                className="flex h-7 items-center gap-1 rounded-md px-1.5 text-foreground-tertiary transition-colors hover:bg-surface-tertiary hover:text-foreground-primary"
                aria-label={t("shell.appMenu")}
              >
                <Settings className="size-4" />
                <ChevronDown className="size-3" />
              </button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end" side="bottom" className="w-56">
              <DropdownMenuLabel className="text-[10px] font-semibold uppercase tracking-wider text-foreground-tertiary">
                {t("shell.appMenu")}
              </DropdownMenuLabel>
              <DropdownMenuSeparator />
              <NavLink to="/settings" className="block">
                {({ isActive }) => (
                  <DropdownMenuItem
                    className={cn("gap-2", isActive && "bg-surface-tertiary")}
                  >
                    <Settings className="size-3.5" />
                    <span>{t("nav.settings")}</span>
                  </DropdownMenuItem>
                )}
              </NavLink>
              <DropdownMenuSeparator />
              <DropdownMenuLabel className="text-[10px] font-semibold uppercase tracking-wider text-foreground-tertiary">
                {t("cmdk.groups.theme")}
              </DropdownMenuLabel>
              <DropdownMenuItem onSelect={() => persistTheme("light")} className="gap-2">
                <Sun className="size-3.5" />
                <span>{t("cmdk.switchToLight")}</span>
              </DropdownMenuItem>
              <DropdownMenuItem onSelect={() => persistTheme("dark")} className="gap-2">
                <Moon className="size-3.5" />
                <span>{t("cmdk.switchToDark")}</span>
              </DropdownMenuItem>
              <DropdownMenuItem onSelect={() => persistTheme("system")} className="gap-2">
                <SunMoon className="size-3.5" />
                <span>{t("cmdk.followSystem")}</span>
              </DropdownMenuItem>
              <DropdownMenuSeparator />
              <NavLink to="/debug" className="block">
                <DropdownMenuItem className="gap-2 text-foreground-tertiary">
                  <Bug className="size-3.5" />
                  <span>{t("nav.debug")}</span>
                </DropdownMenuItem>
              </NavLink>
              <DropdownMenuItem
                onSelect={() => navigate("/settings/about")}
                className="gap-2 text-foreground-tertiary"
              >
                <Info className="size-3.5" />
                <span>{t("shell.about")}</span>
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>

          <span className="font-caption text-xs text-foreground-tertiary">
            127.0.0.1:7777
          </span>
        </header>
        <main className="min-h-0 flex-1 overflow-hidden">
          <Outlet />
        </main>
        <CommandPalette />
      </div>
    </TooltipProvider>
  );
}
