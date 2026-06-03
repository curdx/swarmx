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

import { Link, Outlet, useLocation } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { useEffect } from "react";
import { Boxes, Command, Search } from "lucide-react";
import { CommandPalette } from "@/components/CommandPalette";
import { McpActivityBar } from "@/components/mcp/McpActivityBar";
import { ErrorBoundary } from "@/components/ErrorBoundary";
import { NotificationPopover } from "@/components/NotificationPopover";
import { useNotificationBadge } from "@/hooks/useNotificationBadge";
import { api } from "@/api/http";
import { primeInputPolicies } from "@/lib/cliInputPolicy";
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

  // Prime per-CLI input policies from the backend plugin manifest once at
  // startup, so the terminal's keystroke-settle timing is data-driven (no
  // hardcoded per-CLI branch). Best-effort: a fetch failure just leaves the
  // permissive 0ms default. Runs long before any agent terminal mounts.
  useEffect(() => {
    api
      .listPlugins()
      .then(primeInputPolicies)
      .catch(() => {});
  }, []);

  const isMac =
    typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform);

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

          {/* ⌘K trigger — 用 Search icon + 旁边小 kbd hint，跟 🔔 / ⚙
              统一成 icon button 风格。之前用纯 kbd + border 像"输入框
              提示"，视觉权重比旁边的 icon button 重，三个并排不统一。
              click 仍触发同个 keyboard event，键盘党 / 鼠标党都覆盖。
              Notion / Slack / Linear 顶栏 search 全是这模式。 */}
          <Tooltip>
            <TooltipTrigger asChild>
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
                className="flex h-7 items-center gap-1.5 rounded-md px-1.5 text-foreground-tertiary transition-colors hover:bg-surface-tertiary hover:text-foreground-primary"
                aria-label={t("shell.cmdkHint")}
              >
                <Search className="size-4" />
                {/* ⌘ as a crisp lucide SVG (not the U+2318 font glyph, which
                    renders thin/small next to a cap "K"). Non-mac falls back
                    to a "Ctrl" label. Borderless to match the bell / search
                    icon-button weight. */}
                <span className="hidden items-center gap-0.5 font-mono text-[11px] leading-none text-foreground-tertiary sm:inline-flex">
                  {isMac ? (
                    <Command className="size-3" strokeWidth={2.25} />
                  ) : (
                    <span>Ctrl</span>
                  )}
                  K
                </span>
              </button>
            </TooltipTrigger>
            <TooltipContent side="bottom">{t("shell.cmdkHint")}</TooltipContent>
          </Tooltip>

          {/* Notification bell — Popover quick preview。点 bell 弹 360px
              panel 显示最近 12 条事件，每条 click 跳对应 workspace；底
              部"查看全部"跳 /notifications 完整页。GitHub / Discord /
              Notion 同款做法 — 通知是"瞄一眼"动作，不该跳走当前 view。
              hasUnseen 触发右上红点；popover open 时自动 markSeen 清掉。 */}
          <NotificationPopover hasUnseen={hasUnseen} onSeen={markSeen} />

          {/* 设置已挪到最左导航菜单条(McpActivityBar)底部 — 顶栏只留
              ⌘K + 通知,跟 VS Code 把设置放活动栏底一致。 */}
        </header>
        <main className="flex min-h-0 flex-1 overflow-hidden">
          {/* 最外层窄活动栏(VS Code 式)，常驻所有 AppShell 路由的内容区最左。
              MCP 图标点开左侧 Sheet。全屏录像 / debug 在 AppShell 之外，不受影响。 */}
          <McpActivityBar />
          {/* Contain a render throw to the content area — the header (and its
              ⌘K / bell / settings) stays usable, and the route key resets the
              boundary so navigating away clears a stuck error. */}
          <div className="flex min-w-0 flex-1 flex-col">
            <ErrorBoundary resetKey={location.pathname}>
              <Outlet />
            </ErrorBoundary>
          </div>
        </main>
        <CommandPalette />
      </div>
    </TooltipProvider>
  );
}
