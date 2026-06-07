/**
 * CommandPalette — ⌘K (or Ctrl+K) global launcher.
 *
 * Sections:
 *   - 导航: jump to /chat /dag /replays /context /notifications /settings /debug
 *   - 工作空间: switch active workspace (from live /api/agent group-by)
 *   - 唤醒 agent: wake any alive agent by role + id
 *   - 主题: light / dark / system (calls lib/theme.setTheme directly)
 *   - 操作: 新建工作空间(打开 Wizard via custom event) / 全部标已读(notif) ...
 *
 * Built on shadcn `CommandDialog` (which wraps cmdk + Radix Dialog):
 * 自带 portal / focus trap / ESC / ↑↓ / fuzzy filter / 分组渲染.
 * 主题切换 token 跟 popover 一套，跟其他 shadcn 组件视觉一致。
 *
 * Mounted once in AppShell, listens for window keydown to toggle open.
 * Workspace + agent data refreshed lazily on open (one /api/agent call);
 * cheap, avoids a long-lived subscription just for the palette.
 */

import { useCallback, useEffect, useRef, useState } from "react";
import { useNavigate } from "react-router-dom";
import { useTranslation } from "react-i18next";
import {
  Activity,
  BarChart3,
  Bell,
  Boxes,
  Bug,
  ClipboardList,
  GitBranch,
  MessageSquare,
  Moon,
  Play,
  Plus,
  Settings as SettingsIcon,
  Sun,
  SunMoon,
  Zap,
} from "lucide-react";
import { api } from "../api/http";
import type { AgentInfo, Workspace } from "../api/types";
import { setTheme, type ThemeMode } from "@/lib/theme";
import { directionBase } from "@/lib/thread";
import {
  CommandDialog,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
  CommandShortcut,
} from "@/components/ui/command";

// 主导航 — Shell 化后 dag/replays/context 都是 workspace-scoped child
// route，不存在"全局视图"了。CommandPalette 改成：先列对话/通知/设置等
// app-level 入口，再用 WORKSPACE_VIEWS 给当前 workspace 加 4 个内部 view
// 跳转项（只在用户已经选了 workspace 时显示）。
const NAV = [
  { labelKey: "nav.chat", href: "/chat", icon: MessageSquare, hintKey: "cmdk.navHint.chat" },
  { labelKey: "nav.notifications", href: "/notifications", icon: Bell, hintKey: "cmdk.navHint.notifications" },
  // MCP is a primary left-rail destination (McpActivityBar) but was missing
  // from ⌘K — typing "mcp" returned no match. Mirror the rail's Boxes icon and
  // sit it next to 设置, the other config surface.
  { labelKey: "nav.mcp", href: "/mcp", icon: Boxes, hintKey: "cmdk.navHint.mcp" },
  { labelKey: "nav.tasks", href: "/tasks", icon: ClipboardList, hintKey: "cmdk.navHint.tasks" },
  { labelKey: "nav.usage", href: "/usage", icon: BarChart3, hintKey: "cmdk.navHint.usage" },
  { labelKey: "nav.settings", href: "/settings", icon: SettingsIcon, hintKey: "cmdk.navHint.settings" },
  // /debug isn't purely dev tooling — it hosts the blackboard editor (operator
  // writes HITL approval keys) + spells launcher, which have no other home yet.
  // Keep it reachable; the misleading "Legacy M2 grid" hint was fixed to
  // "调试面板" instead (FAULT-005/006).
  { labelKey: "nav.debug", href: "/debug", icon: Bug, hintKey: "cmdk.navHint.debug" },
] as const;

// Keep in sync with buildTabs() in routes/workspace/Shell.tsx — same order,
// same ⌘1-4 mapping (index + 1). The "context" view was replaced by "ledger"
// in the context→ledger migration; the palette wasn't updated, so ⌘ hints
// were off-by-one (replays showed ⌘3 while ⌘3 actually opens the ledger) and
// the ledger view was unreachable from ⌘K. Fixed here.
const WORKSPACE_VIEWS = [
  { id: "chat", labelKey: "chat.tabs.chat", icon: MessageSquare, suffix: "" },
  { id: "dag", labelKey: "chat.tabs.dag", icon: GitBranch, suffix: "/dag" },
  { id: "ledger", labelKey: "chat.tabs.ledger", icon: ClipboardList, suffix: "/ledger" },
  { id: "replays", labelKey: "chat.tabs.replays", icon: Play, suffix: "/replays" },
] as const;

