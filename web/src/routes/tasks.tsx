/**
 * Kanban board (`/tasks`).
 *
 * Upgrades the read-only ledger into a writable control plane: every worker is
 * a task, grouped into status columns. Status is normally DERIVED from
 * lifecycle (running / done / blocked / archived) so the board tracks ground
 * truth automatically; the operator can OVERRIDE from a card (block / done /
 * archive / reopen) and that persists server-side (POST /api/tasks/:id/status).
 *
 * Live-ish via a short poll (the existing /ws/swarm doesn't carry a task event
 * yet — a deliberate scope line; poll is cheap and correct). Mutations refetch
 * immediately so the operator sees their action land.
 */
import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { api } from "@/api/http";
import type { TaskRow } from "@/api/types";
import { cn } from "@/lib/cn";
import { relTime } from "@/lib/relTime";
import { toast } from "@/lib/toast";
import { useToolWorkspaces } from "@/lib/useToolWorkspaces";
import { WorkspacePicker } from "@/components/WorkspacePicker";
import {
  ConfirmActionDialog,
  type ConfirmActionState,
} from "@/components/ConfirmActionDialog";

/** Cards shown per column before a "show all" expander kicks in. */
const COLUMN_CAP = 12;

// Column order + the dot color per status. Derived statuses + operator-set ones
// share this map so a human "blocked" lands in the same column as a derived one.
const COLUMNS: { key: string; dot: string }[] = [
  { key: "todo", dot: "bg-state-idle" },
  { key: "running", dot: "bg-accent-primary" },
  { key: "blocked", dot: "bg-status-danger" },
  { key: "done", dot: "bg-status-success" },
  { key: "archived", dot: "bg-foreground-tertiary" },
];

export default function TasksRoute() {
  const { t } = useTranslation();
  const { workspaces, wsId, setWsId } = useToolWorkspaces();
  const [tasks, setTasks] = useState<TaskRow[] | null>(null);
  const [err, setErr] = useState(false);
  const [expanded, setExpanded] = useState<Record<string, boolean>>({});
  const [confirm, setConfirm] = useState<ConfirmActionState | null>(null);

  const load = useCallback(async () => {
    try {
      const res = await api.listTasks(wsId || undefined);
      setTasks(res.tasks);
      setErr(false);
    } catch {
      setErr(true);
    }
  }, [wsId]);

  useEffect(() => {
    load();
    const id = window.setInterval(load, 4000);
    return () => window.clearInterval(id);
  }, [load]);

  const setStatus = useCallback(
    async (agentId: string, status: string | null) => {
      // optimistic: reflect immediately, then refetch for ground truth
      const snapshot = tasks; // P0-1: keep the pre-change list so we can revert
      setTasks((prev) =>
        prev
          ? prev.map((tk) =>
              tk.agent_id === agentId
                ? { ...tk, status: status ?? tk.status, overridden: status != null }
                : tk,
            )
          : prev,
      );
      try {
        await api.setTaskStatus(agentId, status);
        load();
      } catch (e) {
        // P0-1: a failed write must NOT masquerade as success. Roll the card
        // back to its real state and tell the user, instead of leaving it
        // optimistically "completed" while the backend never recorded it.
        setTasks(snapshot);
        toast.error(t("tasks.statusFailed", { defaultValue: "状态更新失败，请重试" }), {
          description: (e as Error)?.message,
        });
      }
    },
    [tasks, load, t],
  );

  const byCol = (key: string) => (tasks ?? []).filter((tk) => tk.status === key);

  const requestStatus = useCallback(
    (task: TaskRow, status: string | null) => {
      const role = task.role_label || task.role_slug || task.agent_id.slice(0, 8);
      const actionKey = status ?? "reopen";
      setConfirm({
        title: t(`tasks.confirm.${actionKey}.title`, { role }),
        description: t(`tasks.confirm.${actionKey}.desc`),
        confirmLabel: t(`tasks.confirm.${actionKey}.confirm`),
        variant: status === "archived" ? "destructive" : "default",
        onConfirm: () => setStatus(task.agent_id, status),
      });
    },
    [setStatus, t],
  );

  return (
    <div className="flex h-full flex-col overflow-hidden">
      <header className="flex items-center justify-between border-b border-border-subtle px-6 py-4">
        <div className="flex flex-col gap-0.5">
          <h1 className="font-display text-lg text-foreground-primary">{t("tasks.title")}</h1>
          <p className="font-caption text-xs text-foreground-tertiary">{t("tasks.subtitle")}</p>
        </div>
        <div className="flex items-center gap-3">
          {tasks && (
            <span className="font-mono text-xs text-foreground-tertiary">
              {t("tasks.count", { n: tasks.length })}
            </span>
          )}
          <WorkspacePicker workspaces={workspaces} value={wsId} onChange={setWsId} allowAll />
        </div>
      </header>

      {err && (
        <div className="px-6 py-3 font-caption text-sm text-status-danger">{t("tasks.loadError")}</div>
      )}

      {tasks && tasks.length === 0 && !err && (
        <div className="flex flex-1 items-center justify-center px-6 text-center font-caption text-sm text-foreground-tertiary">
          {t("tasks.empty")}
        </div>
      )}

      {tasks && tasks.length > 0 && (
        <div className="flex flex-1 gap-3 overflow-x-auto p-4">
          {COLUMNS.map((col) => {
            const items = byCol(col.key);
            const isExp = expanded[col.key];
            const shown = isExp ? items : items.slice(0, COLUMN_CAP);
            return (
              <div key={col.key} className="flex w-72 shrink-0 flex-col gap-2">
                <div className="flex items-center gap-2 px-1">
                  <span className={cn("size-2 rounded-full", col.dot)} />
                  <span className="font-caption text-xs font-medium text-foreground-secondary">
                    {t(`tasks.status.${col.key}`)}
                  </span>
                  <span className="font-mono text-[11px] text-foreground-tertiary">{items.length}</span>
                </div>
                <div className="flex flex-col gap-2">
                  {shown.map((tk) => (
                    <TaskCard key={tk.agent_id} task={tk} onSet={requestStatus} t={t} />
                  ))}
                  {items.length > COLUMN_CAP && (
                    <button
                      type="button"
                      onClick={() => setExpanded((e) => ({ ...e, [col.key]: !isExp }))}
                      className="rounded border border-border-subtle border-dashed px-2 py-1.5 font-caption text-[11px] text-foreground-tertiary transition-colors hover:bg-surface-tertiary hover:text-foreground-secondary"
                    >
                      {isExp ? t("tasks.collapse") : t("tasks.showAll", { n: items.length })}
                    </button>
                  )}
                </div>
              </div>
            );
          })}
        </div>
      )}
      <ConfirmActionDialog
        action={confirm}
        onOpenChange={(next) => {
          if (!next) setConfirm(null);
        }}
      />
    </div>
  );
}

