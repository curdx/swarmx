/**
 * ModelPicker — choose which model THIS direction's AI (the orchestrator, plus
 * workers that don't pin their own tier) runs at. A small pill in the chat top
 * bar, à la ChatGPT's model dropdown.
 *
 * The value is the direction's `model_tier`: an abstract tier (opus|sonnet|
 * haiku) resolved per-CLI by the global 模型 settings, or null = "use the global
 * default". Changing it persists on the thread and (via the parent) restarts the
 * live orchestrator so the switch takes effect immediately.
 */
import { useTranslation } from "react-i18next";
import { Check, ChevronDown, Cpu, Loader2 } from "lucide-react";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import { cn } from "@/lib/cn";

const TIERS = ["opus", "sonnet", "haiku"] as const;

/** Display label for a tier value (proper-case the known tiers; verbatim for a
 *  concrete model id). */
function tierLabel(tier: string): string {
  if ((TIERS as readonly string[]).includes(tier)) {
    return tier.charAt(0).toUpperCase() + tier.slice(1);
  }
  return tier; // a concrete model id typed in settings
}

export function ModelPicker({
  tier,
  onSet,
  busy = false,
}: {
  /** Current direction model_tier, or null = global default. */
  tier: string | null;
  /** Set the tier (null clears to global default). */
  onSet: (tier: string | null) => void;
  busy?: boolean;
}) {
  const { t } = useTranslation();
  const current = tier && tier.trim() ? tier.trim() : null;
  const label = current ? tierLabel(current) : t("model.default");

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
          <span className="font-medium">{label}</span>
          <ChevronDown className="size-3 text-foreground-tertiary" />
        </button>
      </PopoverTrigger>
      <PopoverContent align="start" sideOffset={6} className="w-52 p-1">
        <p className="px-2 py-1 font-caption text-[10px] uppercase tracking-wider text-foreground-tertiary">
          {t("model.heading")}
        </p>
        <MenuItem
          label={t("model.default")}
          hint={t("model.defaultHint")}
          active={current === null}
          onClick={() => onSet(null)}
        />
        {TIERS.map((tr) => (
          <MenuItem
            key={tr}
            label={tierLabel(tr)}
            hint={t(`model.hint.${tr}`)}
            active={current === tr}
            onClick={() => onSet(tr)}
          />
        ))}
      </PopoverContent>
    </Popover>
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
