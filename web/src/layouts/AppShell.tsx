/**
 * AppShell — top-level chrome shared by every product route.
 *
 * Mirrors the Pencil TitleBar (id NkrSV in untitled.pen → u6kF7Z): 44px
 * row, surface-secondary fill, traffic lights left, brand + nav center,
 * window actions right. Traffic lights are decorative in the browser; the
 * Tauri shell can wire them to the OS window controls later.
 *
 * /debug uses its own dark legacy chrome and renders without AppShell.
 */

import { NavLink, Outlet } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { cn } from "@/lib/cn";
import { CommandPalette } from "@/components/CommandPalette";

const NAV = [
  { to: "/chat", key: "nav.chat" },
  { to: "/dag", key: "nav.dag" },
  { to: "/replays", key: "nav.replays" },
  { to: "/context", key: "nav.context" },
  { to: "/inbox", key: "nav.inbox" },
  { to: "/notifications", key: "nav.notifications" },
  { to: "/settings", key: "nav.settings" },
  { to: "/debug", key: "nav.debug" },
] as const;

function TrafficLights() {
  return (
    <div className="flex items-center gap-2 pl-1">
      <span className="size-3 rounded-full bg-[#FF5F57]" />
      <span className="size-3 rounded-full bg-[#FEBC2E]" />
      <span className="size-3 rounded-full bg-[#28C840]" />
    </div>
  );
}

export function AppShell() {
  const { t } = useTranslation();
  return (
    <div className="flex h-full flex-col bg-surface-primary text-foreground-primary">
      <header className="flex h-11 shrink-0 items-center gap-6 border-b border-border-subtle bg-surface-secondary px-4">
        <TrafficLights />
        <div className="flex items-center gap-2">
          <span className="size-5 rounded-md bg-accent-primary" />
          <span className="font-heading text-sm font-semibold">flockmux</span>
        </div>
        <nav className="flex items-center gap-1">
          {NAV.map((item) => (
            <NavLink
              key={item.to}
              to={item.to}
              className={({ isActive }) =>
                cn(
                  "rounded-md px-3 py-1.5 text-xs transition-colors",
                  isActive
                    ? "bg-accent-primary text-foreground-on-accent"
                    : "text-foreground-secondary hover:bg-surface-tertiary",
                )
              }
            >
              {t(item.key)}
            </NavLink>
          ))}
        </nav>
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
      <CommandPalette />
    </div>
  );
}
