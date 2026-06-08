/**
 * Goal Mode (`/goals`).
 *
 * Workspace-level objectives with acceptance criteria, status, and optional
 * token budget. This is the durable layer above the task board: tasks show
 * what workers are doing now; goals show what the whole run is supposed to
 * achieve and whether it is still active, blocked, or complete.
 */
import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  Archive,
  CheckCircle2,
  CirclePause,
  Flag,
  Loader2,
  MessageSquarePlus,
  OctagonAlert,
  Plus,
  RefreshCw,
} from "lucide-react";
import { api } from "@/api/http";
import type { GoalEvidenceRecord, GoalRecord, GoalStatus, ThreadInfo } from "@/api/types";
import { WorkspacePicker } from "@/components/WorkspacePicker";
import { cn } from "@/lib/cn";
import { relTime } from "@/lib/relTime";
import { useToolWorkspaces } from "@/lib/useToolWorkspaces";

const STATUSES: Array<{ key: GoalStatus; icon: typeof Flag; tone: string }> = [
  { key: "active", icon: Flag, tone: "text-accent-primary" },
  { key: "paused", icon: CirclePause, tone: "text-foreground-tertiary" },
  { key: "blocked", icon: OctagonAlert, tone: "text-status-danger" },
  { key: "complete", icon: CheckCircle2, tone: "text-status-success" },
  { key: "archived", icon: Archive, tone: "text-foreground-tertiary" },
];
const MAIN_THREAD_VALUE = "__main__";

function parseCriteria(input: string): string[] {
  return input
    .split("\n")
    .map((s) => s.replace(/^[-*]\s+/, "").trim())
    .filter(Boolean);
}

