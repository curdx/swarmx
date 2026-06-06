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

import React, { useEffect, useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { useTranslation } from "react-i18next";
import i18n from "@/i18n";
import { setTheme } from "@/lib/theme";
import { HTTP_BASE } from "@/lib/apiBase";
import { api } from "@/api/http";
import type { CliPluginInfo, ModelConfig, ModelsResponse } from "@/api/types";
import {
  Bell,
  CircleUser,
  Cpu,
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
import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import { Label } from "@/components/ui/label";
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
  { id: "models", labelKey: "settings.sections.models", icon: Cpu },
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

  // ⌘1..⌘6 (Ctrl+1..6 on non-mac) jumps to the nth section while
  // /settings is mounted. Skips ⌘K (palette) and modifier-only combos.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const hasModifier = e.metaKey || e.ctrlKey;
      if (!hasModifier || e.shiftKey || e.altKey) return;
      const n = parseInt(e.key, 10);
      if (!Number.isFinite(n) || n < 1 || n > SECTIONS.length) return;
      e.preventDefault();
      navigate(`/settings/${SECTIONS[n - 1].id}`);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [navigate]);

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
          {SECTIONS.map((s, i) => {
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
                <span className="flex-1">{t(s.labelKey)}</span>
                <kbd
                  className={cn(
                    "rounded border border-border-subtle px-1 font-mono text-[9px]",
                    active
                      ? "bg-surface-elevated text-foreground-secondary"
                      : "bg-surface-elevated text-foreground-tertiary",
                  )}
                >
                  ⌘{i + 1}
                </kbd>
              </button>
            );
          })}
          <div className="mt-auto px-3 pt-4 font-caption text-[10px] text-foreground-tertiary">
            <p className="font-mono">flockmux</p>
            <p>v{__APP_VERSION__}</p>
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
          {activeId === "shortcuts" && <ShortcutsPanel />}
          {activeId === "models" && <ModelsPanel />}
          {activeId === "plugins" && <PluginsPanel />}
          {activeId === "privacy" && <PrivacyPanel />}
          {activeId === "about" && <AboutPanel />}
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

// ── Shortcuts panel ─────────────────────────────────────────────────────

interface Binding {
  keys: string[];
  descKey: string;
}
interface Scope {
  id: "global" | "player" | "modal";
  bindings: Binding[];
}

const SHORTCUT_SCOPES: Scope[] = [
  {
    id: "global",
    bindings: [
      { keys: ["⌘", "K"], descKey: "settings.shortcuts.k.cmdk" },
      { keys: ["Esc"], descKey: "settings.shortcuts.k.esc" },
    ],
  },
  {
    id: "player",
    bindings: [
      { keys: ["␣"], descKey: "settings.shortcuts.k.playPause" },
      { keys: ["←", "/", "→"], descKey: "settings.shortcuts.k.skip" },
      { keys: ["."], descKey: "settings.shortcuts.k.frame" },
      { keys: ["Esc"], descKey: "settings.shortcuts.k.back" },
    ],
  },
];

function ShortcutsPanel() {
  const { t } = useTranslation();
  return (
    <div className="mx-auto flex max-w-2xl flex-col gap-8 p-8">
      <PanelTitle
        title={t("settings.shortcuts.title")}
        hint={t("settings.shortcuts.hint")}
      />
      {SHORTCUT_SCOPES.map((scope) => (
        <Field key={scope.id} label={t(`settings.shortcuts.scope.${scope.id}`)}>
          <div className="overflow-hidden rounded-lg border border-border-subtle">
            {scope.bindings.map((b, i) => (
              <div
                key={i}
                className={cn(
                  "flex items-center gap-4 px-4 py-3",
                  i > 0 && "border-t border-border-subtle",
                )}
              >
                <div className="flex flex-1 items-center gap-1.5">
                  {b.keys.map((k, j) => (
                    <kbd
                      key={j}
                      className="rounded border border-border-subtle bg-surface-elevated px-2 py-0.5 font-mono text-[11px] text-foreground-primary shadow-sm"
                    >
                      {k}
                    </kbd>
                  ))}
                </div>
                <span className="font-caption text-xs text-foreground-secondary">
                  {t(b.descKey)}
                </span>
              </div>
            ))}
          </div>
        </Field>
      ))}
    </div>
  );
}

// ── Models panel (F1: per-CLI tier→concrete-model mapping) ───────────────

const MODEL_TIERS = ["opus", "sonnet", "haiku"] as const;

