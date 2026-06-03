/**
 * McpActivityBar — 最外层应用导航菜单，常驻在 AppShell 内容区最左。横排菜单项
 * (图标 + 文字)，可展开/收起成纯图标窄条(状态存 localStorage)：
 *   - MCP(顶)   → 跳独立页面 /mcp
 *   - 设置(顶)  → 跳 /settings
 *   - 底部       → 菜单展开/收起开关
 *
 * MCP 现在是独立页面(routes/mcp.tsx)，不再是滑出/停靠面板，所以这里只是导航。
 */

import { useEffect, useState, type ReactElement } from "react";
import { NavLink, useLocation } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { Blocks, ChevronsLeft, ChevronsRight, Settings } from "lucide-react";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { cn } from "@/lib/cn";

const MENU_KEY = "flockmux:mcp:menuCollapsed:v1";

function readFlag(key: string): boolean {
  try {
    return window.localStorage.getItem(key) === "1";
  } catch {
    return false;
  }
}

const itemBase =
  "flex w-full items-center gap-2.5 rounded-lg px-2.5 py-2 text-[13px] font-medium transition-colors";
const itemActive = "bg-accent-primary-soft text-accent-primary";
const itemIdle =
  "text-foreground-secondary hover:bg-surface-tertiary hover:text-foreground-primary";

export function McpActivityBar() {
  const { t } = useTranslation();
  const { pathname } = useLocation();
  const [collapsedPref, setCollapsedPref] = useState(() => readFlag(MENU_KEY));
  // Below `lg` the workspace column + chat need the width, so force the rail to
  // its icon-only state regardless of the saved preference (R2-004); widening
  // back restores the user's choice. CSS can't do this alone — the labels are
  // rendered conditionally on `collapsed`, not just hidden.
  const [narrow, setNarrow] = useState(false);
  useEffect(() => {
    const mq = window.matchMedia("(max-width: 1023.98px)");
    const sync = () => setNarrow(mq.matches);
    sync();
    mq.addEventListener("change", sync);
    return () => mq.removeEventListener("change", sync);
  }, []);
  const collapsed = collapsedPref || narrow;

  // NOTE: NavLink's function-style className ({isActive}) => ... does NOT
  // survive Radix `asChild`/Slot. In the collapsed state each NavLink is
  // wrapped by <TooltipTrigger asChild>, and Slot only merges *string*
  // classNames — a function gets String()'d into the DOM `class` attribute
  // verbatim, so itemBase/justify-center never apply and the icons sit
  // unaligned. Compute active from the location ourselves and pass a plain
  // string so it merges correctly in both collapsed and expanded states.
  const linkClass = (active: boolean) =>
    cn(itemBase, collapsed && "justify-center px-0", active ? itemActive : itemIdle);

  const setCollapsedPersist = (v: boolean) => {
    setCollapsedPref(v);
    try {
      window.localStorage.setItem(MENU_KEY, v ? "1" : "0");
    } catch {
      /* ignore */
    }
  };

  // 收起态下菜单项只剩图标 → 用 tooltip 补回标签。
  const withTip = (label: string, node: ReactElement) =>
    collapsed ? (
      <Tooltip>
        <TooltipTrigger asChild>{node}</TooltipTrigger>
        <TooltipContent side="right">{label}</TooltipContent>
      </Tooltip>
    ) : (
      node
    );

  return (
    <aside
      className={cn(
        "flex shrink-0 flex-col gap-1 border-r border-border-subtle bg-surface-secondary py-2.5",
        collapsed ? "w-14 px-1.5" : "w-[184px] px-2",
      )}
    >
      {/* MCP — 跳独立页面 /mcp */}
      {withTip(
        t("mcp.title", "MCP"),
        <NavLink
          to="/mcp"
          aria-label={t("mcp.title", "MCP")}
          className={linkClass(pathname.startsWith("/mcp"))}
        >
          <Blocks className="size-[18px] shrink-0" />
          {!collapsed && <span className="font-heading">{t("mcp.title", "MCP")}</span>}
        </NavLink>,
      )}

      {/* 设置 — 跳 /settings */}
      {withTip(
        t("nav.settings", "设置"),
        <NavLink
          to="/settings"
          aria-label={t("nav.settings", "设置")}
          className={linkClass(pathname.startsWith("/settings"))}
        >
          <Settings className="size-[18px] shrink-0" />
          {!collapsed && <span className="font-heading">{t("nav.settings", "设置")}</span>}
        </NavLink>,
      )}

      {/* 底部：菜单展开/收起开关 */}
      {withTip(
        collapsed ? t("mcp.menuExpand", "展开菜单") : t("mcp.menuCollapse", "收起菜单"),
        <button
          type="button"
          onClick={() => setCollapsedPersist(!collapsed)}
          aria-label={collapsed ? t("mcp.menuExpand", "展开菜单") : t("mcp.menuCollapse", "收起菜单")}
          className={cn(itemBase, "mt-auto", collapsed && "justify-center px-0", itemIdle)}
        >
          {collapsed ? (
            <ChevronsRight className="size-[18px] shrink-0" />
          ) : (
            <>
              <ChevronsLeft className="size-[18px] shrink-0" />
              <span className="font-heading">{t("mcp.menuCollapse", "收起")}</span>
            </>
          )}
        </button>,
      )}
    </aside>
  );
}
