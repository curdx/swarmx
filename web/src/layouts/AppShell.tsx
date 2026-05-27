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

import { Link, NavLink, Outlet, useLocation } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { useEffect } from "react";
import { Boxes, Settings } from "lucide-react";
import { cn } from "@/lib/cn";
import { CommandPalette } from "@/components/CommandPalette";
import { NotificationPopover } from "@/components/NotificationPopover";
import { useNotificationBadge } from "@/hooks/useNotificationBadge";
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

export function AppShell() {
  const { t } = useTranslation();
  const location = useLocation();
  const { hasUnseen, markSeen } = useNotificationBadge();

  // 进入 /notifications 时把 badge 标记 seen — 用户已经在看了，红点应该
  // 消失。其他路由变化不动 seenAt。
  useEffect(() => {
    if (location.pathname.startsWith("/notifications")) markSeen();
  }, [location.pathname, markSeen]);

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

          {/* Notification bell — Popover quick preview。点 bell 弹 360px
              panel 显示最近 12 条事件，每条 click 跳对应 workspace；底
              部"查看全部"跳 /notifications 完整页。GitHub / Discord /
              Notion 同款做法 — 通知是"瞄一眼"动作，不该跳走当前 view。
              hasUnseen 触发右上红点；popover open 时自动 markSeen 清掉。 */}
          <NotificationPopover hasUnseen={hasUnseen} onSeen={markSeen} />

          {/* Settings — 直接跳完整页，不做下拉杂物箱。GitHub / Slack /
              Cursor 都是这模式：gear icon = 单击就走，theme / debug /
              about 全部活在 /settings 内部 section。Debug 主入口走 ⌘K
              (它是开发者后门，不该 first-class)。 */}
          <Tooltip>
            <TooltipTrigger asChild>
              <NavLink
                to="/settings"
                className={({ isActive }) =>
                  cn(
                    "flex size-7 items-center justify-center rounded-md transition-colors hover:bg-surface-tertiary hover:text-foreground-primary",
                    isActive
                      ? "text-foreground-primary"
                      : "text-foreground-tertiary",
                  )
                }
                aria-label={t("nav.settings")}
              >
                <Settings className="size-4" />
              </NavLink>
            </TooltipTrigger>
            <TooltipContent side="bottom">{t("nav.settings")}</TooltipContent>
          </Tooltip>

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
