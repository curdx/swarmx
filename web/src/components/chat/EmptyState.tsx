/**
 * EmptyState — the honest, helpful empty-room canvas (P0-8).
 *
 * Replaces the bare "暂无消息" when a room has no messages AND isn't in a
 * startup (BootstrapChecklistCard) or failure (OrchestratorFailureCard) state.
 * It does three jobs the bare text didn't:
 *   - sets expectations ("send and the captain comes online, plans, delegates")
 *   - offers 3 tappable starter prompts that FILL the composer (never auto-send)
 *   - shows an honest engine pre-check (✓ logged in / ✕ not installed), never a
 *     fake green — if no engine is usable it says so and links to setup.
 *
 * Rendered by MessagesPanel (not via Chat's emptyStateOverride) so a starter
 * click can reach the composer's setBody directly.
 */
import { useTranslation } from "react-i18next";
import { Link } from "react-router-dom";
import {
  Activity,
  CircleAlert,
  CircleCheck,
  CircleDashed,
  CircleX,
  Loader2,
  PlugZap,
  RefreshCw,
  ShieldCheck,
  Sparkles,
} from "lucide-react";
import type { EngineReadiness } from "../../api/types";
import { evidenceOf, EVIDENCE_I18N } from "../../lib/engineEvidence";

export interface EmptyStateCliReadiness {
  loading: boolean;
  /** A real-usability sweep is running right now. */
  probing: boolean;
  engines: EngineReadiness[];
  /** Kick a real-usability sweep (actually start each engine). */
  onProbe: () => void;
}

interface StarterDef {
  key: string;
  fallback: string;
}

const STARTERS: StarterDef[] = [
  // 第一条必须是「看得见 swarm 的」:队长派 worker、阶段条、黑板、交付
  // 信号全部跑一遍 —— 30 秒讲清这个产品和单 agent 工具的区别。
  { key: "chat.emptyState.starter.swarm", fallback: "派一个 worker 看看这个目录里有什么,它汇报给你后你再总结给我" },
  { key: "chat.emptyState.starter.explore", fallback: "看一下这个项目的目录结构,用中文讲讲每部分是干什么的" },
  { key: "chat.emptyState.starter.bug", fallback: "帮我查这个 bug 的根因" },
];

export function EmptyState({
  cliReadiness,
  onPickStarter,
}: {
  cliReadiness: EmptyStateCliReadiness;
  onPickStarter: (text: string) => void;
}) {
  const { t } = useTranslation();
  const { loading, probing, engines, onProbe } = cliReadiness;
  // "No engine" = nothing is even installed → hide starters and point to setup.
  const noEngine =
    !loading && engines.length > 0 && engines.every((e) => !e.installed);
  // Offer a usability check once at least one engine is installed but we don't
  // yet have a fresh "usable" verdict for all of them.
  const canProbe =
    !loading &&
    engines.some((e) => e.installed) &&
    engines.some((e) => e.state === "unknown" || e.state === "not_usable" || e.state === "needs_login");

  return (
    <div className="mx-auto mt-12 flex w-full max-w-[460px] flex-col gap-6">
      {/* greeting */}
      <div className="flex flex-col gap-1.5">
        <h2 className="font-heading text-base font-semibold text-foreground">
          {t("chat.emptyState.greetingTitle", "把要做的事交给队长")}
        </h2>
        <p className="font-caption text-xs leading-6 text-foreground-secondary">
          {t(
            "chat.emptyState.greetingBody",
            "说出你想做的事，发送后队长会上线，拆成计划、派给成员推进，并在这里向你汇报。",
          )}
        </p>
      </div>

      {/* starter prompts — fill the composer, never auto-send */}
      {!noEngine && (
        <div className="flex flex-col gap-2">
          <p className="font-caption text-[10px] uppercase tracking-wider text-foreground-tertiary">
            {t("chat.emptyState.starterHint", "试试这样开始：")}
          </p>
          {STARTERS.map((s) => {
            const text = t(s.key, s.fallback);
            return (
              <button
                key={s.key}
                type="button"
                onClick={() => onPickStarter(text)}
                className="group flex items-center gap-2.5 rounded-xl border border-border-subtle bg-surface-secondary px-3 py-2 text-left transition-colors hover:border-accent-primary/40 hover:bg-accent-primary-soft/40"
              >
                <Sparkles className="size-3.5 shrink-0 text-foreground-tertiary transition-colors group-hover:text-accent-primary" />
                <span className="min-w-0 flex-1 font-body text-[13px] text-foreground-primary">
                  {text}
                </span>
              </button>
            );
          })}
        </div>
      )}

      {/* engine pre-check — honest, never a fake green. "Installed" is shown
          neutrally as "未验证" until a real-usability probe confirms it can run. */}
      <div className="flex flex-col gap-2 rounded-xl border border-border-subtle bg-surface-primary px-3 py-2.5">
        <div className="flex items-center justify-between gap-2">
          <p className="flex items-center gap-1.5 font-caption text-[10px] uppercase tracking-wider text-foreground-tertiary">
            <PlugZap className="size-3" />
            {t("chat.emptyState.precheckTitle", "AI 引擎")}
          </p>
          {(canProbe || probing) && (
            <button
              type="button"
              onClick={onProbe}
              disabled={probing}
              className="inline-flex items-center gap-1 font-caption text-[10px] text-foreground-tertiary transition-colors hover:text-foreground-secondary disabled:opacity-60"
            >
              {probing ? (
                <Loader2 className="size-3 animate-spin" />
              ) : (
                <RefreshCw className="size-3" />
              )}
              {probing
                ? t("chat.emptyState.probing", "检测中…")
                : t("chat.emptyState.probe", "检测可用性")}
            </button>
          )}
        </div>
        {loading ? (
          <span className="flex items-center gap-1.5 font-caption text-[11px] text-foreground-tertiary">
            <Loader2 className="size-3 animate-spin" />
            {t("chat.emptyState.precheckLoading", "正在检查引擎…")}
          </span>
        ) : (
          <div className="flex flex-col gap-1">
            {engines.map((e) => (
              <EngineRow key={e.id} engine={e} />
            ))}
          </div>
        )}
        {noEngine && (
          <div className="mt-1 flex flex-col gap-1.5">
            <p className="font-caption text-[11px] leading-5 text-state-warning">
              {t(
                "chat.emptyState.noEngineBody",
                "需要先装好并登录至少一个 AI 引擎，队长才能上岗。",
              )}
            </p>
            <Link
              to="/settings/plugins"
              className="inline-flex w-fit items-center gap-1.5 rounded-md border border-border-subtle px-2.5 py-1 font-caption text-[11px] text-foreground-secondary transition-colors hover:bg-surface-tertiary"
            >
              <PlugZap className="size-3.5" />
              {t("chat.emptyState.setupEngine", "去安装 AI 引擎")}
            </Link>
          </div>
        )}
      </div>
    </div>
  );
}

