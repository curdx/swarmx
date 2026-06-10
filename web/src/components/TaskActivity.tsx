/**
 * TaskActivity — chat 底部浮一行 "AI 正在干活" inline 状态卡片。
 *
 * 解决业界 2026 共识里说的 "doomscrolling gap" —— 用户发完消息后 5-30
 * 秒，planner LLM 在思考、spell 在 spawn agent，整个 UI 黑盒，用户
 * OS："我发了消息然后..."。这一秒钟决定用户是 "wow 它真的自己干了"
 * 还是 "咦它死了"。
 *
 * 业界标杆 (Claude Code Agent Teams / Vibe Kanban / GitHub Agent HQ) 都
 * 用 pending → in-progress → blocked → completed 4 态 inline card 解决。
 * 我们这一版是 MVP，先做 pending → spawning → ready 3 态，blocked 留
 * 给 P3-2。
 */

import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Loader2, Sparkles, X, type LucideIcon } from "lucide-react";
import { cn } from "@/lib/cn";

export type TaskStatus = "pending" | "spawning" | "ready";

export interface TaskActivity {
  id: string;
  startedAt: number;
  status: TaskStatus;
  /** 触发本次 task 的用户消息片段（"帮我加个登录页…"），最多 32 字符。 */
  trigger?: string;
  /** spawn 出来的 agent role 列表 (e.g. ["architect", "frontend"]) */
  spawnedRoles: string[];
}

interface Props {
  tasks: TaskActivity[];
  onDismiss: (id: string) => void;
}

const STATUS_LABEL: Record<TaskStatus, { icon: LucideIcon; key: string }> = {
  pending: { icon: Loader2, key: "task.pending" },
  spawning: { icon: Loader2, key: "task.spawning" },
  ready: { icon: Sparkles, key: "task.ready" },
};

export function TaskActivity({ tasks, onDismiss }: Props) {
  if (tasks.length === 0) return null;
  return (
    <div className="flex shrink-0 flex-col gap-1.5 border-t border-border-subtle bg-surface-secondary/40 px-4 py-2">
      {tasks.map((task) => (
        <TaskRow key={task.id} task={task} onDismiss={onDismiss} />
      ))}
    </div>
  );
}

function TaskRow({
  task,
  onDismiss,
}: {
  task: TaskActivity;
  onDismiss: (id: string) => void;
}) {
  const { t } = useTranslation();
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    if (task.status === "ready") return; // ready 之后不需要 tick
    const id = window.setInterval(() => setNow(Date.now()), 500);
    return () => window.clearInterval(id);
  }, [task.status]);

  const elapsedSec = Math.floor((now - task.startedAt) / 1000);
  const { icon: Icon, key } = STATUS_LABEL[task.status];
  const label = t(key, {
    count: task.spawnedRoles.length,
    roles: task.spawnedRoles.join(" · "),
  });

  return (
    <div
      className={cn(
        "flex items-center gap-2.5 rounded-lg border px-3 py-1.5 text-[12px]",
        task.status === "ready"
          ? "border-state-success/30 bg-status-success-soft"
          : "border-accent-primary/30 bg-accent-primary-soft",
      )}
    >
      <Icon
        className={cn(
          "size-3.5 shrink-0",
          task.status === "ready"
            ? "text-state-success"
            : "animate-spin text-accent-primary-deep",
        )}
      />
      <span className="min-w-0 flex-1 truncate font-body text-foreground-primary">
        {label}
      </span>
      {task.trigger && (
        <span
          className="hidden max-w-[180px] truncate font-caption text-[10px] text-foreground-tertiary md:inline"
          title={task.trigger}
        >
          {t("task.triggered", { msg: task.trigger })}
        </span>
      )}
      <span className="shrink-0 font-mono text-[10px] text-foreground-tertiary">
        {elapsedSec}s
      </span>
      <button
        type="button"
        onClick={() => onDismiss(task.id)}
        className="flex size-8 shrink-0 items-center justify-center rounded text-foreground-tertiary hover:bg-surface-tertiary hover:text-foreground-primary"
        title={t("task.dismiss")}
      >
        <X className="size-3" />
      </button>
    </div>
  );
}