function ModelsPanel() {
  const { t } = useTranslation();
  const [data, setData] = useState<ModelsResponse | null>(null);
  const [cfg, setCfg] = useState<ModelConfig | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [saved, setSaved] = useState(false);

  useEffect(() => {
    let cancelled = false;
    api
      .getModels()
      .then((r) => {
        if (!cancelled) {
          setData(r);
          setCfg(r.config);
        }
      })
      .catch((e) => {
        if (!cancelled) setError((e as Error).message);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const cliOf = (id: string) =>
    cfg?.clis[id] ?? { default: "", tiers: {}, effort: "" };
  const setDefault = (id: string, v: string) =>
    setCfg((prev) =>
      prev
        ? {
            ...prev,
            clis: {
              ...prev.clis,
              [id]: { ...(prev.clis[id] ?? { default: "", tiers: {} }), default: v },
            },
          }
        : prev,
    );
  const setTier = (id: string, tier: string, v: string) =>
    setCfg((prev) => {
      if (!prev) return prev;
      const c = prev.clis[id] ?? { default: "", tiers: {} };
      return {
        ...prev,
        clis: { ...prev.clis, [id]: { ...c, tiers: { ...c.tiers, [tier]: v } } },
      };
    });
  const setEffort = (id: string, v: string) =>
    setCfg((prev) => {
      if (!prev) return prev;
      const c = prev.clis[id] ?? { default: "", tiers: {} };
      return { ...prev, clis: { ...prev.clis, [id]: { ...c, effort: v } } };
    });

  const save = async () => {
    if (!cfg) return;
    setBusy(true);
    setError(null);
    setSaved(false);
    try {
      const r = await api.putModels(cfg);
      setCfg(r.config);
      setSaved(true);
      window.setTimeout(() => setSaved(false), 2500);
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="mx-auto flex max-w-2xl flex-col gap-6 p-8">
      <PanelTitle
        title={t("settings.models.title")}
        hint={t("settings.models.hint")}
      />
      {error && (
        <div className="rounded-md border border-state-danger/40 bg-status-danger-soft px-3 py-2 text-xs text-state-danger">
          {error}
        </div>
      )}
      {!data && !error && (
        <p className="font-caption text-sm text-foreground-tertiary">
          {t("common.loading")}
        </p>
      )}
      {data &&
        cfg &&
        data.clis.map((cli) => (
          <div
            key={cli.id}
            className="flex flex-col gap-4 rounded-lg border border-border-subtle bg-surface-elevated p-5"
          >
            <div className="flex items-center gap-2.5">
              <span className="flex size-8 items-center justify-center rounded-md bg-accent-primary-soft text-accent-primary-deep">
                <Cpu className="size-4" />
              </span>
              <div className="flex flex-col">
                <span className="font-heading text-sm font-semibold text-foreground-primary">
                  {cli.display_name}
                </span>
                <span className="font-mono text-[11px] text-foreground-tertiary">
                  {cli.id}
                </span>
              </div>
            </div>

            {!cli.supports_model ? (
              <p className="font-caption text-xs text-foreground-tertiary">
                {t("settings.models.noOverride")}
              </p>
            ) : (
              <div className="flex flex-col gap-3">
                <ModelRow
                  label={t("settings.models.default")}
                  placeholder={t("settings.models.cliDefaultPlaceholder")}
                  value={cliOf(cli.id).default}
                  onChange={(v) => setDefault(cli.id, v)}
                />
                {/* opus/sonnet/haiku rows ONLY for CLIs that natively have
                    those tiers (claude). codex (gpt-5.x) doesn't — showing them
                    there is a leaky abstraction, so it gets just default+effort. */}
                {cli.native_tiers &&
                  MODEL_TIERS.map((tier) => (
                    <ModelRow
                      key={tier}
                      label={tier}
                      placeholder={t("settings.models.tierPlaceholder")}
                      value={cliOf(cli.id).tiers[tier] ?? ""}
                      onChange={(v) => setTier(cli.id, tier, v)}
                      mono
                    />
                  ))}
                <div className="mt-1 h-px bg-border-subtle" />
                <EffortRow
                  label={t("settings.models.effort")}
                  hint={t("settings.models.effortHint")}
                  // codex's reasoning effort tops out at xhigh (极高); picking
                  // 最大/max silently downgrades server-side. The per-direction
                  // picker says so in its level descriptions — mirror that here
                  // so the global default isn't a silent surprise.
                  note={
                    cli.id === "codex"
                      ? t("settings.models.effortCodexNote")
                      : undefined
                  }
                  value={cliOf(cli.id).effort ?? ""}
                  onChange={(v) => setEffort(cli.id, v)}
                />
              </div>
            )}
          </div>
        ))}

      {data && cfg && (
        <div className="flex items-center gap-3">
          <Button onClick={save} disabled={busy} className="self-start">
            {busy ? t("common.loading") : t("settings.models.save")}
          </Button>
          {saved && (
            <span className="font-caption text-xs text-status-success">
              {t("settings.models.saved")}
            </span>
          )}
        </div>
      )}
    </div>
  );
}

function ModelRow({
  label,
  placeholder,
  value,
  onChange,
  mono,
}: {
  label: string;
  placeholder?: string;
  value: string;
  onChange: (v: string) => void;
  mono?: boolean;
}) {
  const id = React.useId();
  return (
    <div className="flex items-center gap-3">
      <Label
        htmlFor={id}
        className={cn(
          "w-20 shrink-0 text-sm text-foreground-secondary",
          mono && "font-mono",
        )}
      >
        {label}
      </Label>
      <input
        id={id}
        type="text"
        spellCheck={false}
        value={value}
        placeholder={placeholder}
        onChange={(e) => onChange(e.target.value)}
        className="flex-1 rounded-md border border-border-subtle bg-surface-primary px-3 py-1.5 font-mono text-xs text-foreground-primary outline-none focus:border-accent-primary"
      />
    </div>
  );
}

/** Default reasoning-effort selector for a CLI (a fixed-option dropdown, not a
 *  free-text model id). "" = the model's own default. */
function EffortRow({
  label,
  hint,
  note,
  value,
  onChange,
}: {
  label: string;
  hint?: string;
  /** Extra CLI-specific caveat shown below the hint (e.g. codex effort cap). */
  note?: string;
  value: string;
  onChange: (v: string) => void;
}) {
  const { t } = useTranslation();
  const id = React.useId();
  const LEVELS = ["low", "medium", "high", "xhigh", "max"] as const;
  return (
    <div className="flex items-start gap-3">
      <Label
        htmlFor={id}
        className="mt-1.5 w-20 shrink-0 text-sm text-foreground-secondary"
      >
        {label}
      </Label>
      <div className="flex flex-1 flex-col gap-1">
        <select
          id={id}
          value={value}
          onChange={(e) => onChange(e.target.value)}
          className="w-full rounded-md border border-border-subtle bg-surface-primary px-3 py-1.5 text-xs text-foreground-primary outline-none focus:border-accent-primary"
        >
          <option value="">{t("model.default")}</option>
          {LEVELS.map((lv) => (
            <option key={lv} value={lv}>
              {t(`model.effort.${lv}`)}
            </option>
          ))}
        </select>
        {hint && (
          <span className="font-caption text-[10px] text-foreground-tertiary">
            {hint}
          </span>
        )}
        {note && (
          <span className="font-caption text-[10px] text-state-warning">
            {note}
          </span>
        )}
      </div>
    </div>
  );
}

// ── Plugins panel ───────────────────────────────────────────────────────

function PluginsPanel() {
  const { t } = useTranslation();
  const [items, setItems] = useState<CliPluginInfo[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    api
      .listPlugins()
      .then((rows) => {
        if (!cancelled) setItems(rows);
      })
      .catch((e) => {
        if (!cancelled) setError((e as Error).message);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  return (
    <div className="mx-auto flex max-w-2xl flex-col gap-6 p-8">
      <PanelTitle
        title={t("settings.plugins.title")}
        hint={t("settings.plugins.hint")}
      />
      {error && (
        <div className="rounded-md border border-state-danger/40 bg-status-danger-soft px-3 py-2 text-xs text-state-danger">
          {t("settings.plugins.loadError", { error })}
        </div>
      )}
      {items === null && !error && (
        <p className="font-caption text-sm text-foreground-tertiary">
          {t("common.loading")}
        </p>
      )}
      {items?.length === 0 && (
        <p className="font-caption text-sm text-foreground-tertiary">
          {t("settings.plugins.empty")}
        </p>
      )}
      {items && items.length > 0 && (
        <ul className="flex flex-col gap-2.5">
          {items.map((p) => (
            <li
              key={p.id}
              className="flex items-center gap-3 rounded-lg border border-border-subtle bg-surface-elevated p-3.5"
            >
              <span className="flex size-9 items-center justify-center rounded-md bg-accent-primary-soft text-accent-primary-deep">
                <Plug className="size-4" />
              </span>
              <div className="flex min-w-0 flex-1 flex-col">
                <span className="font-heading text-sm font-semibold text-foreground-primary">
                  {p.display_name}
                </span>
                <span className="font-caption text-[11px] text-foreground-tertiary">
                  <span className="font-mono">{p.id}</span> ·{" "}
                  {t("settings.plugins.binaryLabel")}{" "}
                  <span className="font-mono">{p.binary}</span>
                </span>
              </div>
              <span className="rounded-full bg-status-success-soft px-2.5 py-0.5 font-caption text-[10px] text-status-success">
                {t("settings.plugins.managedTag")}
              </span>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

// ── Privacy panel ───────────────────────────────────────────────────────

function flockmuxLocalStorageKeys(): string[] {
  const keys: string[] = [];
  for (let i = 0; i < window.localStorage.length; i++) {
    const k = window.localStorage.key(i);
    if (k && k.startsWith("flockmux:")) keys.push(k);
  }
  return keys;
}

function PrivacyPanel() {
  const { t } = useTranslation();
  const [toast, setToast] = useState<string | null>(null);

  const exportJson = () => {
    const data: Record<string, unknown> = {};
    for (const k of flockmuxLocalStorageKeys()) {
      const raw = window.localStorage.getItem(k);
      if (raw == null) continue;
      try {
        data[k] = JSON.parse(raw);
      } catch {
        data[k] = raw;
      }
    }
    const blob = new Blob([JSON.stringify(data, null, 2)], {
      type: "application/json",
    });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = `flockmux-local-${new Date().toISOString().slice(0, 10)}.json`;
    document.body.appendChild(a);
    a.click();
    a.remove();
    URL.revokeObjectURL(url);
  };

  const clearAll = () => {
    // eslint-disable-next-line no-alert
    if (!window.confirm(t("settings.privacy.clearConfirm"))) return;
    const keys = flockmuxLocalStorageKeys();
    for (const k of keys) window.localStorage.removeItem(k);
    setToast(t("settings.privacy.cleared", { count: keys.length }));
    window.setTimeout(() => setToast(null), 3000);
  };

  return (
    <div className="mx-auto flex max-w-2xl flex-col gap-8 p-8">
      <PanelTitle
        title={t("settings.privacy.title")}
        hint={t("settings.privacy.hint")}
      />

      <Field
        label={t("settings.privacy.approvalMode")}
        hint={t("settings.privacy.approvalModeHint")}
      >
        <div className="flex items-center gap-3 rounded-lg border border-status-warning/40 bg-status-warning-soft px-4 py-3">
          <Shield className="size-4 shrink-0 text-status-warning" />
          <span className="font-mono text-xs text-foreground-primary">
            {t("settings.privacy.approvalCurrent")}
          </span>
        </div>
      </Field>

      <Field
        label={t("settings.privacy.exportTitle")}
        hint={t("settings.privacy.exportHint")}
      >
        <Button variant="outline" onClick={exportJson} className="self-start">
          {t("settings.privacy.exportButton")}
        </Button>
      </Field>

      <Field
        label={t("settings.privacy.clearTitle")}
        hint={t("settings.privacy.clearHint")}
      >
        <Button variant="destructive" onClick={clearAll} className="self-start">
          {t("settings.privacy.clearButton")}
        </Button>
      </Field>

      {toast && (
        <div className="rounded-md border border-status-success-soft bg-status-success-soft px-3 py-2 text-xs text-status-success">
          {toast}
        </div>
      )}
    </div>
  );
}

// ── About panel ─────────────────────────────────────────────────────────

interface CrateInfo {
  name: string;
  desc: string;
}
const CRATES: CrateInfo[] = [
  { name: "flockmux-server", desc: "axum HTTP/WS gateway · :7777" },
  { name: "flockmux-pty", desc: "portable-pty bridge + WebSocket frame protocol" },
  { name: "flockmux-shim", desc: "wraps claude/codex CLIs · injects hooks + MCP" },
  { name: "flockmux-mcp", desc: "MCP server exposed to each agent (swarm bridge)" },
  { name: "flockmux-swarm", desc: "blackboard + mailbox + wake coordinator" },
  { name: "flockmux-storage", desc: "rusqlite-backed message/recording/event store" },
  { name: "flockmux-recorder", desc: "asciicast v2 writer for every PTY" },
  { name: "flockmux-protocol", desc: "wire types shared client/server/shim" },
  { name: "flockmux-cli", desc: "(stub) future `flockmux up` launcher" },
];

const DEPS: { name: string; ver: string; what: string }[] = [
  { name: "react", ver: "18.3", what: "UI runtime" },
  { name: "react-router-dom", ver: "6.30", what: "routing" },
  { name: "tailwindcss", ver: "4.x", what: "styling (CSS-first @theme)" },
  { name: "react-i18next", ver: "17.0", what: "i18n" },
  { name: "react-markdown", ver: "10.x", what: "Context Board renderer" },
  { name: "@xyflow/react", ver: "12.x", what: "DAG canvas" },
  { name: "@dagrejs/dagre", ver: "3.x", what: "DAG auto-layout" },
  { name: "cmdk", ver: "latest", what: "command palette (⌘K)" },
  { name: "asciinema-player", ver: "3.15", what: "replay player" },
  { name: "@xterm/xterm", ver: "5.5", what: "terminal (Agent Drawer / Debug)" },
  { name: "tauri 2.x", ver: "2.11", what: "desktop shell" },
];

function AboutPanel() {
  const { t } = useTranslation();
  return (
    <div className="mx-auto flex max-w-2xl flex-col gap-8 p-8">
      <PanelTitle
        title={t("settings.about.title")}
        hint={t("settings.about.hint")}
      />

      <section className="flex items-center gap-4 rounded-lg border border-border-subtle bg-surface-elevated p-5">
        <span className="flex size-14 items-center justify-center rounded-lg bg-accent-primary text-foreground-on-accent">
          <Info className="size-6" />
        </span>
        <div className="flex flex-col gap-0.5">
          <span className="font-heading text-lg font-bold text-foreground-primary">
            {t("settings.about.appName")}
          </span>
          <span className="font-caption text-xs text-foreground-tertiary">
            {t("settings.about.version", { v: __APP_VERSION__ })}
          </span>
          <a
            href={`https://${t("settings.about.repoUrl")}`}
            target="_blank"
            rel="noreferrer"
            className="mt-1 font-mono text-xs text-accent-primary hover:underline"
          >
            {t("settings.about.repo")}: {t("settings.about.repoUrl")}
          </a>
        </div>
      </section>

      <Field label={t("settings.about.endpointTitle")} hint={t("settings.about.endpointHint")}>
        <code className="inline-block rounded border border-border-subtle bg-surface-tertiary px-2 py-1 font-mono text-xs text-foreground-primary">
          {/* Derive from the live connection instead of hardcoding 7777 — a
              FLOCKMUX_PORT instance (or vite proxy) is no longer mislabeled. */}
          {HTTP_BASE ? HTTP_BASE.replace(/^https?:\/\//, "") : window.location.host}
        </code>
      </Field>

      <Field label={t("settings.about.cratesTitle")} hint={t("settings.about.cratesHint")}>
        <ul className="grid grid-cols-1 gap-1.5 sm:grid-cols-2">
          {CRATES.map((c) => (
            <li
              key={c.name}
              className="flex flex-col rounded border border-border-subtle bg-surface-elevated px-3 py-2"
            >
              <span className="font-mono text-xs font-semibold text-foreground-primary">
                {c.name}
              </span>
              <span className="font-caption text-[11px] text-foreground-tertiary">
                {c.desc}
              </span>
            </li>
          ))}
        </ul>
      </Field>

      <Field label={t("settings.about.depsTitle")}>
        <ul className="flex flex-col gap-1">
          {DEPS.map((d) => (
            <li
              key={d.name}
              className="flex items-baseline gap-2 px-3 py-1.5 font-caption text-xs"
            >
              <span className="font-mono font-semibold text-foreground-primary">
                {d.name}
              </span>
              <span className="font-mono text-foreground-tertiary">{d.ver}</span>
              <span className="flex-1" />
              <span className="text-foreground-secondary">{d.what}</span>
            </li>
          ))}
        </ul>
      </Field>
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
  const { t } = useTranslation();
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
          {t("settings.appearance.preview")}
        </span>
      </div>
      <div className="flex items-center gap-2 px-3 py-2">
        <span className="text-foreground-tertiary">{icon}</span>
        <span className="font-heading text-sm font-medium text-foreground-primary">
          {label}
        </span>
        {active && (
          <span className="ml-auto rounded-full bg-accent-primary px-2 py-0.5 font-caption text-[9px] text-foreground-on-accent">
            {t("settings.appearance.current")}
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
  // 用 React.useId 给 Switch + Label 配对，可访问性 + 点 Label 也能切 Switch。
  const id = React.useId();
  return (
    <div className="flex items-center gap-3 rounded-lg border border-border-subtle bg-surface-elevated px-4 py-3">
      <div className="flex min-w-0 flex-1 flex-col">
        <Label
          htmlFor={id}
          className="cursor-pointer font-heading text-sm font-medium text-foreground-primary"
        >
          {label}
        </Label>
        {hint && (
          <span className="font-caption text-[11px] text-foreground-tertiary">
            {hint}
          </span>
        )}
      </div>
      <Switch id={id} checked={value} onCheckedChange={onChange} />
    </div>
  );
}

// silence unused-icon imports we'll wire up later
void CircleUser;
void Bell;
void Layers;