/** One engine's honest readiness line. The probe verdict drives icon + color;
 *  an installed-but-unprobed engine is neutral "未验证", never a green check. */
function EngineRow({ engine }: { engine: EngineReadiness }) {
  const { t } = useTranslation();
  const { display_name: name, state, reason } = engine;

  const variants = {
    usable: {
      icon: <CircleCheck className="size-3.5 text-status-success" />,
      cls: "text-foreground-secondary",
      text: t("chat.emptyState.engineUsable", { name, defaultValue: "{{name}} 可用" }),
    },
    unknown: {
      icon: <PlugZap className="size-3.5 text-foreground-tertiary" />,
      cls: "text-foreground-tertiary",
      text: t("chat.emptyState.engineUnverified", {
        name,
        defaultValue: "{{name}} 已安装（未验证）",
      }),
    },
    needs_login: {
      icon: <CircleAlert className="size-3.5 text-state-warning" />,
      cls: "text-state-warning",
      text: t("chat.emptyState.engineNeedsLogin", {
        name,
        defaultValue: "{{name}} 需登录",
      }),
    },
    not_usable: {
      icon: <CircleX className="size-3.5 text-state-danger" />,
      cls: "text-state-danger",
      text: t("chat.emptyState.engineNotUsable", {
        name,
        defaultValue: "{{name}} 无法启动",
      }),
    },
    not_installed: {
      icon: <CircleX className="size-3.5 text-foreground-tertiary" />,
      cls: "text-foreground-tertiary",
      text: t("chat.emptyState.engineMissing", {
        name,
        defaultValue: "{{name}} 未安装",
      }),
    },
  } as const;

  const v = variants[state];
  return (
    <span
      className={`flex items-center gap-1.5 font-caption text-[11px] ${v.cls}`}
      title={reason ?? undefined}
    >
      {v.icon}
      {v.text}
      <EngineEvidence engine={engine} />
    </span>
  );
}

/** Compact evidence marker for a "usable" engine in the empty-room precheck:
 *  a real one-turn check (已验证回合) reads as a shield, live use as activity,
 *  launch-only as a dashed circle — so "可用" shows how it was proven. */
function EngineEvidence({ engine }: { engine: EngineReadiness }) {
  const { t } = useTranslation();
  const ev = evidenceOf(engine);
  if (ev === "none") return null;
  const meta = {
    verified: {
      icon: <ShieldCheck className="size-3 text-status-success" />,
      cls: "text-status-success",
    },
    live: {
      icon: <Activity className="size-3 text-accent-primary" />,
      cls: "text-accent-primary-deep",
    },
    launch: {
      icon: <CircleDashed className="size-3 text-foreground-tertiary" />,
      cls: "text-foreground-tertiary",
    },
  }[ev];
  return (
    <span
      className={`ml-0.5 inline-flex items-center gap-0.5 ${meta.cls}`}
      title={t(EVIDENCE_I18N[ev].detail)}
    >
      {meta.icon}
      {t(EVIDENCE_I18N[ev].label)}
    </span>
  );
}
