/**
 * Cron page (`/cron`).
 *
 * Schedule a prompt to be delivered to a workspace's orchestrator on a 5-field
 * cron (UTC). Backed by /api/cron CRUD + a server-side scheduler; "Run now"
 * fires immediately via /api/cron/:id/run. Schedules are evaluated in UTC.
 */
import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Play, Trash2, Loader2 } from "lucide-react";
import { api } from "@/api/http";
import type { CronJob, Workspace } from "@/api/types";
import { cn } from "@/lib/cn";

export default function CronRoute() {
  const { t } = useTranslation();
  const [jobs, setJobs] = useState<CronJob[]>([]);
  const [workspaces, setWorkspaces] = useState<Workspace[]>([]);
  const [err, setErr] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  // create form
  const [wsId, setWsId] = useState("");
  const [name, setName] = useState("");
  const [expr, setExpr] = useState("0 9 * * *");
  const [prompt, setPrompt] = useState("");
  const [creating, setCreating] = useState(false);
  // Live validation + next-run preview (debounced) so a typo'd expr is caught
  // before create and the user sees when it will actually fire.
  const [exprPreview, setExprPreview] = useState<{ valid: boolean; next_run: number | null } | null>(null);

  useEffect(() => {
    const e = expr.trim();
    if (!e) {
      setExprPreview(null);
      return;
    }
    const id = setTimeout(() => {
      api.cronPreview(e).then(setExprPreview).catch(() => setExprPreview(null));
    }, 300);
    return () => clearTimeout(id);
  }, [expr]);

  // Run-now feedback: spinner on the firing row, a transient success notice
  // (previously only failures surfaced), and a two-step inline delete confirm.
  const [notice, setNotice] = useState<string | null>(null);
  const [runningId, setRunningId] = useState<string | null>(null);
  const [confirmDel, setConfirmDel] = useState<string | null>(null);

  useEffect(() => {
    if (!notice) return;
    const id = setTimeout(() => setNotice(null), 3500);
    return () => clearTimeout(id);
  }, [notice]);

  const load = useCallback(async () => {
    try {
      const [c, ws] = await Promise.all([api.listCron(), api.listWorkspaces().catch(() => [])]);
      setJobs(c.jobs);
      setWorkspaces(ws);
      if (!wsId && ws.length) setWsId(ws[0].id);
      setErr(null);
    } catch (e) {
      setErr((e as Error).message);
    } finally {
      setLoading(false);
    }
  }, [wsId]);

  useEffect(() => {
    load();
  }, [load]);

  const create = async () => {
    setCreating(true);
    setErr(null);
    try {
      const res = await api.createCron({ workspace_id: wsId, name, cron_expr: expr, prompt });
      if (!res.ok) {
        setErr(res.error ?? "create failed");
      } else {
        setName("");
        setPrompt("");
        await load();
      }
    } catch (e) {
      setErr((e as Error).message);
    } finally {
      setCreating(false);
    }
  };

  const wsName = (id: string) => workspaces.find((w) => w.id === id)?.name ?? id.slice(0, 8);

  return (
    <div className="h-full overflow-y-auto">
      <div className="mx-auto flex max-w-3xl flex-col gap-6 px-6 py-6">
        <header className="flex flex-col gap-1">
          <h1 className="font-display text-lg text-foreground-primary">{t("cron.title")}</h1>
          <p className="font-caption text-xs text-foreground-tertiary">{t("cron.subtitle")}</p>
        </header>

        {/* create form */}
        <section className="flex flex-col gap-2 rounded-lg border border-border-subtle bg-surface-secondary p-4">
          <div className="grid grid-cols-2 gap-2">
            <label className="flex flex-col gap-1">
              <span className="font-caption text-[11px] text-foreground-tertiary">{t("cron.workspace")}</span>
              <select
                name="cron-workspace"
                value={wsId}
                onChange={(e) => setWsId(e.target.value)}
                className="min-h-8 rounded border border-border-subtle bg-surface-primary px-2 py-1 text-[13px]"
              >
                {workspaces.length === 0 && <option value="">—</option>}
                {workspaces.map((w) => (
                  <option key={w.id} value={w.id}>
                    {w.name}
                  </option>
                ))}
              </select>
            </label>
            <label className="flex flex-col gap-1">
              <span className="font-caption text-[11px] text-foreground-tertiary">{t("cron.expr")} (UTC)</span>
              <input
                name="cron-expression"
                value={expr}
                onChange={(e) => setExpr(e.target.value)}
                placeholder="0 9 * * 1-5"
                className={cn(
                  "min-h-8 rounded border bg-surface-primary px-2 py-1 font-mono text-[13px]",
                  exprPreview && !exprPreview.valid ? "border-status-danger" : "border-border-subtle",
                )}
              />
              {exprPreview &&
                (!exprPreview.valid ? (
                  <span className="font-caption text-[11px] text-status-danger">{t("cron.invalidExpr")}</span>
                ) : exprPreview.next_run ? (
                  <span className="font-caption text-[11px] text-foreground-tertiary">
                    {t("cron.nextRun", { time: new Date(exprPreview.next_run).toLocaleString() })}
                  </span>
                ) : (
                  <span className="font-caption text-[11px] text-status-warning">{t("cron.noUpcoming")}</span>
                ))}
            </label>
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
          <div className="flex justify-end">
            <button
              type="button"
              disabled={
                creating || !wsId || !expr.trim() || !prompt.trim() || exprPreview?.valid === false
              }
              onClick={create}
              className="min-h-8 rounded-md bg-accent-primary px-3 py-1.5 text-[13px] text-foreground-on-accent disabled:opacity-50"
            >
              {creating ? t("cron.creating") : t("cron.create")}
            </button>
          </div>
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
          <ul className="flex flex-col gap-2">
            {jobs.map((j) => (
              <li
                key={j.id}
                className="flex items-center gap-3 rounded-lg border border-border-subtle bg-surface-secondary px-3 py-2"
              >
                <button
                  type="button"
                  onClick={async () => {
                    await api.toggleCron(j.id, !j.enabled);
                    load();
                  }}
                  title={j.enabled ? t("cron.disable") : t("cron.enable")}
                  className={cn(
                    "size-8 shrink-0 rounded-full border border-transparent p-0 hover:border-border-subtle hover:bg-surface-tertiary after:mx-auto after:block after:size-2.5 after:rounded-full",
                    j.enabled ? "after:bg-status-success" : "after:bg-foreground-tertiary",
                  )}
                />
                <div className="flex min-w-0 flex-1 flex-col">
                  <span className="truncate text-[13px] text-foreground-primary">{j.name}</span>
                  <span className="truncate font-mono text-[11px] text-foreground-tertiary">
                    {j.cron_expr} · {wsName(j.workspace_id)}
                    {j.last_run_at ? ` · ${t("cron.lastRun")} ${new Date(j.last_run_at).toLocaleString()}` : ""}
                  </span>
                </div>
                <button
                  type="button"
                  disabled={runningId === j.id}
                  onClick={async () => {
                    setRunningId(j.id);
                    setErr(null);
                    setNotice(null);
                    try {
                      const r = await api.runCron(j.id);
                      if (!r.ok && r.skipped) setErr(`${j.name}: ${r.skipped}`);
                      else setNotice(`${j.name} · ${t("cron.ranOk")}`);
                    } finally {
                      setRunningId(null);
                      load();
                    }
                  }}
                  title={runningId === j.id ? t("cron.running") : t("cron.runNow")}
                  className="flex size-8 shrink-0 items-center justify-center rounded text-foreground-tertiary hover:bg-surface-tertiary hover:text-foreground-primary disabled:opacity-60"
                >
                  {runningId === j.id ? (
                    <Loader2 className="size-3.5 animate-spin" />
                  ) : (
                    <Play className="size-3.5" />
                  )}
                </button>
                {confirmDel === j.id ? (
                  <button
                    type="button"
                    onClick={async () => {
                      setConfirmDel(null);
                      await api.deleteCron(j.id);
                      load();
                    }}
                    title={t("cron.confirmDelete")}
                    className="min-h-8 shrink-0 rounded bg-status-danger/10 px-2 py-1 font-caption text-[11px] text-status-danger hover:bg-status-danger/20"
                  >
                    {t("cron.confirmDelete")}
                  </button>
                ) : (
                  <button
                    type="button"
                    onClick={() => setConfirmDel(j.id)}
                    title={t("cron.delete")}
                    className="flex size-8 shrink-0 items-center justify-center rounded text-foreground-tertiary hover:bg-surface-tertiary hover:text-status-danger"
                  >
                    <Trash2 className="size-3.5" />
                  </button>
                )}
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}
