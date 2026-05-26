/**
 * CommandPalette — ⌘K (or Ctrl+K) global launcher.
 *
 * Sections:
 *   - 导航: jump to /chat /dag /replays /context /inbox /notifications /settings /debug
 *   - 工作空间: switch active workspace (from live /api/agent group-by)
 *   - 主题: light / dark / system (calls lib/theme.setTheme directly)
 *   - 操作: 新建工作空间(打开 Wizard via custom event) / 全部标已读(notif) ...
 *
 * Implementation notes:
 *   - Mounted once in AppShell, listens for window keydown.
 *   - cmdk Command.Dialog handles ⌘K open + Esc close + ↑↓ navigation
 *     + filtering by typing.
 *   - Workspace + agent data refreshed lazily on open (one /api/agent
 *     call); cheap, avoids a long-lived subscription just for the
 *     palette.
 */

import { useCallback, useEffect, useState } from "react";
import { Command } from "cmdk";
import { useNavigate } from "react-router-dom";
import { useTranslation } from "react-i18next";
import {
  Activity,
  Bell,
  Bug,
  FileText,
  GitBranch,
  Inbox as InboxIcon,
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

const NAV = [
  { labelKey: "nav.chat", href: "/chat", icon: MessageSquare, hintKey: "cmdk.navHint.chat" },
  { labelKey: "nav.dag", href: "/dag", icon: GitBranch, hintKey: "cmdk.navHint.dag" },
  { labelKey: "nav.replays", href: "/replays", icon: Play, hintKey: "cmdk.navHint.replays" },
  { labelKey: "nav.context", href: "/context", icon: FileText, hintKey: "cmdk.navHint.context" },
  { labelKey: "nav.inbox", href: "/inbox", icon: InboxIcon, hintKey: "cmdk.navHint.inbox" },
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
    <Command.Dialog
      open={open}
      onOpenChange={setOpen}
      label={t("cmdk.placeholder")}
      className="fixed inset-0 z-[60] flex items-start justify-center bg-black/40 p-6 pt-[15vh] backdrop-blur-sm"
      shouldFilter={true}
    >
      <div
        className="flex w-full max-w-xl flex-col overflow-hidden rounded-xl border border-border-subtle bg-surface-elevated shadow-2xl"
        onClick={(e) => e.stopPropagation()}
      >
        <Command.Input
          placeholder={t("cmdk.placeholder")}
          className="h-12 w-full border-b border-border-subtle bg-transparent px-4 font-body text-sm text-foreground-primary placeholder:text-foreground-tertiary focus:outline-none"
        />
        <Command.List className="max-h-[60vh] overflow-y-auto p-2">
          <Command.Empty className="py-6 text-center font-caption text-xs text-foreground-tertiary">
            {t("common.noMatch")}
          </Command.Empty>

          <Command.Group
            heading={t("cmdk.groups.nav")}
            className="[&_[cmdk-group-heading]]:px-2 [&_[cmdk-group-heading]]:py-1.5 [&_[cmdk-group-heading]]:font-caption [&_[cmdk-group-heading]]:text-[10px] [&_[cmdk-group-heading]]:font-semibold [&_[cmdk-group-heading]]:uppercase [&_[cmdk-group-heading]]:tracking-wider [&_[cmdk-group-heading]]:text-foreground-tertiary"
          >
            {NAV.map((n) => {
              const Icon = n.icon;
              return (
                <Item
                  key={n.href}
                  onSelect={() => go(n.href)}
                  icon={<Icon className="size-4" />}
                  label={t(n.labelKey)}
                  hint={t(n.hintKey)}
                />
              );
            })}
          </Command.Group>

          {workspaces.length > 0 && (
            <Command.Group
              heading={t("cmdk.groups.workspaces")}
              className="mt-1 [&_[cmdk-group-heading]]:px-2 [&_[cmdk-group-heading]]:py-1.5 [&_[cmdk-group-heading]]:font-caption [&_[cmdk-group-heading]]:text-[10px] [&_[cmdk-group-heading]]:font-semibold [&_[cmdk-group-heading]]:uppercase [&_[cmdk-group-heading]]:tracking-wider [&_[cmdk-group-heading]]:text-foreground-tertiary"
            >
              {workspaces.map((ws) => {
                const id = ws.slice(-8) || "default";
                return (
                  <Item
                    key={ws}
                    value={`ws ${ws}`}
                    onSelect={() => go(`/chat/${id}`)}
                    icon={<Activity className="size-4" />}
                    label={ws.split("/").slice(-2).join("/") || ws}
                    hint={t("cmdk.switchWs")}
                  />
                );
              })}
            </Command.Group>
          )}

          {liveAgents.length > 0 && (
            <Command.Group
              heading={t("cmdk.groups.wakeAgent")}
              className="mt-1 [&_[cmdk-group-heading]]:px-2 [&_[cmdk-group-heading]]:py-1.5 [&_[cmdk-group-heading]]:font-caption [&_[cmdk-group-heading]]:text-[10px] [&_[cmdk-group-heading]]:font-semibold [&_[cmdk-group-heading]]:uppercase [&_[cmdk-group-heading]]:tracking-wider [&_[cmdk-group-heading]]:text-foreground-tertiary"
            >
              {liveAgents.map((a) => (
                <Item
                  key={a.agent_id}
                  value={`wake ${a.role} ${a.agent_id}`}
                  onSelect={() => {
                    api.wakeAgent(a.agent_id).catch(() => {});
                    close();
                  }}
                  icon={<Zap className="size-4 text-state-wake" />}
                  label={t("cmdk.wake", { role: a.role })}
                  hint={a.agent_id}
                />
              ))}
            </Command.Group>
          )}

          <Command.Group
            heading={t("cmdk.groups.theme")}
            className="mt-1 [&_[cmdk-group-heading]]:px-2 [&_[cmdk-group-heading]]:py-1.5 [&_[cmdk-group-heading]]:font-caption [&_[cmdk-group-heading]]:text-[10px] [&_[cmdk-group-heading]]:font-semibold [&_[cmdk-group-heading]]:uppercase [&_[cmdk-group-heading]]:tracking-wider [&_[cmdk-group-heading]]:text-foreground-tertiary"
          >
            <Item
              value="theme light"
              onSelect={() => {
                persistTheme("light");
                close();
              }}
              icon={<Sun className="size-4" />}
              label={t("cmdk.switchToLight")}
              hint="light"
            />
            <Item
              value="theme dark"
              onSelect={() => {
                persistTheme("dark");
                close();
              }}
              icon={<Moon className="size-4" />}
              label={t("cmdk.switchToDark")}
              hint="dark"
            />
            <Item
              value="theme system"
              onSelect={() => {
                persistTheme("system");
                close();
              }}
              icon={<SunMoon className="size-4" />}
              label={t("cmdk.followSystem")}
              hint="system"
            />
          </Command.Group>

          <Command.Group
            heading={t("cmdk.groups.actions")}
            className="mt-1 [&_[cmdk-group-heading]]:px-2 [&_[cmdk-group-heading]]:py-1.5 [&_[cmdk-group-heading]]:font-caption [&_[cmdk-group-heading]]:text-[10px] [&_[cmdk-group-heading]]:font-semibold [&_[cmdk-group-heading]]:uppercase [&_[cmdk-group-heading]]:tracking-wider [&_[cmdk-group-heading]]:text-foreground-tertiary"
          >
            <Item
              value="new workspace"
              onSelect={() => {
                // Surface a window-level event so chat route's wizard
                // can open without prop-drilling state across routes.
                window.dispatchEvent(new CustomEvent("flockmux:open-wizard"));
                go("/chat");
              }}
              icon={<Plus className="size-4" />}
              label={t("cmdk.newWorkspace")}
              hint={t("cmdk.openWizard")}
            />
            <Item
              value="run spell"
              onSelect={() => {
                window.dispatchEvent(new CustomEvent("flockmux:open-wizard"));
                go("/chat");
              }}
              icon={<Sparkles className="size-4" />}
              label={t("cmdk.runSpell")}
              hint={t("cmdk.openWizard")}
            />
          </Command.Group>

          <Command.Group
            heading={t("cmdk.groups.settings")}
            className="mt-1 [&_[cmdk-group-heading]]:px-2 [&_[cmdk-group-heading]]:py-1.5 [&_[cmdk-group-heading]]:font-caption [&_[cmdk-group-heading]]:text-[10px] [&_[cmdk-group-heading]]:font-semibold [&_[cmdk-group-heading]]:uppercase [&_[cmdk-group-heading]]:tracking-wider [&_[cmdk-group-heading]]:text-foreground-tertiary"
          >
            {SETTINGS_SECTIONS.map((s, i) => {
              const label = t(s.labelKey);
              // cmdk filter only sees `value`; include both the english id
              // (typeable in any locale) and the translated label so e.g.
              // searching "隐私" or "privacy" both surface the same item.
              return (
                <Item
                  key={s.id}
                  value={`settings ${s.id} ${label}`}
                  onSelect={() => go(`/settings/${s.id}`)}
                  icon={<SettingsIcon className="size-4" />}
                  label={label}
                  hint={`⌘${i + 1}`}
                />
              );
            })}
          </Command.Group>
        </Command.List>
        <div className="flex items-center justify-between border-t border-border-subtle bg-surface-secondary px-3 py-2 font-caption text-[10px] text-foreground-tertiary">
          <span>
            <kbd className="rounded bg-surface-tertiary px-1.5 py-0.5">↑↓</kbd>{" "}
            {t("cmdk.kbd.navigate")}
            <kbd className="ml-2 rounded bg-surface-tertiary px-1.5 py-0.5">⏎</kbd>{" "}
            {t("cmdk.kbd.confirm")}
            <kbd className="ml-2 rounded bg-surface-tertiary px-1.5 py-0.5">Esc</kbd>{" "}
            {t("cmdk.kbd.close")}
          </span>
          <span className="font-mono">{t("cmdk.kbd.openHint")}</span>
        </div>
      </div>
    </Command.Dialog>
  );
}

// Update localStorage in the same shape Settings persists, then apply
// runtime. Avoids drift between palette and Settings UI.
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

function Item({
  value,
  onSelect,
  icon,
  label,
  hint,
}: {
  value?: string;
  onSelect: () => void;
  icon: React.ReactNode;
  label: string;
  hint?: string;
}) {
  return (
    <Command.Item
      value={value ?? label}
      onSelect={onSelect}
      className="flex cursor-pointer items-center gap-2.5 rounded-md px-2 py-2 text-sm text-foreground-secondary aria-selected:bg-accent-primary-soft aria-selected:text-foreground-primary"
    >
      <span className="text-foreground-tertiary">{icon}</span>
      <span className="flex-1">{label}</span>
      {hint && (
        <span className="font-caption text-[10px] text-foreground-tertiary">
          {hint}
        </span>
      )}
    </Command.Item>
  );
}
