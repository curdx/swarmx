/**
 * Cron page (`/cron`).
 *
 * Schedule a prompt to be delivered to a workspace's orchestrator on a 5-field
 * cron. Times are interpreted in a fixed offset (the browser's local offset for
 * new jobs; the job's own stored offset when editing), which the server uses to
 * evaluate the expression — the UI never shows raw UTC. A live cronstrue
 * description + server-computed next-run preview catch a typo before save; each
 * row shows when it next fires, can be edited in place, and "Run now" fires
 * immediately via /api/cron/:id/run.
 */
import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { Play, Trash2, Loader2, Check, X, Pencil } from "lucide-react";
import { api } from "@/api/http";
import type { CronJob, Workspace } from "@/api/types";
import { cn } from "@/lib/cn";
import { toast } from "@/lib/toast";
import {
  describeCron,
  fmtDate,
  fmtTime,
  localOffsetMinutes,
  relativeFromNow,
  tzOffsetLabel,
  wallClock,
} from "@/lib/cron";

const PRESETS: { key: string; expr: string }[] = [
  { key: "hourly", expr: "0 * * * *" },
  { key: "daily9", expr: "0 9 * * *" },
  { key: "weekdays9", expr: "0 9 * * 1-5" },
  { key: "monday9", expr: "0 9 * * 1" },
  { key: "monthly1", expr: "0 9 1 * *" },
];

type FormValues = {
  workspace_id: string;
  name: string;
  cron_expr: string;
  prompt: string;
  tz_offset_minutes: number;
};

type TFn = ReturnType<typeof useTranslation>["t"];

/** "今天 09:00 · 约 3 小时后" — a UTC instant rendered in a fixed offset. */
function formatRun(utcMs: number, offsetMin: number, now: number, t: TFn, lang: string): string {
  const w = wallClock(utcMs, offsetMin, now);
  const day =
    w.dayDiff === 0
      ? t("cron.today")
      : w.dayDiff === 1
        ? t("cron.tomorrow")
        : w.dayDiff === -1
          ? t("cron.yesterday")
          : fmtDate(w);
  return `${day} ${fmtTime(w)} · ${relativeFromNow(utcMs, now, lang)}`;
}

/**
 * Shared create/edit form. `tzOffset` is the offset the expression is authored
 * in — the browser's local offset for a new job, the job's own offset when
 * editing (so an edit never silently shifts the schedule's timezone).
 */
