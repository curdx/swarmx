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

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
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
import { Radio, Users, Zap } from "lucide-react";
import { cn } from "@/lib/cn";
import {
  roleColorClass as roleColor,
  inferAgentStatus,
  agentStatusLabel,
  agentStatusDotClass,
  agentStatusIsTyping,
} from "@/lib/agent";
import type {
  BlackboardEntry,
  BlackboardSnapshot,
  MessageRecord,
  SwarmEvent,
} from "../../../api/types";
import { useSwarmFeed } from "../../../hooks/useSwarmFeed";
import { useWorkspaceContext } from "../Shell";

/** Pull every `<workspace_id>/<thread_slug>/<role>.progress.md` breadcrumb the
 *  workers in this DIRECTION have written, newest first. `prefix` is the
 *  direction key prefix (`<workspace_id>/<thread_slug>/`). Same source the
 *  Ledger "近况" card uses — rendered slim inline in the chat sidebar so users
 *  don't switch tabs to know a worker is alive during npm install / build. */
function useBreadcrumbs(prefix: string) {
  const [rows, setRows] = useState<
    { role: string; content: string; at: number }[]
  >([]);
  const reload = useCallback(async () => {
    try {
      const all = (await api.listBlackboard()) as BlackboardEntry[];
      const suffix = ".progress.md";
      const candidates = all.filter(
        (e) => e.path.startsWith(prefix) && e.path.endsWith(suffix),
      );
      const snaps = await Promise.all(
        candidates.map(async (e) => {
          try {
            const snap = (await api.readBlackboard(e.path)) as BlackboardSnapshot | null;
            if (!snap) return null;
            const role = e.path.slice(prefix.length, -suffix.length);
            return { role, content: snap.content.trim(), at: snap.at };
          } catch {
            return null;
          }
        }),
      );
      const out = snaps.filter(
        (s): s is { role: string; content: string; at: number } => s !== null,
      );
      out.sort((a, b) => b.at - a.at);
      setRows(out);
    } catch {
      setRows([]);
    }
  }, [prefix]);
  useEffect(() => {
    reload();
  }, [reload]);
  const lastIdRef = useRef<number>(0);
  useSwarmFeed({
    onEvent: (ev: SwarmEvent) => {
      if (ev.type !== "blackboard_changed") return;
      if (ev.id === lastIdRef.current) return;
      if (!ev.path.startsWith(prefix)) return;
      if (!ev.path.endsWith(".progress.md")) return;
      lastIdRef.current = ev.id;
      reload();
    },
    onReconnect: () => reload(),
  });
  return rows;
}

function fmtBreadcrumbAgo(at: number, now: number): string {
  const sec = Math.max(0, Math.floor((now - at) / 1000));
  if (sec < 60) return `${sec}s 前`;
  const min = Math.floor(sec / 60);
  if (min < 60) return `${min}m 前`;
  const hr = Math.floor(min / 60);
  return `${hr}h 前`;
}

/** 成员列表一行的"AI 当前在干啥"视觉。微信式简洁:
 *  - typing 动画(··· 闪烁)  → responding / working
 *  - 绿点                 → idle / awaiting_user (默认 "在线")
 *  - 灰点 + "已结束"       → exited
 *  - 灰点 + "已暂停"       → paused
 *  - 黄点 + "启动中"       → shim 还没 ready */
function statusDot(
  a: AgentInfo,
  messages: MessageRecord[],
  t: (k: string) => string,
) {
  if (a.killed_at)
    return { typing: false, className: "bg-state-idle", label: t("chat.exited") };
  if (a.shim_exit != null)
    return { typing: false, className: "bg-state-idle", label: t("chat.shimExit") };
  if (!a.shim_ready)
    return { typing: false, className: "bg-state-wake", label: t("chat.starting") };
  const status = inferAgentStatus(a, messages);
  return {
    typing: agentStatusIsTyping(status),
    className: agentStatusDotClass(status),
    label: agentStatusLabel(status),
  };
}

/** 三点 typing 动画 —— 跟微信"对方正在输入"风格一致。三个圆点错相位
 *  bounce,纯 CSS,不依赖外部库。 */