function TaskCard({
  task,
  onSet,
  t,
}: {
  task: TaskRow;
  onSet: (task: TaskRow, status: string | null) => void;
  t: (k: string, o?: Record<string, unknown>) => string;
}) {
  return (
    <div className="flex flex-col gap-2 rounded-lg border border-border-subtle bg-surface-secondary p-3">
      <div className="flex items-center justify-between gap-2">
        <span className="flex min-w-0 items-center gap-1.5">
          {task.killed_at === null && (
            <span
              className="size-1.5 shrink-0 rounded-full bg-status-success"
              title={t("tasks.live")}
            />
          )}
          <span className="truncate font-medium text-sm text-foreground-primary" title={task.role_label}>
            {task.role_label || task.role_slug || t("tasks.untitled")}
          </span>
        </span>
        {task.overridden && (
          <span
            className="shrink-0 rounded bg-surface-tertiary px-1 py-0.5 font-caption text-[10px] text-foreground-tertiary"
            title={t("tasks.overriddenHint")}
          >
            {t("tasks.manual")}
          </span>
        )}
      </div>
      <div className="flex flex-wrap items-center gap-x-2 gap-y-0.5 font-mono text-[10px] text-foreground-tertiary">
        <span title={task.agent_id}>{task.agent_id.slice(0, 8)}</span>
        {task.handoff_signal && (
          <span className="truncate" title={task.handoff_signal}>
            → {task.handoff_signal.split("/").pop()}
          </span>
        )}
        <span className="ml-auto shrink-0" title={new Date(task.spawned_at).toLocaleString()}>
          {relTime(task.last_activity_at ?? task.spawned_at, t)}
        </span>
      </div>
      <div className="flex flex-wrap gap-1">
        {task.status !== "blocked" && (
          <CardBtn onClick={() => onSet(task, "blocked")}>{t("tasks.action.block")}</CardBtn>
        )}
        {task.status !== "done" && (
          <CardBtn onClick={() => onSet(task, "done")}>{t("tasks.action.done")}</CardBtn>
        )}
        {task.status !== "archived" && (
          <CardBtn onClick={() => onSet(task, "archived")}>{t("tasks.action.archive")}</CardBtn>
        )}
        {task.overridden && (
          <CardBtn onClick={() => onSet(task, null)}>{t("tasks.action.reopen")}</CardBtn>
        )}
      </div>
    </div>
  );
}

function CardBtn({ onClick, children }: { onClick: () => void; children: React.ReactNode }) {
  return (
    <button
      type="button"
      onClick={onClick}
      className="rounded border border-border-subtle px-1.5 py-0.5 font-caption text-[10px] text-foreground-secondary transition-colors hover:bg-surface-tertiary hover:text-foreground-primary"
    >
      {children}
    </button>
  );
}