function formatBudget(n: number | null): string {
  if (!n) return "—";
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(2)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`;
  return String(n);
}

export default function GoalsRoute() {
  const { t } = useTranslation();
  const { workspaces, wsId, setWsId, ready } = useToolWorkspaces();
  const [threadId, setThreadId] = useState<string>("");
  const [threads, setThreads] = useState<ThreadInfo[]>([]);
  const [goals, setGoals] = useState<GoalRecord[]>([]);
  const [objective, setObjective] = useState("");
  const [criteria, setCriteria] = useState("");
  const [budget, setBudget] = useState("");
  const [loading, setLoading] = useState(true);
  const [creating, setCreating] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  const activeWorkspace = useMemo(
    () => workspaces.find((w) => w.id === wsId) ?? null,
    [workspaces, wsId],
  );

  useEffect(() => {
    setThreadId("");
    setThreads(activeWorkspace?.threads ?? []);
  }, [activeWorkspace]);

  const load = useCallback(async () => {
    if (!wsId) {
      setGoals([]);
      setLoading(false);
      return;
    }
    setLoading(true);
    try {
      const threadFilter =
        threadId === MAIN_THREAD_VALUE ? null : threadId || undefined;
      const res = await api.listGoals(wsId, threadFilter);
      setGoals(res.goals);
      setErr(null);
    } catch (e) {
      setErr((e as Error).message);
    } finally {
      setLoading(false);
    }
  }, [threadId, wsId]);

  useEffect(() => {
    if (!ready) return;
    load();
  }, [load, ready]);

  const create = async () => {
    if (!wsId || !objective.trim()) return;
    setCreating(true);
    setErr(null);
    try {
      const rawBudget = budget.trim();
      await api.createGoal({
        workspace_id: wsId,
        thread_id: threadId && threadId !== MAIN_THREAD_VALUE ? threadId : null,
        objective: objective.trim(),
        success_criteria: parseCriteria(criteria),
        budget_tokens: rawBudget ? Number(rawBudget) : null,
      });
      setObjective("");
      setCriteria("");
      setBudget("");
      await load();
    } catch (e) {
      setErr((e as Error).message);
    } finally {
      setCreating(false);
    }
  };

  const setStatus = async (goal: GoalRecord, status: GoalStatus) => {
    setGoals((prev) =>
      prev.map((g) =>
        g.id === goal.id
          ? {
              ...g,
              status,
              updated_at: Date.now(),
              completed_at: status === "complete" || status === "archived" ? Date.now() : null,
            }
          : g,
      ),
    );
    try {
      await api.updateGoalStatus(goal.id, status);
    } finally {
      load();
    }
  };

  return (
    <div className="h-full overflow-y-auto">
      <div className="mx-auto flex max-w-5xl flex-col gap-5 px-6 py-6">
        <header className="flex items-start justify-between gap-3">
          <div className="flex flex-col gap-1">
            <h1 className="font-display text-lg text-foreground-primary">{t("goals.title")}</h1>
            <p className="font-caption text-xs text-foreground-tertiary">{t("goals.subtitle")}</p>
          </div>
          <div className="flex shrink-0 items-center gap-2">
            <button
              type="button"
              onClick={load}
              title={t("common.refresh")}
              aria-label={t("common.refresh")}
              className="rounded border border-border-subtle p-1 text-foreground-tertiary hover:bg-surface-tertiary hover:text-foreground-secondary"
            >
              <RefreshCw className="size-3.5" />
            </button>
            <WorkspacePicker workspaces={workspaces} value={wsId} onChange={setWsId} />
            <select
              value={threadId}
              onChange={(e) => setThreadId(e.target.value)}
              disabled={!wsId}
              className="rounded border border-border-subtle bg-surface-primary px-2 py-1 text-[13px] text-foreground-primary disabled:opacity-50"
              aria-label={t("goals.thread")}
            >
              <option value="">{t("goals.allDirections")}</option>
              <option value={MAIN_THREAD_VALUE}>{t("goals.mainDirection")}</option>
              {threads.map((th) => (
                <option key={th.id} value={th.id}>
                  {th.name || th.slug}
                </option>
              ))}
            </select>
          </div>
        </header>

        <section className="grid gap-4 lg:grid-cols-[360px_1fr]">
          <div className="flex flex-col gap-3 rounded-lg border border-border-subtle bg-surface-secondary p-4">
            <div className="flex items-center gap-2">
              <Plus className="size-4 text-accent-primary" />
              <h2 className="font-caption text-sm font-medium text-foreground-primary">
                {t("goals.newGoal")}
              </h2>
            </div>
            <label className="flex flex-col gap-1">
              <span className="font-caption text-[11px] text-foreground-tertiary">
                {t("goals.objective")}
              </span>
              <textarea
                value={objective}
                onChange={(e) => setObjective(e.target.value)}
                rows={3}
                className="resize-none rounded border border-border-subtle bg-surface-primary px-2 py-1.5 text-[13px] text-foreground-primary"
                placeholder={t("goals.objectivePlaceholder")}
              />
            </label>
            <label className="flex flex-col gap-1">
              <span className="font-caption text-[11px] text-foreground-tertiary">
                {t("goals.criteria")}
              </span>
              <textarea
                value={criteria}
                onChange={(e) => setCriteria(e.target.value)}
                rows={5}
                className="resize-none rounded border border-border-subtle bg-surface-primary px-2 py-1.5 text-[13px] text-foreground-primary"
                placeholder={t("goals.criteriaPlaceholder")}
              />
            </label>
            <label className="flex flex-col gap-1">
              <span className="font-caption text-[11px] text-foreground-tertiary">
                {t("goals.budget")}
              </span>
              <input
                value={budget}
                onChange={(e) => setBudget(e.target.value.replace(/[^\d]/g, ""))}
                inputMode="numeric"
                className="rounded border border-border-subtle bg-surface-primary px-2 py-1.5 font-mono text-[13px]"
                placeholder="200000"
              />
            </label>
            <button
              type="button"
              onClick={create}
              disabled={creating || !wsId || !objective.trim()}
              className="inline-flex items-center justify-center gap-1.5 rounded-md bg-accent-primary px-3 py-1.5 text-[13px] text-foreground-on-accent disabled:opacity-50"
            >
              {creating && <Loader2 className="size-3.5 animate-spin" />}
              {creating ? t("goals.creating") : t("goals.create")}
            </button>
          </div>

          <div className="flex min-h-[360px] flex-col gap-3">
            {err && (
              <div className="rounded-lg border border-border-subtle bg-surface-secondary px-4 py-3 font-caption text-sm text-status-danger">
                {err}
              </div>
            )}
            {loading ? (
              <div className="flex items-center gap-2 rounded-lg border border-border-subtle bg-surface-secondary px-4 py-6 font-caption text-xs text-foreground-tertiary">
                <Loader2 className="size-3.5 animate-spin" /> {t("common.loading")}
              </div>
            ) : goals.length === 0 ? (
              <div className="flex flex-1 items-center justify-center rounded-lg border border-border-subtle bg-surface-secondary px-4 py-10 text-center font-caption text-sm text-foreground-tertiary">
                {t("goals.empty")}
              </div>
            ) : (
              goals.map((goal) => (
                <GoalCard
                  key={goal.id}
                  goal={goal}
                  threads={threads}
                  onStatus={setStatus}
                  onEvidenceAdded={load}
                  t={t}
                />
              ))
            )}
          </div>
        </section>
      </div>
    </div>
  );
}

function GoalCard({
  goal,
  threads,
  onStatus,
  onEvidenceAdded,
  t,
}: {
  goal: GoalRecord;
  threads: ThreadInfo[];
  onStatus: (goal: GoalRecord, status: GoalStatus) => void;
  onEvidenceAdded: () => void;
  t: (k: string, o?: Record<string, unknown>) => string;
}) {
  const [evidence, setEvidence] = useState<GoalEvidenceRecord[]>([]);
  const [evidenceOpen, setEvidenceOpen] = useState(false);
  const [evidenceSummary, setEvidenceSummary] = useState("");
  const [evidenceKind, setEvidenceKind] = useState("note");
  const [evidenceLoading, setEvidenceLoading] = useState(false);
  const [evidenceErr, setEvidenceErr] = useState<string | null>(null);
  const metaThread =
    goal.thread_id == null
      ? t("goals.mainDirection")
      : threads.find((th) => th.id === goal.thread_id)?.name ||
        threads.find((th) => th.id === goal.thread_id)?.slug ||
        goal.thread_id.slice(0, 8);
  const current = STATUSES.find((s) => s.key === goal.status) ?? STATUSES[0];
  const CurrentIcon = current.icon;
  const loadEvidence = useCallback(async () => {
    if (!evidenceOpen) return;
    setEvidenceLoading(true);
    try {
      const res = await api.listGoalEvidence(goal.id, 20);
      setEvidence(res.evidence);
      setEvidenceErr(null);
    } catch (e) {
      setEvidenceErr((e as Error).message);
    } finally {
      setEvidenceLoading(false);
    }
  }, [evidenceOpen, goal.id]);

  useEffect(() => {
    loadEvidence();
  }, [loadEvidence]);

  const addEvidence = async () => {
    if (!evidenceSummary.trim()) return;
    setEvidenceLoading(true);
    try {
      await api.addGoalEvidence(goal.id, {
        kind: evidenceKind.trim() || "note",
        summary: evidenceSummary.trim(),
      });
      setEvidenceSummary("");
      setEvidenceOpen(true);
      await loadEvidence();
      onEvidenceAdded();
    } catch (e) {
      setEvidenceErr((e as Error).message);
    } finally {
      setEvidenceLoading(false);
    }
  };

  return (
    <article className="rounded-lg border border-border-subtle bg-surface-secondary p-4">
      <div className="flex items-start justify-between gap-3">
        <div className="flex min-w-0 flex-col gap-1">
          <div className="flex items-center gap-2">
            <CurrentIcon className={cn("size-4 shrink-0", current.tone)} />
            <h2 className="text-sm font-medium leading-snug text-foreground-primary">
              {goal.objective}
            </h2>
          </div>
          <div className="flex flex-wrap gap-x-3 gap-y-1 font-caption text-[11px] text-foreground-tertiary">
            <span>{metaThread}</span>
            <span>{t("goals.updated", { time: relTime(goal.updated_at, t) })}</span>
            <span>{t("goals.budgetShort", { budget: formatBudget(goal.budget_tokens) })}</span>
          </div>
        </div>
        <span className="shrink-0 rounded bg-surface-tertiary px-2 py-0.5 font-caption text-[11px] text-foreground-secondary">
          {t(`goals.status.${goal.status}`)}
        </span>
      </div>

      {goal.success_criteria.length > 0 && (
        <ul className="mt-3 flex flex-col gap-1">
          {goal.success_criteria.map((c) => (
            <li key={c} className="flex gap-2 text-[12px] text-foreground-secondary">
              <span className="mt-1 size-1.5 shrink-0 rounded-full bg-accent-primary" />
              <span>{c}</span>
            </li>
          ))}
        </ul>
      )}

      <div className="mt-3 flex flex-wrap gap-1.5">
        {STATUSES.map(({ key, icon: Icon }) => (
          <button
            key={key}
            type="button"
            disabled={goal.status === key}
            onClick={() => onStatus(goal, key)}
            className={cn(
              "inline-flex items-center gap-1 rounded border px-2 py-1 font-caption text-[11px] transition-colors",
              goal.status === key
                ? "border-accent-primary bg-accent-primary-soft text-accent-primary"
                : "border-border-subtle text-foreground-secondary hover:bg-surface-tertiary hover:text-foreground-primary",
            )}
          >
            <Icon className="size-3" />
            {t(`goals.status.${key}`)}
          </button>
        ))}
        <button
          type="button"
          onClick={() => setEvidenceOpen((v) => !v)}
          className="inline-flex items-center gap-1 rounded border border-border-subtle px-2 py-1 font-caption text-[11px] text-foreground-secondary hover:bg-surface-tertiary hover:text-foreground-primary"
        >
          <MessageSquarePlus className="size-3" />
          {t("goals.evidence")}
        </button>
      </div>

      {evidenceOpen && (
        <div className="mt-3 rounded-md border border-border-subtle bg-surface-primary p-3">
          <div className="grid gap-2 sm:grid-cols-[120px_1fr_auto]">
            <input
              value={evidenceKind}
              onChange={(e) => setEvidenceKind(e.target.value)}
              className="rounded border border-border-subtle bg-surface-secondary px-2 py-1 font-mono text-[12px]"
              aria-label={t("goals.evidenceKind")}
            />
            <input
              value={evidenceSummary}
              onChange={(e) => setEvidenceSummary(e.target.value)}
              className="rounded border border-border-subtle bg-surface-secondary px-2 py-1 text-[12px]"
              placeholder={t("goals.evidencePlaceholder")}
              aria-label={t("goals.evidenceSummary")}
            />
            <button
              type="button"
              disabled={evidenceLoading || !evidenceSummary.trim()}
              onClick={addEvidence}
              className="inline-flex items-center justify-center gap-1 rounded bg-accent-primary px-2 py-1 font-caption text-[11px] text-foreground-on-accent disabled:opacity-50"
            >
              {evidenceLoading && <Loader2 className="size-3 animate-spin" />}
              {t("goals.addEvidence")}
            </button>
          </div>
          {evidenceErr && (
            <p className="mt-2 font-caption text-[11px] text-status-danger">{evidenceErr}</p>
          )}
          <div className="mt-3 flex flex-col gap-2">
            {evidenceLoading && evidence.length === 0 ? (
              <p className="font-caption text-[11px] text-foreground-tertiary">
                {t("common.loading")}
              </p>
            ) : evidence.length === 0 ? (
              <p className="font-caption text-[11px] text-foreground-tertiary">
                {t("goals.noEvidence")}
              </p>
            ) : (
              evidence.map((ev) => (
                <div key={ev.id} className="flex gap-2 text-[12px] text-foreground-secondary">
                  <span className="shrink-0 rounded bg-surface-tertiary px-1.5 py-0.5 font-mono text-[10px] text-foreground-tertiary">
                    {ev.kind}
                  </span>
                  <span className="min-w-0 flex-1">{ev.summary}</span>
                  <span className="shrink-0 font-caption text-[10px] text-foreground-tertiary">
                    {relTime(ev.created_at, t)}
                  </span>
                </div>
              ))
            )}
          </div>
        </div>
      )}
    </article>
  );
}
