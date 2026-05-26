/**
 * Preferences — Pencil frame nJqkA.
 *
 * Left nav (200) + right detail. Persistence is localStorage only —
 * flockmux-storage has no kv settings table yet; the existing crates
 * own session state, not user prefs. When that table lands we'll
 * promote read/write to api.getSettings / api.putSettings without
 * changing this surface.
 *
 * Sections beyond 通用 are scaffolded but only General is fully wired —
 * the visual goal is "settings page exists and looks right", not a full
 * preference matrix.
 */

import { useEffect, useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { useTranslation } from "react-i18next";
import i18n from "@/i18n";
import { setTheme } from "@/lib/theme";
import {
  Bell,
  CircleUser,
  Globe,
  Info,
  Keyboard,
  Layers,
  Moon,
  Palette,
  Plug,
  Settings as SettingsIcon,
  Shield,
  Sun,
  SunMoon,
} from "lucide-react";
import { cn } from "@/lib/cn";

const STORAGE_KEY = "flockmux:settings:v1";

type Lang = "zh" | "en";
type Theme = "light" | "dark" | "system";

interface SettingsState {
  lang: Lang;
  theme: Theme;
  openMainOnLaunch: boolean;
  desktopNotify: boolean;
  killOthersOnFail: boolean;
}

const DEFAULTS: SettingsState = {
  lang: "zh",
  theme: "light",
  openMainOnLaunch: true,
  desktopNotify: true,
  killOthersOnFail: false,
};

function loadSettings(): SettingsState {
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    if (!raw) return DEFAULTS;
    return { ...DEFAULTS, ...JSON.parse(raw) };
  } catch {
    return DEFAULTS;
  }
}

function saveSettings(s: SettingsState) {
  try {
    window.localStorage.setItem(STORAGE_KEY, JSON.stringify(s));
  } catch {
    /* ignore quota errors */
  }
}

const SECTIONS = [
  { id: "general", labelKey: "settings.sections.general", icon: SettingsIcon },
  { id: "appearance", labelKey: "settings.sections.appearance", icon: Palette },
  { id: "shortcuts", labelKey: "settings.sections.shortcuts", icon: Keyboard },
  { id: "plugins", labelKey: "settings.sections.plugins", icon: Plug },
  { id: "privacy", labelKey: "settings.sections.privacy", icon: Shield },
  { id: "about", labelKey: "settings.sections.about", icon: Info },
] as const;

type SectionId = (typeof SECTIONS)[number]["id"];

export default function SettingsRoute() {
  const { t } = useTranslation();
  const { section } = useParams<{ section?: string }>();
  const navigate = useNavigate();
  const [settings, setSettings] = useState<SettingsState>(loadSettings);

  const activeId = (SECTIONS.find((s) => s.id === section)?.id ??
    "general") as SectionId;

  useEffect(() => {
    saveSettings(settings);
    // Runtime side-effects: theme flips data-theme; lang swaps i18n
    // resources. Everything else is passive (read by other code paths).
    setTheme(settings.theme);
    if (i18n.language !== settings.lang) {
      i18n.changeLanguage(settings.lang);
    }
  }, [settings]);

  const update = <K extends keyof SettingsState>(k: K, v: SettingsState[K]) =>
    setSettings((prev) => ({ ...prev, [k]: v }));

  return (
    <div className="flex h-full flex-col bg-surface-primary">
      {/* Header */}
      <header className="flex h-14 shrink-0 items-center gap-3 border-b border-border-subtle bg-surface-elevated px-5">
        <span className="flex size-8 items-center justify-center rounded-md bg-accent-primary-soft">
          <SettingsIcon className="size-4 text-accent-primary-deep" />
        </span>
        <div className="flex flex-col">
          <h1 className="font-heading text-sm font-semibold text-foreground-primary">
            {t("settings.title")}
          </h1>
          <span className="font-caption text-[10px] text-foreground-tertiary">
            {t("settings.subtitle")}
          </span>
        </div>
      </header>

      {/* Body */}
      <div className="flex min-h-0 flex-1">
        {/* Nav */}
        <aside className="flex w-[200px] shrink-0 flex-col gap-1 border-r border-border-subtle bg-surface-secondary p-3">
          {SECTIONS.map((s) => {
            const Icon = s.icon;
            const active = s.id === activeId;
            return (
              <button
                key={s.id}
                onClick={() => navigate(`/settings/${s.id}`)}
                className={cn(
                  "flex items-center gap-2.5 rounded-md px-3 py-2 text-left text-sm",
                  active
                    ? "bg-accent-primary-soft text-foreground-primary"
                    : "text-foreground-secondary hover:bg-surface-tertiary",
                )}
              >
                <Icon className="size-4" />
                {t(s.labelKey)}
              </button>
            );
          })}
          <div className="mt-auto px-3 pt-4 font-caption text-[10px] text-foreground-tertiary">
            <p className="font-mono">flockmux</p>
            <p>v0.1 (M6h · UI/C.3)</p>
          </div>
        </aside>

        {/* Detail */}
        <section className="min-h-0 flex-1 overflow-y-auto">
          {activeId === "general" && (
            <GeneralPanel settings={settings} update={update} />
          )}
          {activeId === "appearance" && (
            <AppearancePanel settings={settings} update={update} />
          )}
          {activeId !== "general" && activeId !== "appearance" && (
            <StubPanel id={activeId} />
          )}
        </section>
      </div>
    </div>
  );
}

