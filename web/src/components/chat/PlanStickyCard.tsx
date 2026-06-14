/**
 * PlanStickyCard — the captain's plan as a glanceable checklist, pinned above
 * the conversation (P2). Driven by the STRUCTURED `plan.json` the orchestrator
 * writes (parsed by lib/parsePlan), so the ✓/◐/○ state is accurate — not
 * guessed from free-text prose. Rendered only when a real plan exists; on a
 * malformed / absent plan the parser returns null and nothing shows.
 */
import { useTranslation } from "react-i18next";
import { Circle, CircleCheck, CircleDot, Pin, TriangleAlert } from "lucide-react";
import i18n from "@/i18n";
import { cn } from "@/lib/cn";
import { roleColorClass as roleColor } from "@/lib/agent";
import type { ParsedPlan, PlanStatus } from "@/lib/parsePlan";

function ownerLabel(
  owner: string | undefined,
): { name: string; isCaptain: boolean } | null {
  if (!owner) return null;
  const o = owner.trim().toLowerCase();
  if (["self", "orchestrator", "captain", "me", "队长"].includes(o)) {
    return {
      name: i18n.t("chat.role.captain", { defaultValue: "队长" }),
      isCaptain: true,
    };
  }
  return { name: owner, isCaptain: false };
}

function StatusGlyph({ status }: { status: PlanStatus }) {
  if (status === "done") {
    return <CircleCheck className="size-4 shrink-0 text-status-success" aria-label="done" />;
  }
  if (status === "doing") {
    return <CircleDot className="size-4 shrink-0 text-accent-primary" aria-label="in progress" />;
  }
  if (status === "blocked") {
    return <TriangleAlert className="size-4 shrink-0 text-state-warning" aria-label="blocked" />;
  }
  return <Circle className="size-4 shrink-0 text-foreground-tertiary" aria-label="todo" />;
}

export function PlanStickyCard({ plan }: { plan: ParsedPlan }) {
  const { t } = useTranslation();
  const total = plan.steps.length;
  const done = plan.steps.filter((s) => s.status === "done").length;
  const allDone = total > 0 && done === total;

  // Once every step is done the full checklist is pure clutter — a scrollable
  // all-✓ list squatting on up to 34vh the user can no longer act on. Collapse
  // to a single confirmation line: frees the space, keeps honest closure, and
  // (unlike hiding outright) doesn't read as "my plan vanished". A new plan from
  // the captain flips a step back to todo/doing and the full list returns.
  if (allDone) {
    return (
      <div className="shrink-0 border-b border-border-subtle bg-surface-primary px-3 py-2">
        <div className="mx-auto flex w-full max-w-[1040px] items-center gap-2 rounded-lg border border-status-success/25 bg-status-success-soft/40 px-3 py-1.5">
          <CircleCheck className="size-4 shrink-0 text-status-success" aria-hidden />
          <span className="font-heading text-xs font-medium text-foreground-secondary">
            {t("chat.plan.allDone", {
              total,
              defaultValue: "计划完成 · 全部 {{total}} 步已交付",
            })}
          </span>
        </div>
      </div>
    );
  }

  return (
    <div className="shrink-0 border-b border-border-subtle bg-surface-primary px-3 py-2">
      <div className="mx-auto w-full max-w-[1040px] rounded-lg border border-accent-primary/25 bg-accent-primary-soft/50 px-3 py-2">
        <div className="mb-1.5 flex items-center gap-2">
          <Pin className="size-3.5 shrink-0 text-accent-primary" aria-hidden />
          <span className="font-heading text-xs font-medium text-accent-primary-deep">
            {t("chat.plan.title", {
              done,
              total,
              defaultValue: "计划 · 队长维护 · {{done}}/{{total}}",
            })}
          </span>
        </div>
        <ul className="flex max-h-[34vh] flex-col gap-1 overflow-y-auto">
          {plan.steps.map((s, i) => {
            const owner = ownerLabel(s.owner);
            return (
              <li
                key={s.seq ?? i}
                className="flex items-center gap-2 text-[13px] leading-snug"
              >
                <StatusGlyph status={s.status} />
                <span
                  className={cn(
                    "min-w-0 flex-1 truncate",
                    s.status === "done"
                      ? "text-foreground-tertiary line-through"
                      : "text-foreground-primary",
                  )}
                  title={s.task}
                >
                  {s.task}
                </span>
                {owner && (
                  <span
                    className={cn(
                      "inline-flex shrink-0 items-center gap-1 rounded-full px-1.5 py-0.5 font-caption text-[10px]",
                      owner.isCaptain
                        ? "bg-accent-primary-soft text-accent-primary"
                        : "bg-surface-secondary text-foreground-secondary",
                    )}
                  >
                    {!owner.isCaptain && (
                      <span className={cn("size-1.5 rounded-full", roleColor(owner.name))} />
                    )}
                    {owner.name}
                  </span>
                )}
              </li>
            );
          })}
        </ul>
      </div>
    </div>
  );
}
