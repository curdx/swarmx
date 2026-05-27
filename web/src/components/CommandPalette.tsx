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

import { useCallback, useEffect, useState } from "react";
import { useNavigate } from "react-router-dom";
import { useTranslation } from "react-i18next";
import {
  Activity,
  Bell,
  Bug,
  FileText,
  GitBranch,
  MessageSquare,
  Moon,
  Play,
  Plus,
  Settings as SettingsIcon,
  Sparkles,
  Sun,
  SunMoon,
  Zap,
} from "lucide-react";
import { api } from "../api/http";
import type { AgentInfo } from "../api/types";
import { setTheme, type ThemeMode } from "@/lib/theme";
import {
  CommandDialog,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
  CommandShortcut,
} from "@/components/ui/command";

const NAV = [
  { labelKey: "nav.chat", href: "/chat", icon: MessageSquare, hintKey: "cmdk.navHint.chat" },
  { labelKey: "nav.dag", href: "/dag", icon: GitBranch, hintKey: "cmdk.navHint.dag" },
  { labelKey: "nav.replays", href: "/replays", icon: Play, hintKey: "cmdk.navHint.replays" },
  { labelKey: "nav.context", href: "/context", icon: FileText, hintKey: "cmdk.navHint.context" },
  { labelKey: "nav.notifications", href: "/notifications", icon: Bell, hintKey: "cmdk.navHint.notifications" },
  { labelKey: "nav.settings", href: "/settings", icon: SettingsIcon, hintKey: "cmdk.navHint.settings" },
  { labelKey: "nav.debug", href: "/debug", icon: Bug, hintKey: "cmdk.navHint.debug" },
] as const;

// Keep in sync with SECTIONS in routes/settings.tsx; surfaced here so
// ⌘K can jump to any settings tab without leaving the keyboard.
const SETTINGS_SECTIONS = [
  { id: "general", labelKey: "settings.sections.general" },
  { id: "appearance", labelKey: "settings.sections.appearance" },
  { id: "shortcuts", labelKey: "settings.sections.shortcuts" },
  { id: "plugins", labelKey: "settings.sections.plugins" },
  { id: "privacy", labelKey: "settings.sections.privacy" },
  { id: "about", labelKey: "settings.sections.about" },
] as const;

export function CommandPalette() {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const [agents, setAgents] = useState<AgentInfo[]>([]);
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

  // Refresh agents lazily on open — palette stays fresh without paying
  // for a constant /ws subscription.
  useEffect(() => {
    if (!open) return;
    api
      .listAgents()
      .then(setAgents)
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

  const liveAgents = agents.filter((a) => a.killed_at == null && a.shim_exit == null);
  const workspaces = Array.from(
    new Set(liveAgents.map((a) => a.workspace || "(no workspace)")),
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

        {workspaces.length > 0 && (
          <CommandGroup heading={t("cmdk.groups.workspaces")}>
            {workspaces.map((ws) => {
              const id = ws.slice(-8) || "default";
              return (
                <CommandItem
                  key={ws}
                  value={`ws ${ws}`}
                  onSelect={() => go(`/chat/${id}`)}
                >
                  <Activity />
                  <span>{ws.split("/").slice(-2).join("/") || ws}</span>
                  <CommandShortcut>{t("cmdk.switchWs")}</CommandShortcut>
                </CommandItem>
              );
            })}
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
          <CommandItem
            value="run spell"
            onSelect={() => {
              window.dispatchEvent(new CustomEvent("flockmux:open-wizard"));
              go("/chat");
            }}
          >
            <Sparkles />
            <span>{t("cmdk.runSpell")}</span>
            <CommandShortcut>{t("cmdk.openWizard")}</CommandShortcut>
          </CommandItem>
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
