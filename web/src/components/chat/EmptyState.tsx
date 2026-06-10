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
  CircleCheck,
  CircleX,
  Loader2,
  PlugZap,
  Sparkles,
} from "lucide-react";
import type { CliPluginInfo } from "../../api/types";

export interface EmptyStateCliReadiness {
  loading: boolean;
  installed: CliPluginInfo[];
  missing: CliPluginInfo[];
}

interface StarterDef {
  key: string;
  fallback: string;
}

const STARTERS: StarterDef[] = [
  { key: "chat.emptyState.starter.refactor", fallback: "重构这个函数，抽出可复用逻辑" },
  { key: "chat.emptyState.starter.test", fallback: "给这段代码补上失败用例的测试" },
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
  const noEngine =
    !cliReadiness.loading &&
    cliReadiness.installed.length === 0 &&
    cliReadiness.missing.length > 0;

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

      {/* engine pre-check — honest, never a fake green */}
      <div className="flex flex-col gap-2 rounded-xl border border-border-subtle bg-surface-primary px-3 py-2.5">
        <p className="flex items-center gap-1.5 font-caption text-[10px] uppercase tracking-wider text-foreground-tertiary">
          <PlugZap className="size-3" />
          {t("chat.emptyState.precheckTitle", "AI 引擎")}
        </p>
        {cliReadiness.loading ? (
          <span className="flex items-center gap-1.5 font-caption text-[11px] text-foreground-tertiary">
            <Loader2 className="size-3 animate-spin" />
            {t("chat.emptyState.precheckLoading", "正在检查引擎…")}
          </span>
        ) : (
          <div className="flex flex-col gap-1">
            {cliReadiness.installed.map((p) => (
              <span
                key={p.id}
                className="flex items-center gap-1.5 font-caption text-[11px] text-foreground-secondary"
              >
                <CircleCheck className="size-3.5 text-status-success" />
                {t("chat.emptyState.engineLoggedIn", {
                  name: p.display_name,
                  defaultValue: "{{name}} 已就绪",
                })}
              </span>
            ))}
            {cliReadiness.missing.map((p) => (
              <span
                key={p.id}
                className="flex items-center gap-1.5 font-caption text-[11px] text-foreground-tertiary"
              >
                <CircleX className="size-3.5 text-foreground-tertiary" />
                {t("chat.emptyState.engineMissing", {
                  name: p.display_name,
                  defaultValue: "{{name}} 未安装",
                })}
              </span>
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