function TypingDots() {
  return (
    <span
      className="inline-flex items-center gap-0.5"
      aria-label="AI 正在输入"
      title="AI 正在输入"
    >
      <span className="inline-block size-1.5 animate-bounce rounded-full bg-accent-primary [animation-delay:-0.3s]" />
      <span className="inline-block size-1.5 animate-bounce rounded-full bg-accent-primary [animation-delay:-0.15s]" />
      <span className="inline-block size-1.5 animate-bounce rounded-full bg-accent-primary" />
    </span>
  );
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
    threadSlug,
    // Members + room are scoped to the ACTIVE direction (thread), not the whole
    // workspace, so each direction is its own self-contained chat.
    threadMembers: activeMembers,
    allAliveAgents,
    threadAgentIds,
    liveMessage,
    liveRead,
    unreadByFrom: activeWorkspaceUnread,
    jumpUnreadTick,
    openAgent,
    refreshAgents,
    // unreadByFrom in OutletContext is workspace-filtered; the right-side
    // members list wants raw agent-id → count so it can show the small
    // red badge per row. We re-derive it by indexing into the filtered
    // map (same keys, just used differently).
  } = useWorkspaceContext();

  // ── Revive orchestrator for a member-less workspace ──────────────────
  // A finished workspace isn't auto-respawned on server boot (that would burn
  // an LLM turn re-concluding "done"). So a returning user can land on a
  // 0-member room. This button runs the `init` spell on demand to bring the
  // orchestrator back; its bootstrap detects the existing ledger and
  // short-circuits, so it just becomes available to chat with again.
  const [reviving, setReviving] = useState(false);
  const reviveOrchestrator = useCallback(async () => {
    if (reviving) return;
    setReviving(true);
    try {
      await api.runSpell({
        name: "init",
        task: "",
        workspace_dir: workspace.path,
        workspace_id: workspace.workspaceId,
      });
      refreshAgents();
    } catch (e) {
      // eslint-disable-next-line no-console
      console.warn("revive orchestrator failed", e);
    } finally {
      setReviving(false);
    }
  }, [reviving, workspace.path, workspace.workspaceId, refreshAgents]);

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

  // 轻量 messages cache —— 给成员列表的语义状态推导喂数据。MessagesPanel
  // 也自己拉一份(它需要展示 200 条 + filter + 排序),这里只需要"最近一条
  // inbound/outbound per agent"这点信息,但写两套缓存不值,直接 fetch 一次
  // + liveMessage append。每次切 workspace 重新拉(workspace.id 变化)。
  const [recentMessages, setRecentMessages] = useState<MessageRecord[]>([]);
  useEffect(() => {
    let cancelled = false;
    api
      .listMessages({ limit: 200 })
      .then((rows) => {
        if (!cancelled) setRecentMessages(rows);
      })
      .catch(() => {
        /* best-effort — statusDot falls back to "online" */
      });
    return () => {
      cancelled = true;
    };
  }, [workspace.id]);
  useEffect(() => {
    if (!liveMessage) return;
    setRecentMessages((prev) =>
      prev.some((m) => m.id === liveMessage.id) ? prev : [...prev, liveMessage],
    );
  }, [liveMessage]);
  // worker 心跳 — orchestrator 让每个 worker 每完成一步覆写
  // `<workspace_id>/<role>.progress.md`,我们订阅这些 key 实时显示在右栏。
  // 同样的数据 Ledger 视图也用,但 chat 这边给个 slim 列表,用户不用切 tab。
  const breadcrumbsRaw = useBreadcrumbs(
    `${workspace.workspaceId}/${threadSlug}/`,
  );

  // 每 5s tick 让 statusDot 重新评估时间窗(responding/working 都有时间窗,
  // 没消息进来时也要让它们自然衰减成 idle/awaiting)。
  const [statusTick, setStatusTick] = useState(0);
  useEffect(() => {
    const i = window.setInterval(() => setStatusTick((t) => t + 1), 5000);
    return () => window.clearInterval(i);
  }, []);
  void statusTick; // referenced via re-render

  // Re-resolve "XX 秒前" labels on each statusTick so the side panel
  // stays current even when no new event lands.
  const breadcrumbs = useMemo(
    () =>
      breadcrumbsRaw.map((b) => ({
        ...b,
        ago: fmtBreadcrumbAgo(b.at, Date.now()),
      })),
    // eslint-disable-next-line react-hooks/exhaustive-deps -- intentional tick dep
    [breadcrumbsRaw, statusTick],
  );

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
          workspaceAgentIds={threadAgentIds}
          workspaceSlug={workspace.id}
          jumpUnreadTick={jumpUnreadTick}
          onOpenAgent={openAgent}
          taskActivityBelow={
            <TaskActivity tasks={tasks} onDismiss={dismissTask} />
          }
        />
      </section>

      {/* surface-secondary (not -elevated) so this rail matches the left
          workspace rail — both side rails read as panels, keeping the
          3-column layout balanced instead of the right one blending into
          the white center canvas. No-op in dark (both resolve slate-800). */}
      <aside className="flex w-[340px] shrink-0 flex-col border-l border-border-subtle bg-surface-secondary">
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
            <div className="flex flex-col items-start gap-2 px-3 py-2">
              <p className="font-caption text-xs text-foreground-tertiary">
                {t("chat.noMembers")}
              </p>
              <Button
                size="sm"
                variant="outline"
                onClick={reviveOrchestrator}
                disabled={reviving}
                className="gap-1.5"
              >
                <Zap className="size-3.5" />
                {reviving ? t("chat.reviving") : t("chat.reviveOrchestrator")}
              </Button>
            </div>
          )}
          {(() => {
            // orchestrator 永远置顶。它是用户在 workspace 里唯一的常驻
            // 对接人 (Magentic-One PM 角色),worker 完工后用户应该回头
            // 找它继续 — 视觉上必须立刻可分辨。其他 agent 按原顺序。
            const sorted = [...activeMembers].sort((a, b) => {
              const ao = a.role === "orchestrator" ? 0 : 1;
              const bo = b.role === "orchestrator" ? 0 : 1;
              return ao - bo;
            });
            return sorted;
          })().map((a) => {
            const dot = statusDot(a, recentMessages, t);
            const unread = activeWorkspaceUnread[a.agent_id] ?? 0;
            const isOrchestrator = a.role === "orchestrator";
            return (
              <div
                key={a.agent_id}
                onClick={() => openAgent(a.agent_id)}
                className={cn(
                  "flex cursor-pointer items-center gap-3 rounded-md px-3 py-2 hover:bg-surface-tertiary",
                )}
              >
                <Avatar
                  className={cn(
                    "size-8 shrink-0",
                    // 金边告诉用户"这个是主理人,长期在线"。没有 ring
                    // 的就是临时 worker — 干完一件事就会消失。
                    isOrchestrator &&
                      "ring-2 ring-accent-primary ring-offset-2 ring-offset-surface-secondary",
                  )}
                  title={a.role}
                >
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
                    {isOrchestrator && (
                      <span className="shrink-0 rounded-full bg-accent-primary-soft px-1.5 py-0.5 font-caption text-[9px] font-semibold text-accent-primary">
                        {t("chat.pmBadge", "主理人")}
                      </span>
                    )}
                    {dot.typing ? (
                      <TypingDots />
                    ) : (
                      <>
                        {dot.className && (
                          <span
                            className={cn("size-1.5 rounded-full", dot.className)}
                            title={dot.label || undefined}
                          />
                        )}
                        {dot.label && (
                          <span className="font-caption text-[10px] text-foreground-tertiary">
                            {dot.label}
                          </span>
                        )}
                      </>
                    )}
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
        {breadcrumbs.length > 0 && (
          <div className="shrink-0 border-t border-border-subtle">
            <div className="flex items-center gap-2 px-4 pt-3 pb-2">
              <Radio className="size-3.5 text-accent-primary" />
              <span className="font-heading text-[11px] font-semibold uppercase tracking-wider text-foreground-tertiary">
                {t("chat.breadcrumbsTitle", "近况")}
              </span>
              <span className="ml-auto font-caption text-[10px] text-foreground-tertiary">
                {breadcrumbs.length}
              </span>
            </div>
            <ul className="flex max-h-[35vh] flex-col gap-1.5 overflow-y-auto px-2 pb-3">
              {breadcrumbs.map((b) => (
                <li
                  key={b.role}
                  className="rounded-md bg-surface-tertiary px-3 py-2"
                >
                  <div className="flex items-baseline gap-2">
                    <span className="shrink-0 font-mono text-[10px] font-semibold text-accent-primary">
                      {b.role}
                    </span>
                    <span className="ml-auto shrink-0 font-caption text-[9px] text-foreground-tertiary">
                      {b.ago}
                    </span>
                  </div>
                  <p className="mt-0.5 break-words font-body text-[11px] leading-snug text-foreground-primary">
                    {b.content}
                  </p>
                </li>
              ))}
            </ul>
          </div>
        )}
      </aside>
    </div>
  );
}
