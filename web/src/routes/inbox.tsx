/**
 * Approval Inbox — Pencil frame NUCBp.
 *
 * UI-only shell with representative mock cards. The real wire requires
 * flockmux-shim to stop short of executing PreToolUse and instead push
 * an approval_pending event over /ws/swarm — today shim is launched
 * with --dangerously-skip-permissions (see memory bypass_approvals), so
 * the path doesn't exist server-side yet.
 *
 * Buttons are intentionally disabled with a tooltip explaining why.
 * When the backend lands, swap the MOCKS array for a real
 * api.listApprovals() + WS subscription, wire 批准/拒绝 to POST
 * /approvals/:id/{approve,reject}, drop the banner, enable the buttons.
 */

import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  AlertCircle,
  Check,
  ChevronDown,
  Eye,
  Inbox as InboxIcon,
  Info,
  ShieldCheck,
  X,
} from "lucide-react";
import { cn } from "@/lib/cn";

const ROLE_BG: Record<string, string> = {
  planner: "bg-agent-planner",
  backend: "bg-agent-backend",
  frontend: "bg-agent-frontend",
  architect: "bg-agent-architect",
  critic: "bg-agent-critic",
  test: "bg-agent-test",
};

type Severity = "danger" | "default";

interface MockApproval {
  id: string;
  agent: string;
  role: string;
  cli: string;
  tool: string;
  command: string;
  rationale: string;
  files: string;
  severity: Severity;
  at: string;
}

// Mock data stays English — once the real backend lands, rationale/at strings
// come from the server's payload, where localisation belongs.
const MOCKS: MockApproval[] = [
  {
    id: "ap-001",
    agent: "claude-2f8c9b1d",
    role: "backend",
    cli: "claude",
    tool: "git push origin main",
    command: "git push origin main",
    rationale:
      "PR branch with 38 commits. Touches apps/backend/api/routes.py +124 / alembic migration / requirements.txt +2 deps.",
    files: "git push origin main",
    severity: "danger",
    at: "just now (mock)",
  },
  {
    id: "ap-002",
    agent: "claude-7a4b32c1",
    role: "frontend",
    cli: "claude",
    tool: "write_blackboard",
    command: "swarm.write_blackboard release.yaml",
    rationale:
      "Writing release metadata to the shared blackboard; downstream deploy spell will read it.",
    files: "swarm.write_blackboard release.yaml [12 lines]",
    severity: "default",
    at: "1 min ago (mock)",
  },
  {
    id: "ap-003",
    agent: "claude-c19f0e62",
    role: "critic",
    cli: "claude",
    tool: "swarm_run_spell deploy",
    command: 'swarm.run_spell deploy --task="prod cutover"',
    rationale:
      "Spawned the deploy spell sub-flow, includes 2 agents: deployer + smoke-test.",
    files: 'swarm.run_spell deploy --task="prod cutover"',
    severity: "default",
    at: "3 min ago (mock)",
  },
];

type Filter = "all" | "high" | "default";

