/**
 * Chat view — the messages tab inside WorkspaceShell.
 *
 * Reduced from the previous chat.tsx route to just two regions:
 *   - center: MessagesPanel (composer + bubbles)
 *   - right:  members sidebar
 *
 * Workspace state, sidebar, channel header, tab bar all live in the
 * parent Shell. We pull what we need (members, unread, live events,
 * composer override) via useWorkspaceContext().
 */

import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { api } from "../../../api/http";
import type { AgentInfo } from "../../../api/types";
import { MessagesPanel } from "../../../components/MessagesPanel";
import { OnboardingTour } from "../../../components/OnboardingTour";
import {
  TaskActivity,
  type TaskActivity as TaskActivityT,
} from "../../../components/TaskActivity";
import { Avatar, AvatarFallback } from "@/components/ui/avatar";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { Users, Zap } from "lucide-react";
import { cn } from "@/lib/cn";
import { roleColorClass as roleColor } from "@/lib/agent";
import { useWorkspaceContext } from "../Shell";

function statusDot(a: AgentInfo, t: (k: string) => string) {
  if (a.killed_at) return { className: "bg-state-idle", label: t("chat.exited") };
  if (a.shim_exit != null) return { className: "bg-state-idle", label: t("chat.shimExit") };
  if (!a.shim_ready) return { className: "bg-state-wake", label: t("chat.starting") };
  return { className: "bg-state-success", label: t("chat.online") };
}

// pending task 兜底超时 — 60s 内没看到任何 AI 反应就放弃 (网络/agent 死)。
// 普通场景：AI reply 几秒后就会 dismiss，触不到这个 timeout。
const TASK_PENDING_TIMEOUT_MS = 60_000;
// task 进入 ready 后展示这么久再自动消失（让用户看清楚已就绪）。
const TASK_READY_DISMISS_MS = 4_000;
// spawning 事件归属到最近 user 消息的窗口。超过这个时长的 spawn 视为
// 独立事件，不归到用户上一条消息。
const TASK_ATTACH_WINDOW_MS = 15_000;

