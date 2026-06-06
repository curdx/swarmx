/**
 * ModelPicker — choose THIS direction's model AND reasoning/thinking effort.
 * A small pill in the chat top bar, à la ChatGPT's model dropdown.
 *
 * Two orthogonal knobs:
 *  - model: abstract tier (opus|sonnet|haiku) resolved per-CLI by the global 模型
 *    settings, or null = global default.
 *  - reasoning effort: abstract low|medium|high|max (both Claude Code and Codex
 *    converged on discrete effort levels in 2026), or null = the model's own
 *    default. Mapped to each CLI's concrete flag at spawn.
 *
 * Changing either persists on the direction and (via the parent) restarts the
 * live orchestrator so it takes effect immediately. The body sent is always the
 * complete desired state, so the parent merges the unchanged knob.
 */
import { useTranslation } from "react-i18next";
import { Check, ChevronDown, Cpu, Gauge, Loader2 } from "lucide-react";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import { cn } from "@/lib/cn";

const TIERS = ["opus", "sonnet", "haiku"] as const;
const EFFORTS = ["low", "medium", "high", "max"] as const;

function tierLabel(tier: string): string {
  if ((TIERS as readonly string[]).includes(tier)) {
    return tier.charAt(0).toUpperCase() + tier.slice(1);
  }
  return tier; // a concrete model id typed in settings
}

export function ModelPicker({
  tier,
  reasoning,
  onSet,
  busy = false,
}: {
  tier: string | null;
  reasoning: string | null;
  /** Set one knob; the parent keeps the other and sends the full state. */
  onSet: (cfg: { tier?: string | null; reasoning?: string | null }) => void;
  busy?: boolean;
}) {
  const { t } = useTranslation();
  const curTier = tier && tier.trim() ? tier.trim() : null;
  const curEffort = reasoning && reasoning.trim() ? reasoning.trim() : null;
  const modelLabel = curTier ? tierLabel(curTier) : t("model.default");
  // Pill shows "Sonnet · 高" when an effort is set, else just the model.
  const pillLabel = curEffort
    ? `${modelLabel} · ${t(`model.effort.${curEffort}`)}`
    : modelLabel;

  return (
    <Popover>
      <PopoverTrigger asChild>
        <button
          type="button"
          disabled={busy}
          title={t("model.tooltip")}
          aria-label={t("model.label")}
          className="flex h-7 items-center gap-1.5 rounded-md px-2 text-xs text-foreground-secondary transition-colors hover:bg-surface-tertiary disabled:opacity-60"
        >
          {busy ? (
            <Loader2 className="size-3.5 animate-spin text-foreground-tertiary" />
          ) : (
            <Cpu className="size-3.5 text-foreground-tertiary" />
          )}
          <span className="font-medium">{pillLabel}</span>
          <ChevronDown className="size-3 text-foreground-tertiary" />
        </button>
      </PopoverTrigger>
      <PopoverContent align="start" sideOffset={6} className="w-56 p-1">
        <Section icon={<Cpu className="size-3" />} title={t("model.heading")} />
        <MenuItem
          label={t("model.default")}
          hint={t("model.defaultHint")}
          active={curTier === null}
          onClick={() => onSet({ tier: null })}
        />
        {TIERS.map((tr) => (
          <MenuItem
            key={tr}
            label={tierLabel(tr)}
            hint={t(`model.hint.${tr}`)}
            active={curTier === tr}
            onClick={() => onSet({ tier: tr })}
          />
        ))}

        <div className="my-1 h-px bg-border-subtle" />
        <Section
          icon={<Gauge className="size-3" />}
          title={t("model.effortHeading")}
        />
        <MenuItem
          label={t("model.default")}
          hint={t("model.effortDefaultHint")}
          active={curEffort === null}
          onClick={() => onSet({ reasoning: null })}
        />
        {EFFORTS.map((ef) => (
          <MenuItem
            key={ef}
            label={t(`model.effort.${ef}`)}
            hint={t(`model.effortHint.${ef}`)}
            active={curEffort === ef}
            onClick={() => onSet({ reasoning: ef })}
          />
        ))}
      </PopoverContent>
    </Popover>
  );
}

function Section({ icon, title }: { icon: React.ReactNode; title: string }) {
  return (
    <p className="flex items-center gap-1.5 px-2 py-1 font-caption text-[10px] uppercase tracking-wider text-foreground-tertiary">
      {icon}
      {title}
    </p>
  );
}

function MenuItem({
  label,
  hint,
  active,
  onClick,
}: {
  label: string;
  hint?: string;
  active: boolean;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={cn(
        "flex w-full items-start gap-2 rounded-md px-2 py-1.5 text-left text-xs transition-colors hover:bg-surface-tertiary",
        active && "bg-surface-tertiary",
      )}
    >
      <Check
        className={cn(
          "mt-0.5 size-3.5 shrink-0",
          active ? "text-accent-primary" : "text-transparent",
        )}
      />
      <span className="min-w-0">
        <span className="block font-medium text-foreground-primary">{label}</span>
        {hint && (
          <span className="block font-caption text-[10px] text-foreground-tertiary">
            {hint}
          </span>
        )}
      </span>
    </button>
  );
}