// Keep in sync with SECTIONS in routes/settings.tsx; surfaced here so
// ⌘K can jump to any settings tab without leaving the keyboard.
const SETTINGS_SECTIONS = [
  { id: "general", labelKey: "settings.sections.general" },
  { id: "appearance", labelKey: "settings.sections.appearance" },
  { id: "shortcuts", labelKey: "settings.sections.shortcuts" },
  { id: "models", labelKey: "settings.sections.models" },
  { id: "plugins", labelKey: "settings.sections.plugins" },
  { id: "privacy", labelKey: "settings.sections.privacy" },
  { id: "about", labelKey: "settings.sections.about" },
] as const;

export function CommandPalette() {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  // 进入 open 时把当前 URL 的 wsId 冻结到一个 ref —— 不用 useLocation 是
  // 因为 react-router 的 location 变化会触发 CommandPalette 全量 re-render，
  // 让里头 cmdk 的内部 store 在 dialog 还 mount 时被重新初始化，触发
  // "Cannot read properties of undefined (reading 'subscribe')" 整个 root
  // 白屏。CommandPalette 只在用户主动开 ⌘K 时关心 URL，所以读一次就够。
  const currentWsIdRef = useRef<string | null>(null);
  const [currentWsId, setCurrentWsId] = useState<string | null>(null);
  // Active direction slug (captured alongside wsId) so view-jumps stay in the
  // direction the user is looking at, matching the tab bar / ⌘1-4.
  const [currentThreadSlug, setCurrentThreadSlug] = useState<string | null>(null);
  const [agents, setAgents] = useState<AgentInfo[]>([]);
  const [workspaces, setWorkspaces] = useState<Workspace[]>([]);
  const navigate = useNavigate();

  // Global ⌘K / Ctrl+K opens, Esc closes (handled by Dialog).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const isCmdK =
        e.key.toLowerCase() === "k" && (e.metaKey || e.ctrlKey) && !e.shiftKey;
      if (isCmdK) {
        e.preventDefault();
        setOpen((o) => !o);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  // Refresh agents + 读取当前 URL 的 wsId — 都只在 dialog 打开瞬间发生，
  // 不订阅 router state，避免 nav 流量轰炸 CommandPalette。
  useEffect(() => {
    if (!open) return;
    const m = window.location.pathname.match(/^\/chat\/([^/]+)(?:\/t\/([^/]+))?/);
    const ws = m ? m[1] : null;
    currentWsIdRef.current = ws;
    setCurrentWsId(ws);
    setCurrentThreadSlug(m && m[2] ? m[2] : null);
    api
      .listAgents()
      .then(setAgents)
      .catch(() => {});
    // Canonical workspace list (server filters soft-deleted). Reverse-deriving
    // workspaces from live agents — the old behaviour — surfaced orphan /
    // cwd-missing workspaces of agents whose workspace was already deleted,
    // diverging from the sidebar. Use the same source the sidebar does.
    api
      .listWorkspaces()
      .then(setWorkspaces)
      .catch(() => {});
  }, [open]);

  const close = useCallback(() => setOpen(false), []);
  const go = useCallback(
    (href: string) => {
      navigate(href);
      close();
    },
    [navigate, close],
  );

  const wsIds = new Set(workspaces.map((w) => w.id));
  // Live agents, scoped to those whose workspace still exists. Drops orphan
  // agents whose workspace was deleted (they'd otherwise appear as phantom
  // wake targets the sidebar no longer lists). Don't over-filter before the
  // workspace list has loaded (wsIds empty).
  const liveAgents = agents.filter(
    (a) =>
      a.killed_at == null &&
      a.shim_exit == null &&
      (wsIds.size === 0 || a.workspace_id == null || wsIds.has(a.workspace_id)),
  );

  return (
    <CommandDialog
      open={open}
      onOpenChange={setOpen}
      title={t("cmdk.placeholder")}
      description={t("cmdk.kbd.openHint")}
    >
      <CommandInput placeholder={t("cmdk.placeholder")} />
      <CommandList>
        <CommandEmpty>{t("common.noMatch")}</CommandEmpty>

        <CommandGroup heading={t("cmdk.groups.nav")}>
          {NAV.map((n) => {
            const Icon = n.icon;
            return (
              <CommandItem
                key={n.href}
                value={`${n.href} ${t(n.labelKey)}`}
                onSelect={() => go(n.href)}
              >
                <Icon />
                <span>{t(n.labelKey)}</span>
                <CommandShortcut>{t(n.hintKey)}</CommandShortcut>
              </CommandItem>
            );
          })}
        </CommandGroup>

        {currentWsId && (
          <CommandGroup heading={t("cmdk.groups.currentWsView")}>
            {WORKSPACE_VIEWS.map((v, i) => {
              const Icon = v.icon;
              const href = `${directionBase(currentWsId, currentThreadSlug)}${v.suffix}`;
              return (
                <CommandItem
                  key={v.id}
                  value={`view ${v.id} ${t(v.labelKey)}`}
                  onSelect={() => go(href)}
                >
                  <Icon />
                  <span>{t(v.labelKey)}</span>
                  <CommandShortcut>{`⌘${i + 1}`}</CommandShortcut>
                </CommandItem>
              );
            })}
          </CommandGroup>
        )}

        {workspaces.length > 0 && (
          <CommandGroup heading={t("cmdk.groups.workspaces")}>
            {workspaces.map((ws) => (
              <CommandItem
                key={ws.id}
                value={`ws ${ws.name} ${ws.cwd}`}
                onSelect={() => go(`/chat/${ws.slug}`)}
              >
                <Activity />
                <span>{ws.name}</span>
                <CommandShortcut>{t("cmdk.switchWs")}</CommandShortcut>
              </CommandItem>
            ))}
          </CommandGroup>
        )}

        {liveAgents.length > 0 && (
          <CommandGroup heading={t("cmdk.groups.wakeAgent")}>
            {liveAgents.map((a) => (
              <CommandItem
                key={a.agent_id}
                value={`wake ${a.role} ${a.agent_id}`}
                onSelect={() => {
                  api.wakeAgent(a.agent_id).catch(() => {});
                  close();
                }}
              >
                <Zap className="text-state-wake" />
                <span>{t("cmdk.wake", { role: a.role })}</span>
                <CommandShortcut>{a.agent_id}</CommandShortcut>
              </CommandItem>
            ))}
          </CommandGroup>
        )}

        <CommandGroup heading={t("cmdk.groups.theme")}>
          <CommandItem
            value="theme light"
            onSelect={() => {
              persistTheme("light");
              close();
            }}
          >
            <Sun />
            <span>{t("cmdk.switchToLight")}</span>
            <CommandShortcut>light</CommandShortcut>
          </CommandItem>
          <CommandItem
            value="theme dark"
            onSelect={() => {
              persistTheme("dark");
              close();
            }}
          >
            <Moon />
            <span>{t("cmdk.switchToDark")}</span>
            <CommandShortcut>dark</CommandShortcut>
          </CommandItem>
          <CommandItem
            value="theme system"
            onSelect={() => {
              persistTheme("system");
              close();
            }}
          >
            <SunMoon />
            <span>{t("cmdk.followSystem")}</span>
            <CommandShortcut>system</CommandShortcut>
          </CommandItem>
        </CommandGroup>

        <CommandGroup heading={t("cmdk.groups.actions")}>
          <CommandItem
            value="new workspace"
            onSelect={() => {
              // 用 window-level 事件让 chat 路由的 wizard 自己开，避免把
              // wizard open state 沿 route 树拽下来。
              window.dispatchEvent(new CustomEvent("flockmux:open-wizard"));
              go("/chat");
            }}
          >
            <Plus />
            <span>{t("cmdk.newWorkspace")}</span>
            <CommandShortcut>{t("cmdk.openWizard")}</CommandShortcut>
          </CommandItem>
          {/* 旧"运行配方"项删了 —— 它和"新建工作空间"打开的是同一个创建
              向导（已无 spell 选择器），重复入口徒增困惑。 */}
        </CommandGroup>

        <CommandGroup heading={t("cmdk.groups.settings")}>
          {SETTINGS_SECTIONS.map((s, i) => {
            const label = t(s.labelKey);
            // cmdk 只看 `value` 过滤，把英文 id + 翻译后的 label 都塞进
            // value，这样输 "隐私" 或 "privacy" 都能命中。
            return (
              <CommandItem
                key={s.id}
                value={`settings ${s.id} ${label}`}
                onSelect={() => go(`/settings/${s.id}`)}
              >
                <SettingsIcon />
                <span>{label}</span>
                <CommandShortcut>{`⌘${i + 1}`}</CommandShortcut>
              </CommandItem>
            );
          })}
        </CommandGroup>
      </CommandList>
    </CommandDialog>
  );
}

// 同 Settings 持久化主题的写入路径，避免 palette 和 Settings UI 漂移。
function persistTheme(mode: ThemeMode) {
  const KEY = "flockmux:settings:v1";
  try {
    const raw = window.localStorage.getItem(KEY);
    const obj = raw ? JSON.parse(raw) : {};
    obj.theme = mode;
    window.localStorage.setItem(KEY, JSON.stringify(obj));
  } catch {
    /* ignore */
  }
  setTheme(mode);
}