export default function ChatView() {
  const { t } = useTranslation();
  const {
    workspace,
    activeMembers,
    allAliveAgents,
    workspaceAgentIds,
    liveMessage,
    liveRead,
    unreadByFrom: activeWorkspaceUnread,
    jumpUnreadTick,
    openAgent,
    composerOverride,
    // unreadByFrom in OutletContext is workspace-filtered; the right-side
    // members list wants raw agent-id → count so it can show the small
    // red badge per row. We re-derive it by indexing into the filtered
    // map (same keys, just used differently).
  } = useWorkspaceContext();

  // ── Task activity state machine ──────────────────────────────────
  // 解决业界说的 "doomscrolling gap" — 用户发完消息后 5-30 秒 UI 黑盒。
  // 简化版状态机:
  //   1. user 发消息 → push pending TaskActivity
  //   2. agent_state=spawning 且 8s 内有 pending task → 把 role 加进去，
  //      升级到 spawning 状态
  //   3. 所有 spawning agents 都 ready → 升 ready，4s 后自动 dismiss
  //   4. pending 超过 15s 没 spawning → expired (默认 dismiss)
  // user 发的是普通聊天消息 (没派活) 时 15s timeout 兜底，无副作用。
  const [tasks, setTasks] = useState<TaskActivityT[]>([]);
  const tasksRef = useRef<TaskActivityT[]>([]);
  useEffect(() => {
    tasksRef.current = tasks;
  }, [tasks]);

  // 已知 agent_id 集合 — 用来判定 spawning 事件是不是新 agent (老 agent
  // 重启也会发 spawning 但不算新派活)。
  const knownAgentIdsRef = useRef<Set<string>>(new Set());
  useEffect(() => {
    for (const a of allAliveAgents) knownAgentIdsRef.current.add(a.agent_id);
  }, [allAliveAgents]);

  const dismissTask = useCallback((id: string) => {
    setTasks((prev) => prev.filter((tsk) => tsk.id !== id));
  }, []);

  // pending task 兜底过期 + ready task 自动消失
  useEffect(() => {
    if (tasks.length === 0) return;
    const now = Date.now();
    const timers: number[] = [];
    for (const task of tasks) {
      if (task.status === "ready") {
        const remaining = TASK_READY_DISMISS_MS - (now - task.startedAt);
        timers.push(
          window.setTimeout(() => dismissTask(task.id), Math.max(0, remaining)),
        );
      } else if (task.status === "pending") {
        const remaining = TASK_PENDING_TIMEOUT_MS - (now - task.startedAt);
        timers.push(
          window.setTimeout(() => dismissTask(task.id), Math.max(0, remaining)),
        );
      }
    }
    return () => {
      for (const id of timers) window.clearTimeout(id);
    };
  }, [tasks, dismissTask]);

  // 状态机重新设计 (基于用户反馈 — chat typing indicator 已经表达"AI 正
  // 在响应"，task card 不要重复，只在 chat 不能表达的事才出现：
  //
  //   ✗ 普通 single-agent reply → 不显示 task card (chat typing 够了)
  //   ✓ 派活拉起了新 agent → 显示 "拉起 X 个 agent · roles..."
  //     这是 chat 看不到的信息：成员栏多了几个人但没人特别提示
  //   ✓ 所有新 agent ready → 升 ready (✓)，4s 后消失
  //   ✓ 兜底 60s 没 ready 就 expired (网络/agent 死)
  //
  // 触发：记录最近 user msg 时间，新 agent 出现且在 user msg 后 15s 内
  // 才认为是"派活触发的"，否则视为 agent 自发 spawn (比如别人启动的) 不
  // 显示。
  const lastUserMsgRef = useRef<{ at: number; body: string; id: number } | null>(null);
  useEffect(() => {
    if (!liveMessage) return;
    if (liveMessage.from_agent === "user") {
      lastUserMsgRef.current = {
        at: liveMessage.sent_at,
        body: liveMessage.body.slice(0, 32),
        id: liveMessage.id,
      };
    }
  }, [liveMessage]);

  // allAliveAgents 出现新 agent → 如果最近 15s 内有 user msg → 创建/更新
  // spawning task；新 agent 进入 ready 时升 task 状态
  useEffect(() => {
    const newAgents: AgentInfo[] = [];
    for (const a of allAliveAgents) {
      if (!knownAgentIdsRef.current.has(a.agent_id)) {
        newAgents.push(a);
        knownAgentIdsRef.current.add(a.agent_id);
      }
    }
    if (newAgents.length === 0) {
      // 没新 agent — 检查现有 spawning task 内的 agent 是否都 ready，是
      // 就升级 task。
      setTasks((prev) =>
        prev.map((tsk) => {
          if (tsk.status !== "spawning") return tsk;
          const matching = allAliveAgents.filter((a) =>
            tsk.spawnedRoles.includes(a.role),
          );
          if (matching.length === 0) return tsk;
          const allReady = matching.every((a) => a.shim_ready);
          return allReady ? { ...tsk, status: "ready" as const } : tsk;
        }),
      );
      return;
    }
    const lastUser = lastUserMsgRef.current;
    const now = Date.now();
    if (!lastUser || now - lastUser.at > TASK_ATTACH_WINDOW_MS) {
      // 新 agent 不是用户消息触发的 (>15s 前 / 没 user msg) — 不显示 task
      // card，agent 加入会通过成员栏体现。
      return;
    }
    setTasks((prev) => {
      const taskId = `task-${lastUser.id}`;
      const existing = prev.find((t) => t.id === taskId);
      const allReady = newAgents.every((a) => a.shim_ready);
      if (existing) {
        // 已经为这条 user msg 创建过 task — append roles
        return prev.map((tsk) =>
          tsk.id === taskId
            ? {
                ...tsk,
                status: (allReady ? "ready" : "spawning") as TaskActivityT["status"],
                spawnedRoles: [...tsk.spawnedRoles, ...newAgents.map((a) => a.role)],
              }
            : tsk,
        );
      }
      return [
        ...prev,
        {
          id: taskId,
          startedAt: lastUser.at,
          status: (allReady ? "ready" : "spawning") as TaskActivityT["status"],
          trigger: lastUser.body,
          spawnedRoles: newAgents.map((a) => a.role),
        },
      ];
    });
  }, [allAliveAgents]);

  return (
    <div className="flex min-h-0 flex-1">
      {/* 4 步 onboarding tour — 第一次进 chat 时弹，跳过/走完 mark seen
       *  存 localStorage 之后不再弹。装在 ChatView 而不是 Shell，是因为
       *  Shell 在没 workspace 时也 render (Welcome 屏)，那里 tour 没意
       *  义；只有真的进了某个 workspace 的 chat 才相关。 */}
      <OnboardingTour />
      <section className="flex min-w-0 flex-1 flex-col">
        <MessagesPanel
          liveMessage={liveMessage}
          liveRead={liveRead}
          unreadByFrom={activeWorkspaceUnread}
          activeMembers={activeMembers}
          allAliveAgents={allAliveAgents}
          workspaceAgentIds={workspaceAgentIds}
          workspaceLabel={workspace.name}
          composerOverride={composerOverride}
          jumpUnreadTick={jumpUnreadTick}
          onOpenAgent={openAgent}
          taskActivityBelow={
            <TaskActivity tasks={tasks} onDismiss={dismissTask} />
          }
        />
      </section>

      <aside className="flex w-[340px] shrink-0 flex-col border-l border-border-subtle bg-surface-elevated">
        <div className="flex h-12 shrink-0 items-center gap-2 border-b border-border-subtle px-4">
          <Users className="size-4 text-foreground-tertiary" />
          <h2 className="font-heading text-xs font-semibold uppercase tracking-wider text-foreground-tertiary">
            {t("chat.members")}
          </h2>
          <span className="ml-auto font-caption text-xs text-foreground-tertiary">
            {activeMembers.length}
          </span>
        </div>
        <div className="flex-1 overflow-y-auto px-2 py-2">
          {activeMembers.length === 0 && (
            <p className="px-3 py-2 font-caption text-xs text-foreground-tertiary">
              {t("chat.selectWorkspace")}
            </p>
          )}
          {activeMembers.map((a) => {
            const dot = statusDot(a, t);
            const unread = activeWorkspaceUnread[a.agent_id] ?? 0;
            return (
              <div
                key={a.agent_id}
                onClick={() => openAgent(a.agent_id)}
                className={cn(
                  "flex cursor-pointer items-center gap-3 rounded-md px-3 py-2 hover:bg-surface-tertiary",
                )}
              >
                <Avatar className="size-8 shrink-0" title={a.role}>
                  <AvatarFallback
                    className={cn(
                      "text-xs font-medium text-foreground-on-accent",
                      roleColor(a.role),
                    )}
                  >
                    {a.role.slice(0, 1).toUpperCase()}
                  </AvatarFallback>
                </Avatar>
                <div className="min-w-0 flex-1">
                  <div className="flex items-center gap-2">
                    <span className="truncate font-heading text-sm text-foreground-primary">
                      {a.role}
                    </span>
                    <span
                      className={cn("size-1.5 rounded-full", dot.className)}
                      title={dot.label}
                    />
                  </div>
                  <div className="truncate font-mono text-[10px] text-foreground-tertiary">
                    {a.cli} · {a.agent_id.slice(-8)}
                  </div>
                </div>
                {unread > 0 && (
                  <Badge
                    variant="destructive"
                    className="rounded-full px-1.5 py-0.5 text-[10px]"
                  >
                    {unread}
                  </Badge>
                )}
                <Tooltip>
                  <TooltipTrigger asChild>
                    <Button
                      variant="ghost"
                      size="icon"
                      className="size-7 text-foreground-tertiary hover:text-state-wake"
                      onClick={(e) => {
                        e.stopPropagation();
                        api.wakeAgent(a.agent_id).catch(() => {});
                      }}
                    >
                      <Zap className="size-4" />
                    </Button>
                  </TooltipTrigger>
                  <TooltipContent side="left">{t("chat.wake")}</TooltipContent>
                </Tooltip>
              </div>
            );
          })}
        </div>
      </aside>
    </div>
  );
}
