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
import { roleDisplayName } from "@/lib/agent";

export type TaskStatus = "pending" | "spawning" | "ready";

export interface TaskActivity {
  id: string;
  startedAt: number;
  /**
   * Epoch ms when the task FLIPPED to `ready`. Distinct from `startedAt` (the
   * triggering user-message time): a spawn that takes 30s to go green still
   * gets the full auto-dismiss window from this moment, not a window already
   * burned down by the spawn latency. Unset until the task reaches `ready`.
   */
  readyAt?: number;
  status: TaskStatus;
  /** 触发本次 task 的用户消息片段（"帮我加个登录页…"），最多 32 字符。 */
  trigger?: string;
  /** spawn 出来的 agent role 列表 (e.g. ["architect", "frontend"]) */
  spawnedRoles: string[];
  /** spawn 出来的 agent_id 列表(与 spawnedRoles 同步 append)——阶段条按
   *  这些 id 查 agent_stage 事件。 */
  spawnedAgentIds: string[];
}

/** 冷启动阶段(stage 事件名 → 步骤序号)。未知/未来 stage 归并到最后。 */
const STAGE_ORDER = ["shim_ready", "mcp_ready", "bootstrap_injected"] as const;
const STAGE_I18N_KEYS = [
  "task.stage.spawn",
  "task.stage.cli",
  "task.stage.tools",
  "task.stage.bootstrap",
] as const;

function stageIndex(stage: string | undefined): number {
  if (!stage) return 0;
  const i = STAGE_ORDER.indexOf(stage as (typeof STAGE_ORDER)[number]);
  // bootstrap_injected → 步骤 3(等首个响应);未知 stage 视为至少到 1。
  return i < 0 ? 1 : i + 1;
}

interface Props {
  tasks: TaskActivity[];
  onDismiss: (id: string) => void;
  /** agent_stage 事件的最新值(useWorkspaceShellData.agentStageById)。 */
  stageById: Record<string, { stage: string; at: number }>;
}

const STATUS_LABEL: Record<TaskStatus, { icon: LucideIcon; key: string }> = {
  pending: { icon: Loader2, key: "task.pending" },
  spawning: { icon: Loader2, key: "task.spawning" },
  ready: { icon: Sparkles, key: "task.ready" },
};

export function TaskActivity({ tasks, onDismiss, stageById }: Props) {
  if (tasks.length === 0) return null;
  return (
    <div className="flex shrink-0 flex-col gap-1.5 border-t border-border-subtle bg-surface-secondary/40 px-4 py-2">
      {tasks.map((task) => (
        <TaskRow key={task.id} task={task} onDismiss={onDismiss} stageById={stageById} />
      ))}
    </div>
  );
}

function TaskRow({
  task,
  onDismiss,
  stageById,
}: {
  task: TaskActivity;
  onDismiss: (id: string) => void;
  stageById: Props["stageById"];
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
  // 冷启动阶段条:取所有新 agent 里「最慢」的阶段(任何一只落后整体就
  // 算落后),让 30s 的冷启动变成可读的进度而不是死等。legacy 任务
  // (无 agentIds)退化为第一步进行中。
  const coldest =
    task.spawnedAgentIds.length === 0
      ? 0
      : task.spawnedAgentIds.reduce(
          (min, id) => Math.min(min, stageIndex(stageById[id]?.stage)),
          Number.POSITIVE_INFINITY,
        );
  const safeColdest = Number.isFinite(coldest) ? coldest : 0;
  // Display layer localizes the role labels — spawnedRoles holds the raw,
  // orchestrator-minted slugs (e.g. "Code Reviewer") which Chat.tsx matches
  // against a.role, so we MUST NOT mutate the stored values. roleDisplayName
  // normalizes + aliases them to the localized name (代码审查员) for display
  // only; without it a zh UI leaks raw English role names here.
  const label = t(key, {
    count: task.spawnedRoles.length,
    roles: task.spawnedRoles.map(roleDisplayName).join(" · "),
  });

  return (
    <div
      className={cn(
        "flex flex-col gap-1.5 rounded-lg border px-3 py-1.5 text-[12px]",
        task.status === "ready"
          ? "border-state-success/30 bg-status-success-soft"
          : "border-accent-primary/30 bg-accent-primary-soft",
      )}
    >
      <div className="flex items-center gap-2.5">
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
      {(task.status === "spawning" || task.status === "ready") && (
        <ol className="flex items-center gap-1 pl-6 pr-1" aria-label={t("task.stage.aria")}>
          {STAGE_I18N_KEYS.map((k, i) => {
            // ready 的卡:四个阶段全部按完成渲染 —— 快机器上 spawn→ready 只要
            // 一两秒,只放 spawning 态展示等于永远看不到;完成态链路同样回答
            // 「刚才那 30 秒它到底在干嘛」。
            const done = task.status === "ready" ? true : i < safeColdest;
            const current = task.status === "ready" ? false : i === safeColdest;
            return (
              <li key={k} className="flex min-w-0 items-center gap-1">
                {i > 0 && <span className={cn("mx-0.5 h-px w-3", done || current ? "bg-accent-primary/60" : "bg-border-subtle")} />}
                <span
                  className={cn(
                    "size-1.5 shrink-0 rounded-full",
                    done && "bg-accent-primary",
                    current && "animate-pulse bg-accent-primary-deep",
                    !done && !current && "bg-border-strong",
                  )}
                />
                <span
                  className={cn(
                    "truncate font-caption text-[10px]",
                    done && "text-foreground-tertiary line-through",
                    current && "font-medium text-accent-primary-deep",
                    !done && !current && "text-foreground-tertiary",
                  )}
                >
                  {t(k)}
                </span>
              </li>
            );
          })}
        </ol>
      )}
    </div>
  );
}