// ── Sections ─────────────────────────────────────────────────────────────

function GeneralPanel({
  settings,
  update,
}: {
  settings: SettingsState;
  update: <K extends keyof SettingsState>(k: K, v: SettingsState[K]) => void;
}) {
  const { t } = useTranslation();
  return (
    <div className="mx-auto flex max-w-2xl flex-col gap-8 p-8">
      <PanelTitle
        title={t("settings.general.title")}
        hint={t("settings.general.hint")}
      />

      <Field
        label={t("settings.general.lang")}
        hint={t("settings.general.langHint")}
      >
        <div className="grid grid-cols-2 gap-3">
          <ChoiceCard
            active={settings.lang === "zh"}
            onClick={() => update("lang", "zh")}
            title="中文"
            sub="简体中文"
            icon={<Globe className="size-5" />}
          />
          <ChoiceCard
            active={settings.lang === "en"}
            onClick={() => update("lang", "en")}
            title="English"
            sub="US English"
            icon={<Globe className="size-5" />}
          />
        </div>
      </Field>

      <Field
        label={t("settings.general.launch")}
        hint={t("settings.general.launchHint")}
      >
        <div className="flex flex-col gap-3">
          <ToggleRow
            label={t("settings.general.openMain")}
            hint={t("settings.general.openMainHint")}
            value={settings.openMainOnLaunch}
            onChange={(v) => update("openMainOnLaunch", v)}
          />
          <ToggleRow
            label={t("settings.general.desktopNotify")}
            hint={t("settings.general.desktopNotifyHint")}
            value={settings.desktopNotify}
            onChange={(v) => update("desktopNotify", v)}
          />
        </div>
      </Field>

      <Field
        label={t("settings.general.failure")}
        hint={t("settings.general.failureHint")}
      >
        <ToggleRow
          label={t("settings.general.killOthers")}
          hint={t("settings.general.killOthersHint")}
          value={settings.killOthersOnFail}
          onChange={(v) => update("killOthersOnFail", v)}
        />
      </Field>
    </div>
  );
}

function AppearancePanel({
  settings,
  update,
}: {
  settings: SettingsState;
  update: <K extends keyof SettingsState>(k: K, v: SettingsState[K]) => void;
}) {
  const { t } = useTranslation();
  return (
    <div className="mx-auto flex max-w-2xl flex-col gap-8 p-8">
      <PanelTitle
        title={t("settings.appearance.title")}
        hint={t("settings.appearance.hint")}
      />
      <Field
        label={t("settings.appearance.theme")}
        hint={t("settings.appearance.themeHint")}
      >
        <div className="grid grid-cols-3 gap-3">
          <ThemeCard
            active={settings.theme === "light"}
            onClick={() => update("theme", "light")}
            label={t("settings.appearance.themes.light")}
            preview="light"
            icon={<Sun className="size-4" />}
          />
          <ThemeCard
            active={settings.theme === "dark"}
            onClick={() => update("theme", "dark")}
            label={t("settings.appearance.themes.dark")}
            preview="dark"
            icon={<Moon className="size-4" />}
          />
          <ThemeCard
            active={settings.theme === "system"}
            onClick={() => update("theme", "system")}
            label={t("settings.appearance.themes.system")}
            preview="system"
            icon={<SunMoon className="size-4" />}
          />
        </div>
      </Field>
    </div>
  );
}

function StubPanel({ id }: { id: SectionId }) {
  const { t } = useTranslation();
  const meta = SECTIONS.find((s) => s.id === id)!;
  const Icon = meta.icon;
  return (
    <div className="mx-auto flex h-full max-w-2xl flex-col items-center justify-center gap-4 p-8 text-center text-foreground-tertiary">
      <span className="flex size-14 items-center justify-center rounded-full bg-surface-tertiary">
        <Icon className="size-7" />
      </span>
      <h2 className="font-heading text-lg font-semibold text-foreground-secondary">
        {t(meta.labelKey)}
      </h2>
      <p className="max-w-sm font-caption text-sm">
        本节占位。后续会在这里铺
        {id === "shortcuts"
          ? "可编辑的快捷键列表"
          : id === "plugins"
            ? "CLI 插件列表 + 启用/禁用 + 路径覆盖"
            : id === "privacy"
              ? "审批门禁、PTY 日志保留时长、本地数据导出"
              : "版本号、依赖 crate 清单、致谢"}
        。
      </p>
    </div>
  );
}

// ── Atoms ────────────────────────────────────────────────────────────────