export default function InboxRoute() {
  const { t } = useTranslation();
  const [filter, setFilter] = useState<Filter>("all");
  const [dismissed, setDismissed] = useState<Set<string>>(new Set());

  const filtered = useMemo(() => {
    return MOCKS.filter((m) => {
      if (dismissed.has(m.id)) return false;
      if (filter === "high") return m.severity === "danger";
      if (filter === "default") return m.severity === "default";
      return true;
    });
  }, [filter, dismissed]);

  return (
    <div className="flex h-full flex-col bg-surface-primary">
      {/* Head */}
      <header className="flex h-14 shrink-0 items-center gap-3 border-b border-border-subtle bg-surface-elevated px-5">
        <span className="flex size-8 items-center justify-center rounded-md bg-accent-primary-soft">
          <InboxIcon className="size-4 text-accent-primary-deep" />
        </span>
        <div className="flex flex-col">
          <h1 className="font-heading text-sm font-semibold text-foreground-primary">
            {t("inbox.title")}
          </h1>
          <span className="font-caption text-[10px] text-foreground-tertiary">
            {t("inbox.subtitle")}
          </span>
        </div>
        <span className="flex-1" />
        <span className="rounded-full bg-accent-primary px-2 py-0.5 font-caption text-[10px] font-bold text-foreground-on-accent">
          {filtered.length}
        </span>
        <button
          className="flex size-8 items-center justify-center rounded-md bg-surface-tertiary text-foreground-secondary hover:bg-surface-secondary"
          title={t("nav.settings")}
        >
          <ShieldCheck className="size-4" />
        </button>
      </header>

      {/* Filter bar */}
      <div className="flex h-11 shrink-0 items-center gap-1.5 border-b border-border-subtle bg-surface-secondary px-5">
        {(
          [
            { id: "all", labelKey: "inbox.all" },
            { id: "high", labelKey: "inbox.high" },
            { id: "default", labelKey: "inbox.default" },
          ] as const
        ).map((f) => {
          const active = f.id === filter;
          return (
            <button
              key={f.id}
              onClick={() => setFilter(f.id)}
              className={cn(
                "rounded-full px-3 py-1 text-xs",
                active
                  ? "bg-accent-primary text-foreground-on-accent"
                  : "border border-border-subtle bg-surface-elevated text-foreground-secondary hover:bg-surface-tertiary",
              )}
            >
              {t(f.labelKey)}
            </button>
          );
        })}
        <span className="flex-1" />
        <button className="flex h-6 items-center gap-1 rounded border border-border-subtle bg-surface-elevated px-2 font-caption text-[11px] text-foreground-secondary">
          {t("inbox.rejectAll")}
          <ChevronDown className="size-3" />
        </button>
      </div>

      {/* WIP banner */}
      <div className="m-4 flex items-start gap-3 rounded-lg border border-status-warning/40 bg-status-warning-soft p-3">
        <AlertCircle className="mt-0.5 size-4 shrink-0 text-status-warning" />
        <div className="font-caption text-[11px] text-foreground-secondary">
          <p className="font-semibold text-foreground-primary">{t("inbox.wipTitle")}</p>
          <p>{t("inbox.wipBody")}</p>
        </div>
      </div>

      {/* List */}
      <div className="flex min-h-0 flex-1 flex-col gap-2.5 overflow-y-auto px-4 pb-4">
        {filtered.map((a) => (
          <article
            key={a.id}
            className={cn(
              "flex flex-col gap-2 rounded-lg border bg-surface-elevated p-3.5 shadow-sm",
              a.severity === "danger"
                ? "border-state-danger"
                : "border-border-subtle",
            )}
          >
            <div className="flex items-center gap-2">
              <span
                className={cn(
                  "flex size-6 items-center justify-center rounded-full font-bold text-[10px] text-foreground-on-accent",
                  ROLE_BG[a.role] ?? "bg-state-idle",
                )}
              >
                {a.role.slice(0, 1).toUpperCase()}
              </span>
              <span className="font-heading text-sm font-semibold text-foreground-primary">
                {a.role}
              </span>
              <span className="font-mono text-[11px] text-foreground-primary">
                {a.tool}
              </span>
              {a.severity === "danger" && (
                <span className="rounded-full bg-status-danger-soft px-2 py-0.5 font-caption text-[9px] text-state-danger">
                  {t("inbox.highRiskChip")}
                </span>
              )}
              <span className="ml-auto font-caption text-[10px] text-foreground-tertiary">
                {a.at}
              </span>
            </div>
            <p className="font-caption text-xs text-foreground-secondary">
              {a.rationale}
            </p>
            <pre className="overflow-x-auto rounded-md bg-surface-tertiary px-3 py-2 font-mono text-[11px] text-foreground-primary">
              {a.command}
            </pre>
            <div className="flex items-center gap-1.5">
              <span className="font-caption text-[10px] text-foreground-tertiary">
                {a.agent.slice(0, 14)}…
              </span>
              <span className="flex-1" />
              <button
                className="flex h-7 items-center gap-1 rounded-md border border-border-subtle bg-surface-elevated px-3 text-xs text-foreground-secondary hover:bg-surface-tertiary"
                title={t("inbox.viewTooltip")}
              >
                <Eye className="size-3" />
                {t("inbox.view")}
              </button>
              <button
                disabled
                title={t("inbox.disabledTooltip")}
                className="flex h-7 items-center gap-1 rounded-md border border-border-subtle bg-surface-elevated px-3 text-xs text-state-danger opacity-50"
                onClick={() =>
                  setDismissed((p) => new Set(p).add(a.id))
                }
              >
                <X className="size-3" />
                {t("inbox.reject")}
              </button>
              <button
                disabled
                title={t("inbox.disabledTooltip")}
                className="flex h-7 items-center gap-1 rounded-md bg-accent-primary px-3 text-xs font-bold text-foreground-on-accent opacity-50"
              >
                <Check className="size-3" />
                {t("inbox.approve")}
              </button>
            </div>
          </article>
        ))}
        {filtered.length === 0 && (
          <div className="flex flex-1 flex-col items-center justify-center gap-3 text-foreground-tertiary">
            <InboxIcon className="size-10 opacity-40" />
            <p className="font-caption text-sm">{t("inbox.empty")}</p>
          </div>
        )}
      </div>

      {/* Footer */}
      <footer className="flex shrink-0 items-center gap-3 border-t border-border-subtle bg-surface-elevated px-5 py-3">
        <Info className="size-3 text-foreground-tertiary" />
        <span className="font-caption text-[11px] text-foreground-secondary">
          {t("inbox.footerHint")}
        </span>
        <span className="flex-1" />
        <span className="rounded bg-surface-tertiary px-2 py-1 font-caption text-[10px] text-foreground-tertiary">
          {t("inbox.autoRefresh")}
        </span>
      </footer>
    </div>
  );
}