function CronJobForm({
  workspaces,
  tzOffset,
  lang,
  initial,
  submitLabel,
  submittingLabel,
  onSubmit,
  onDone,
  onCancel,
  resetAfterSubmit,
}: {
  workspaces: Workspace[];
  tzOffset: number;
  lang: string;
  initial: { wsId: string; name: string; expr: string; prompt: string };
  submitLabel: string;
  submittingLabel: string;
  onSubmit: (v: FormValues) => Promise<{ ok: boolean; error?: string }>;
  onDone: (describe: string | null) => void;
  onCancel?: () => void;
  resetAfterSubmit?: boolean;
}) {
  const { t } = useTranslation();
  const tzLabel = tzOffsetLabel(tzOffset);
  const [wsId, setWsId] = useState(initial.wsId);
  const [name, setName] = useState(initial.name);
  const [expr, setExpr] = useState(initial.expr);
  const [prompt, setPrompt] = useState(initial.prompt);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const [exprPreview, setExprPreview] = useState<{ valid: boolean; next_run: number | null } | null>(null);
  const describe = useMemo(() => describeCron(expr, lang), [expr, lang]);

  // Fill an empty workspace once the list arrives (create mode mounts before
  // workspaces load). Never overrides a value already set (edit mode).
  useEffect(() => {
    if (!wsId && workspaces.length) setWsId(workspaces[0].id);
  }, [workspaces, wsId]);

  useEffect(() => {
    const e = expr.trim();
    if (!e) {
      setExprPreview(null);
      return;
    }
    // Guard against out-of-order responses: a slow earlier request must not
    // overwrite a newer one. clearTimeout covers the not-yet-fired case; the
    // flag covers a request already in flight when the input changes.
    let cancelled = false;
    const id = setTimeout(() => {
      api
        .cronPreview(e, tzOffset)
        .then((r) => {
          if (!cancelled) setExprPreview(r);
        })
        .catch(() => {
          if (!cancelled) setExprPreview(null);
        });
    }, 300);
    return () => {
      cancelled = true;
      clearTimeout(id);
    };
  }, [expr, tzOffset]);

  const now = Date.now();
  const canSubmit = !busy && !!wsId && !!expr.trim() && !!prompt.trim() && exprPreview?.valid !== false;

  const submit = async () => {
    setBusy(true);
    setErr(null);
    try {
      const res = await onSubmit({
        workspace_id: wsId,
        name,
        cron_expr: expr,
        prompt,
        tz_offset_minutes: tzOffset,
      });
      if (!res.ok) {
        setErr(res.error ?? "failed");
      } else {
        if (resetAfterSubmit) {
          setName("");
          setPrompt("");
        }
        onDone(describe);
      }
    } catch (e) {
      setErr((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="flex flex-col gap-2">
      <div className="grid grid-cols-1 gap-2 sm:grid-cols-2">
        <label className="flex flex-col gap-1">
          <span className="font-caption text-[11px] text-foreground-tertiary">{t("cron.workspace")}</span>
          <select
            name="cron-workspace"
            value={wsId}
            onChange={(e) => setWsId(e.target.value)}
            className="min-h-8 rounded border border-border-subtle bg-surface-primary px-2 py-1 text-[13px]"
          >
            {workspaces.length === 0 && <option value="">—</option>}
            {/* an edited job may point at a deleted workspace not in the list */}
            {wsId && !workspaces.some((w) => w.id === wsId) && <option value={wsId}>{wsId.slice(0, 8)}</option>}
            {workspaces.map((w) => (
              <option key={w.id} value={w.id}>
                {w.name}
              </option>
            ))}
          </select>
        </label>
        <label className="flex flex-col gap-1">
          <span className="font-caption text-[11px] text-foreground-tertiary">
            {t("cron.expr")} · {tzLabel}
          </span>
          <input
            name="cron-expression"
            value={expr}
            onChange={(e) => setExpr(e.target.value)}
            placeholder="0 9 * * 1-5"
            spellCheck={false}
            aria-invalid={exprPreview?.valid === false}
            className={cn(
              "min-h-8 rounded border bg-surface-primary px-2 py-1 font-mono text-[13px]",
              exprPreview && !exprPreview.valid ? "border-status-danger" : "border-border-subtle",
            )}
          />
        </label>
      </div>

      {/* preset chips */}
      <div className="flex flex-wrap items-center gap-1.5">
        <span className="font-caption text-[11px] text-foreground-tertiary">{t("cron.presetsLabel")}</span>
        {PRESETS.map((p) => (
          <button
            key={p.key}
            type="button"
            onClick={() => setExpr(p.expr)}
            aria-pressed={expr.trim() === p.expr}
            className={cn(
              "rounded-full border px-2 py-0.5 font-caption text-[11px] transition-colors",
              expr.trim() === p.expr
                ? "border-accent-primary bg-accent-primary/10 text-accent-primary"
                : "border-border-subtle text-foreground-secondary hover:bg-surface-tertiary",
            )}
          >
            {t(`cron.preset.${p.key}`)}
          </button>
        ))}
      </div>

      {/* human-readable description + validity / next-run */}
      <div className="min-h-[16px] font-caption text-[11px]">
        {exprPreview?.valid === false ? (
          <span className="text-status-danger">{t("cron.invalidExpr")}</span>
        ) : describe ? (
          <span className="text-foreground-secondary">
            {describe}
            {exprPreview?.next_run
              ? ` · ${t("cron.nextLabel")} ${formatRun(exprPreview.next_run, tzOffset, now, t, lang)}`
              : exprPreview && exprPreview.next_run === null && exprPreview.valid
                ? ` · ${t("cron.noUpcoming")}`
                : ""}
          </span>
        ) : (
          <span className="text-foreground-tertiary">{t("cron.exprHint")}</span>
        )}
      </div>

      <label className="flex flex-col gap-1">
        <span className="font-caption text-[11px] text-foreground-tertiary">{t("cron.name")}</span>
        <input
          name="cron-name"
          value={name}
          onChange={(e) => setName(e.target.value)}
          placeholder={t("cron.namePlaceholder")}
          className="min-h-8 rounded border border-border-subtle bg-surface-primary px-2 py-1 text-[13px]"
        />
      </label>
      <label className="flex flex-col gap-1">
        <span className="font-caption text-[11px] text-foreground-tertiary">{t("cron.prompt")}</span>
        <textarea
          name="cron-prompt"
          value={prompt}
          onChange={(e) => setPrompt(e.target.value)}
          rows={2}
          placeholder={t("cron.promptPlaceholder")}
          className="resize-none rounded border border-border-subtle bg-surface-primary px-2 py-1 text-[13px]"
        />
      </label>

      {err && <div className="font-caption text-[11px] text-status-danger">{err}</div>}

      <div className="flex justify-end gap-2">
        {onCancel && (
          <button
            type="button"
            onClick={onCancel}
            className="min-h-8 rounded-md border border-border-subtle px-3 py-1.5 text-[13px] text-foreground-secondary hover:bg-surface-tertiary"
          >
            {t("cron.cancel")}
          </button>
        )}
        <button
          type="button"
          disabled={!canSubmit}
          onClick={submit}
          className="min-h-8 rounded-md bg-accent-primary px-3 py-1.5 text-[13px] text-foreground-on-accent disabled:opacity-50"
        >
          {busy ? submittingLabel : submitLabel}
        </button>
      </div>
    </div>
  );
}

export default function CronRoute() {
  const { t, i18n } = useTranslation();
  const lang = i18n.language ?? "zh";
  const tzOffset = useMemo(() => localOffsetMinutes(), []);
  const tzLabel = tzOffsetLabel(tzOffset);

  const [jobs, setJobs] = useState<CronJob[]>([]);
  const [workspaces, setWorkspaces] = useState<Workspace[]>([]);
  const [err, setErr] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  const [notice, setNotice] = useState<string | null>(null);
  const [runResult, setRunResult] = useState<{ id: string; ok: boolean; msg: string } | null>(null);
  const [runningId, setRunningId] = useState<string | null>(null);
  const [confirmDel, setConfirmDel] = useState<string | null>(null);
  const [editingId, setEditingId] = useState<string | null>(null);

  useEffect(() => {
    if (!notice) return;
    const id = setTimeout(() => setNotice(null), 4000);
    return () => clearTimeout(id);
  }, [notice]);
  useEffect(() => {
    if (!runResult) return;
    const id = setTimeout(() => setRunResult(null), 4000);
    return () => clearTimeout(id);
  }, [runResult]);
  useEffect(() => {
    if (!confirmDel) return;
    const id = setTimeout(() => setConfirmDel(null), 3500);
    return () => clearTimeout(id);
  }, [confirmDel]);

  const load = useCallback(async () => {
    try {
      const [c, ws] = await Promise.all([api.listCron(), api.listWorkspaces().catch(() => [])]);
      setJobs(c.jobs);
      setWorkspaces(ws);
      setErr(null);
    } catch (e) {
      setErr((e as Error).message);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  const toggle = async (j: CronJob) => {
    // Optimistic: flip immediately, reconcile on the response.
    const snap = jobs; // P1-21: keep the pre-toggle list so we can revert
    setJobs((prev) => prev.map((x) => (x.id === j.id ? { ...x, enabled: !x.enabled } : x)));
    try {
      await api.toggleCron(j.id, !j.enabled);
      load();
    } catch (e) {
      // A failed toggle must NOT leave the switch optimistically flipped while the
      // backend never recorded it. Roll the switch back and surface the error.
      setJobs(snap);
      toast.error(t("cron.toggleFailed", { defaultValue: "切换启用状态失败，请重试" }), {
        description: (e as Error)?.message,
      });
    }
  };

  const runNow = async (j: CronJob) => {
    setRunningId(j.id);
    setRunResult(null);
    try {
      const r = await api.runCron(j.id);
      if (!r.ok && r.skipped) setRunResult({ id: j.id, ok: false, msg: r.skipped });
      else setRunResult({ id: j.id, ok: true, msg: t("cron.ranOk") });
    } catch (e) {
      setRunResult({ id: j.id, ok: false, msg: (e as Error).message });
    } finally {
      setRunningId(null);
      load();
    }
  };

  const remove = async (id: string) => {
    setConfirmDel(null);
    const snap = jobs; // P1-22: keep the pre-delete list so we can revert
    setJobs((prev) => prev.filter((x) => x.id !== id)); // optimistic
    try {
      await api.deleteCron(id);
      load();
    } catch (e) {
      // A failed delete must NOT make the row silently vanish as if it succeeded.
      // Pull it back (snapshot first, then reload for ground truth) and tell the user.
      setJobs(snap);
      load();
      toast.error(t("cron.deleteFailed", { defaultValue: "删除任务失败，请重试" }), {
        description: (e as Error)?.message,
      });
    }
  };

  const wsName = (id: string) => workspaces.find((w) => w.id === id)?.name ?? id.slice(0, 8);
  const now = Date.now();

  return (
    <div className="h-full overflow-y-auto">
      <div className="mx-auto flex max-w-3xl flex-col gap-6 px-6 py-6">
        <header className="flex flex-col gap-1">
          <h1 className="font-display text-lg text-foreground-primary">{t("cron.title")}</h1>
          <p className="font-caption text-xs text-foreground-tertiary">
            {t("cron.subtitle")} · {t("cron.localTimeNote", { tz: tzLabel })}
          </p>
        </header>

        {/* create form */}
        <section className="rounded-lg border border-border-subtle bg-surface-secondary p-4">
          <CronJobForm
            workspaces={workspaces}
            tzOffset={tzOffset}
            lang={lang}
            initial={{ wsId: "", name: "", expr: "0 9 * * *", prompt: "" }}
            submitLabel={t("cron.create")}
            submittingLabel={t("cron.creating")}
            resetAfterSubmit
            onSubmit={(v) => api.createCron(v)}
            onDone={(describe) => {
              setNotice(describe ? t("cron.createdOk", { desc: describe }) : t("cron.createdPlain"));
              load();
            }}
          />
        </section>

        {err && <div className="font-caption text-sm text-status-danger">{err}</div>}
        {notice && <div className="font-caption text-sm text-status-success">{notice}</div>}

        {/* job list */}
        {loading ? (
          <div className="flex items-center gap-2 font-caption text-xs text-foreground-tertiary">
            <Loader2 className="size-3.5 animate-spin" /> {t("common.loading")}
          </div>
        ) : jobs.length === 0 ? (
          <div className="rounded-lg border border-border-subtle bg-surface-secondary px-4 py-6 text-center font-caption text-sm text-foreground-tertiary">
            {t("cron.empty")}
          </div>
        ) : (
          <>
            <div className="font-caption text-[11px] text-foreground-tertiary">{t("cron.count", { n: jobs.length })}</div>
            <ul className="flex flex-col gap-2">
              {jobs.map((j) => {
                if (editingId === j.id) {
                  return (
                    <li
                      key={j.id}
                      className="rounded-lg border border-accent-primary/40 bg-surface-secondary px-3 py-3"
                    >
                      <CronJobForm
                        workspaces={workspaces}
                        // Edit in the job's OWN offset so changing name/expr/prompt
                        // never silently shifts the schedule's timezone.
                        tzOffset={j.tz_offset_minutes}
                        lang={lang}
                        initial={{ wsId: j.workspace_id, name: j.name, expr: j.cron_expr, prompt: j.prompt }}
                        submitLabel={t("cron.save")}
                        submittingLabel={t("cron.saving")}
                        onCancel={() => setEditingId(null)}
                        onSubmit={(v) => api.updateCron(j.id, v)}
                        onDone={() => {
                          setEditingId(null);
                          setNotice(t("cron.savedOk"));
                          load();
                        }}
                      />
                    </li>
                  );
                }
                const desc = describeCron(j.cron_expr, lang);
                const orphaned = workspaces.length > 0 && !workspaces.some((w) => w.id === j.workspace_id);
                return (
                  <li
                    key={j.id}
                    className="flex flex-col gap-1.5 rounded-lg border border-border-subtle bg-surface-secondary px-3 py-2"
                  >
                    <div className="flex items-center gap-3">
                      <button
                        type="button"
                        role="switch"
                        aria-checked={j.enabled}
                        aria-label={j.enabled ? t("cron.disable") : t("cron.enable")}
                        onClick={() => toggle(j)}
                        title={j.enabled ? t("cron.disable") : t("cron.enable")}
                        className={cn(
                          "size-8 shrink-0 rounded-full border border-transparent p-0 hover:border-border-subtle hover:bg-surface-tertiary after:mx-auto after:block after:size-2.5 after:rounded-full",
                          j.enabled ? "after:bg-status-success" : "after:bg-foreground-tertiary",
                        )}
                      />
                      <div className={cn("flex min-w-0 flex-1 flex-col", !j.enabled && "opacity-55")}>
                        <span className="flex items-center gap-1.5 truncate text-[13px] text-foreground-primary">
                          <span className="truncate">{j.name}</span>
                          {!j.enabled && (
                            <span className="shrink-0 rounded bg-surface-tertiary px-1 py-px font-caption text-[10px] text-foreground-tertiary">
                              {t("cron.paused")}
                            </span>
                          )}
                          {orphaned && (
                            <span className="shrink-0 rounded bg-status-danger/10 px-1 py-px font-caption text-[10px] text-status-danger">
                              {t("cron.orphaned")}
                            </span>
                          )}
                        </span>
                        <span className="truncate font-caption text-[11px] text-foreground-tertiary" title={j.cron_expr}>
                          {desc ?? j.cron_expr} · {wsName(j.workspace_id)}
                          {/* Disclose the authoring zone when it differs from the
                              viewer's, so the global "your local zone" note can't
                              silently mislabel a job written in another offset
                              (incl. legacy UTC rows that predate this column). */}
                          {j.tz_offset_minutes !== tzOffset ? ` · ${tzOffsetLabel(j.tz_offset_minutes)}` : ""}
                        </span>
                        <span className="truncate font-caption text-[11px] text-foreground-tertiary">
                          {j.enabled && j.next_run
                            ? `${t("cron.nextLabel")} ${formatRun(j.next_run, j.tz_offset_minutes, now, t, lang)}`
                            : j.enabled
                              ? t("cron.noUpcoming")
                              : ""}
                          {j.last_run_at
                            ? `${j.enabled && j.next_run ? " · " : ""}${t("cron.lastRun")} ${relativeFromNow(j.last_run_at, now, lang)}`
                            : ""}
                        </span>
                      </div>
                      <button
                        type="button"
                        onClick={() => {
                          setConfirmDel(null);
                          setEditingId(j.id);
                        }}
                        aria-label={t("cron.edit")}
                        title={t("cron.edit")}
                        className="flex size-8 shrink-0 items-center justify-center rounded text-foreground-tertiary hover:bg-surface-tertiary hover:text-foreground-primary"
                      >
                        <Pencil className="size-3.5" />
                      </button>
                      <button
                        type="button"
                        disabled={runningId === j.id}
                        onClick={() => runNow(j)}
                        aria-label={runningId === j.id ? t("cron.running") : t("cron.runNow")}
                        title={runningId === j.id ? t("cron.running") : t("cron.runNow")}
                        className="flex size-8 shrink-0 items-center justify-center rounded text-foreground-tertiary hover:bg-surface-tertiary hover:text-foreground-primary disabled:opacity-60"
                      >
                        {runningId === j.id ? <Loader2 className="size-3.5 animate-spin" /> : <Play className="size-3.5" />}
                      </button>
                      {confirmDel === j.id ? (
                        <div className="flex shrink-0 items-center gap-1">
                          <button
                            type="button"
                            onClick={() => remove(j.id)}
                            aria-label={t("cron.confirmDelete")}
                            title={t("cron.confirmDelete")}
                            className="flex items-center gap-1 rounded bg-status-danger/10 px-2 py-1 font-caption text-[11px] text-status-danger hover:bg-status-danger/20"
                          >
                            <Check className="size-3" />
                            {t("cron.confirmDelete")}
                          </button>
                          <button
                            type="button"
                            onClick={() => setConfirmDel(null)}
                            aria-label={t("cron.cancel")}
                            title={t("cron.cancel")}
                            className="flex size-7 items-center justify-center rounded text-foreground-tertiary hover:bg-surface-tertiary"
                          >
                            <X className="size-3.5" />
                          </button>
                        </div>
                      ) : (
                        <button
                          type="button"
                          onClick={() => setConfirmDel(j.id)}
                          aria-label={t("cron.delete")}
                          title={t("cron.delete")}
                          className="flex size-8 shrink-0 items-center justify-center rounded text-foreground-tertiary hover:bg-surface-tertiary hover:text-status-danger"
                        >
                          <Trash2 className="size-3.5" />
                        </button>
                      )}
                    </div>
                    {runResult?.id === j.id && (
                      <div
                        className={cn(
                          "pl-11 font-caption text-[11px]",
                          runResult.ok ? "text-status-success" : "text-status-danger",
                        )}
                      >
                        {runResult.msg}
                      </div>
                    )}
                  </li>
                );
              })}
            </ul>
          </>
        )}
      </div>
    </div>
  );
}