function PanelTitle({ title, hint }: { title: string; hint: string }) {
  return (
    <div className="flex flex-col gap-1">
      <h2 className="font-heading text-xl font-bold text-foreground-primary">
        {title}
      </h2>
      <p className="font-caption text-sm text-foreground-tertiary">{hint}</p>
    </div>
  );
}

function Field({
  label,
  hint,
  children,
}: {
  label: string;
  hint?: string;
  children: React.ReactNode;
}) {
  return (
    <div className="flex flex-col gap-2">
      <div className="flex items-baseline gap-2">
        <span className="font-heading text-sm font-semibold text-foreground-primary">
          {label}
        </span>
        {hint && (
          <span className="font-caption text-xs text-foreground-tertiary">
            {hint}
          </span>
        )}
      </div>
      {children}
    </div>
  );
}

function ChoiceCard({
  active,
  onClick,
  title,
  sub,
  icon,
}: {
  active: boolean;
  onClick: () => void;
  title: string;
  sub: string;
  icon: React.ReactNode;
}) {
  return (
    <button
      onClick={onClick}
      className={cn(
        "flex items-center gap-3 rounded-lg border-[1.5px] bg-surface-elevated px-4 py-3 text-left transition-colors",
        active
          ? "border-accent-primary bg-surface-accent-tint"
          : "border-border-subtle hover:border-border-strong",
      )}
    >
      <span
        className={cn(
          "flex size-8 items-center justify-center rounded-md",
          active
            ? "bg-accent-primary text-foreground-on-accent"
            : "bg-surface-tertiary text-foreground-secondary",
        )}
      >
        {icon}
      </span>
      <div className="flex flex-col">
        <span className="font-heading text-sm font-semibold text-foreground-primary">
          {title}
        </span>
        <span className="font-caption text-[11px] text-foreground-tertiary">
          {sub}
        </span>
      </div>
    </button>
  );
}

function ThemeCard({
  active,
  onClick,
  label,
  preview,
  icon,
}: {
  active: boolean;
  onClick: () => void;
  label: string;
  preview: "light" | "dark" | "system";
  icon: React.ReactNode;
}) {
  return (
    <button
      onClick={onClick}
      className={cn(
        "flex flex-col gap-2 overflow-hidden rounded-lg border-[1.5px] transition-colors",
        active ? "border-accent-primary" : "border-border-subtle hover:border-border-strong",
      )}
    >
      <div
        className={cn(
          "flex h-24 items-end justify-end p-2",
          preview === "light" && "bg-gradient-to-b from-surface-elevated to-surface-secondary",
          preview === "dark" && "bg-gradient-to-b from-[#1F1F1F] to-[#0A0A0A]",
          preview === "system" &&
            "bg-[linear-gradient(135deg,var(--color-surface-secondary)_0%,var(--color-surface-secondary)_50%,#1A1A1A_50%,#0A0A0A_100%)]",
        )}
      >
        <span
          className={cn(
            "rounded px-1.5 py-0.5 font-caption text-[9px]",
            preview === "light"
              ? "bg-surface-elevated text-foreground-secondary"
              : preview === "dark"
                ? "bg-[#1F1F1F] text-foreground-inverse-secondary"
                : "bg-black/50 text-white",
          )}
        >
          Preview
        </span>
      </div>
      <div className="flex items-center gap-2 px-3 py-2">
        <span className="text-foreground-tertiary">{icon}</span>
        <span className="font-heading text-sm font-medium text-foreground-primary">
          {label}
        </span>
        {active && (
          <span className="ml-auto rounded-full bg-accent-primary px-2 py-0.5 font-caption text-[9px] text-foreground-on-accent">
            当前
          </span>
        )}
      </div>
    </button>
  );
}

function ToggleRow({
  label,
  hint,
  value,
  onChange,
}: {
  label: string;
  hint?: string;
  value: boolean;
  onChange: (v: boolean) => void;
}) {
  return (
    <div className="flex items-center gap-3 rounded-lg border border-border-subtle bg-surface-elevated px-4 py-3">
      <div className="flex min-w-0 flex-1 flex-col">
        <span className="font-heading text-sm font-medium text-foreground-primary">
          {label}
        </span>
        {hint && (
          <span className="font-caption text-[11px] text-foreground-tertiary">
            {hint}
          </span>
        )}
      </div>
      <button
        onClick={() => onChange(!value)}
        className={cn(
          "relative h-6 w-11 shrink-0 rounded-full transition-colors",
          value ? "bg-accent-primary" : "bg-surface-tertiary",
        )}
        aria-pressed={value}
      >
        <span
          className={cn(
            "absolute top-0.5 size-5 rounded-full bg-surface-elevated shadow-sm transition-transform",
            value ? "translate-x-[22px]" : "translate-x-0.5",
          )}
        />
      </button>
    </div>
  );
}

// silence unused-icon imports we'll wire up later
void CircleUser;
void Bell;
void Layers;
