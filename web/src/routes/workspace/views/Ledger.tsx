/**
 * Ledger view — orchestrator 的双 ledger 主面板,Magentic-One 模式核心 UI。
 *
 * 左右分栏:
 *   - 左: Task Ledger (blackboard key `task.ledger.md`) — facts / guesses /
 *         acceptance / plan
 *   - 右: Progress Ledger (`progress.ledger.md`) — status / current_step /
 *         assignments / blockers
 *
 * 数据来源都是 blackboard 直接读,跟 Context.tsx 复用同一套 api.readBlackboard
 * 接口。每次有 blackboard_changed 事件就 refetch — wake-coordinator 已经在
 * 推这个事件,我们只是把已有信息渲染成对用户友好的形态。
 *
 * 视觉是双卡片 + markdown 渲染 + 顶部 "最后更新 XX 秒前"。没有任何编辑能力 —
 * orchestrator 是唯一 writer,用户是 reader。
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { useTranslation } from "react-i18next";
import { ClipboardList, RefreshCw, Activity, Radio } from "lucide-react";
import { api } from "../../../api/http";
import type { BlackboardEntry, BlackboardSnapshot, SwarmEvent } from "../../../api/types";
import { useSwarmFeed } from "../../../hooks/useSwarmFeed";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/cn";
import { useWorkspaceContext } from "../Shell";

function fmtAgo(at: number | null, nowTick: number): string {
  if (at == null) return "—";
  const sec = Math.max(0, Math.floor((nowTick - at) / 1000));
  if (sec < 60) return `${sec}s 前`;
  const min = Math.floor(sec / 60);
  if (min < 60) return `${min}m 前`;
  const hr = Math.floor(min / 60);
  return `${hr}h 前`;
}

interface LedgerSnap {
  content: string;
  at: number | null;
  error: string | null;
}

function emptySnap(): LedgerSnap {
  return { content: "", at: null, error: null };
}

export default function LedgerView() {
  const { t } = useTranslation();
  const { workspace } = useWorkspaceContext();
  // Workspace-scoped blackboard paths — multiple workspaces share one
  // blackboard tree, so the orchestrator writes (and we read) the ledger
  // under `<workspace_id>/...` to avoid collision.
  const taskKey = `${workspace.workspaceId}/task.ledger.md`;
  const progressKey = `${workspace.workspaceId}/progress.ledger.md`;
  const [task, setTask] = useState<LedgerSnap>(emptySnap());
  const [progress, setProgress] = useState<LedgerSnap>(emptySnap());
  // 近况 (worker breadcrumbs) — { role_label: { content, at } } keyed by
  // the part of the blackboard path before `.progress.md`. orchestrator
  // tells each worker to overwrite `<workspace_id>/<role>.progress.md`
  // at every milestone (deps installed, build passing, etc.) so this
  // pane lights up while npm install / cargo build / etc. are running.
  const [breadcrumbs, setBreadcrumbs] = useState<
    { role: string; content: string; at: number }[]
  >([]);
  const [refreshing, setRefreshing] = useState(false);
  // tick 用来让"XX 秒前"动起来,1s 一次刷新
  const [nowTick, setNowTick] = useState(Date.now());
  useEffect(() => {
    const i = window.setInterval(() => setNowTick(Date.now()), 1000);
    return () => window.clearInterval(i);
  }, []);

  const loadOne = useCallback(
    async (key: string, setter: (s: LedgerSnap) => void) => {
      try {
        const snap = (await api.readBlackboard(key)) as BlackboardSnapshot | null;
        if (snap) {
          setter({ content: snap.content, at: snap.at, error: null });
        } else {
          setter({
            content: "",
            at: null,
            error: null, // 没写过不算错,显示空态
          });
        }
      } catch (e) {
        setter({
          content: "",
          at: null,
          error: (e as Error).message,
        });
      }
    },
    [],
  );

  const loadBreadcrumbs = useCallback(async () => {
    try {
      const all = (await api.listBlackboard()) as BlackboardEntry[];
      const prefix = `${workspace.workspaceId}/`;
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
      const rows = snaps.filter(
        (s): s is { role: string; content: string; at: number } => s !== null,
      );
      // newest first so the most recent worker activity is at the top
      rows.sort((a, b) => b.at - a.at);
      setBreadcrumbs(rows);
    } catch {
      setBreadcrumbs([]);
    }
  }, [workspace.workspaceId]);

  const refresh = useCallback(async () => {
    setRefreshing(true);
    await Promise.all([
      loadOne(taskKey, setTask),
      loadOne(progressKey, setProgress),
      loadBreadcrumbs(),
    ]);
    setRefreshing(false);
  }, [loadOne, taskKey, progressKey, loadBreadcrumbs]);

  useEffect(() => {
    refresh();
  }, [refresh]);

  // 监听 blackboard_changed —— orchestrator 写 ledger 时立即重拉,
  // 别等用户手动 refresh。
  const lastEventIdRef = useRef<number>(0);
  useSwarmFeed({
    onEvent: (ev: SwarmEvent) => {
      if (ev.type !== "blackboard_changed") return;
      if (ev.id === lastEventIdRef.current) return;
      const isLedger = ev.path === taskKey || ev.path === progressKey;
      const isBreadcrumb =
        ev.path.startsWith(`${workspace.workspaceId}/`) &&
        ev.path.endsWith(".progress.md");
      if (!isLedger && !isBreadcrumb) return;
      lastEventIdRef.current = ev.id;
      refresh();
    },
    onReconnect: () => refresh(),
  });

  const taskAgo = useMemo(() => fmtAgo(task.at, nowTick), [task.at, nowTick]);
  const progressAgo = useMemo(
    () => fmtAgo(progress.at, nowTick),
    [progress.at, nowTick],
  );

  return (
    <div className="flex min-h-0 flex-1 flex-col bg-surface-primary">
      {/* 顶栏:刷新 + 简短说明 */}
      <div className="flex shrink-0 items-center justify-between border-b border-border-subtle px-5 py-3">
        <div className="flex flex-col">
          <h2 className="font-heading text-sm font-semibold text-foreground-primary">
            {t("ledger.title", "AI 工作台账")}
          </h2>
          <p className="font-caption text-[11px] text-foreground-tertiary">
            {t("ledger.subtitle", "orchestrator 的思考过程都在这里。左侧是任务台账(目标 + 计划),右侧是进展状态。")}
          </p>
        </div>
        <Button
          variant="outline"
          size="sm"
          onClick={refresh}
          disabled={refreshing}
          className="gap-1.5"
        >
          <RefreshCw className={cn("size-3.5", refreshing && "animate-spin")} />
          {t("ledger.refresh", "刷新")}
        </Button>
      </div>

      {/* 主体:上半 = 任务 + 进展(双卡),下半 = worker 近况(通栏) */}
      <div className="flex min-h-0 flex-1 flex-col gap-4 overflow-hidden p-5">
        <div className="flex min-h-0 flex-1 gap-4 overflow-hidden">
          <LedgerCard
            icon={<ClipboardList className="size-4 text-accent-primary" />}
            title={t("ledger.taskTitle", "任务台账")}
            subtitle={t("ledger.taskSubtitle", "目标 · 假设 · 计划(DAG)")}
            ago={taskAgo}
            snap={task}
            emptyHint={t(
              "ledger.taskEmpty",
              "还没写。orchestrator 第一次 wake 时会自动建立。",
            )}
          />
          <LedgerCard
            icon={<Activity className="size-4 text-state-success" />}
            title={t("ledger.progressTitle", "进展状态")}
            subtitle={t(
              "ledger.progressSubtitle",
              "当前步骤 · 派出去的活 · 卡点",
            )}
            ago={progressAgo}
            snap={progress}
            emptyHint={t(
              "ledger.progressEmpty",
              "还没写。orchestrator 派活时会实时更新。",
            )}
          />
        </div>
        <BreadcrumbsCard rows={breadcrumbs} nowTick={nowTick} />
      </div>
    </div>
  );
}

/** Worker 近况通栏 —— 把每个 worker 写到 `<role>.progress.md` 的最新一行
 *  当成一条心跳显示。Magentic-One 论文里没有这玩意,是 flockmux 的补丁:
 *  Bash / npm install 这种秒不出动静的任务期间,只有"派活了…然后呢?"对用户
 *  来说是个黑盒。orchestrator 在 worker prompt 里要求每个里程碑都覆写这个
 *  文件,这里就把所有 workers 的最新心跳铺出来,newest first。 */
function BreadcrumbsCard({
  rows,
  nowTick,
}: {
  rows: { role: string; content: string; at: number }[];
  nowTick: number;
}) {
  return (
    <div className="flex shrink-0 flex-col overflow-hidden rounded-lg border border-border-subtle bg-surface-elevated">
      <div className="flex shrink-0 items-center gap-2 border-b border-border-subtle px-4 py-3">
        <Radio className="size-4 text-accent-primary" />
        <div className="flex min-w-0 flex-1 flex-col">
          <span className="font-heading text-sm font-semibold text-foreground-primary">
            近况
          </span>
          <span className="font-caption text-[11px] text-foreground-tertiary">
            worker 们最近的心跳(每完成一步会自动写)
          </span>
        </div>
        <span className="shrink-0 font-caption text-[10px] text-foreground-tertiary">
          {rows.length} 个 worker
        </span>
      </div>
      <div className="max-h-[40vh] overflow-auto px-4 py-3">
        {rows.length === 0 ? (
          <p className="font-caption text-xs text-foreground-tertiary">
            还没有 worker 写过心跳。派出去的 worker 完成里程碑(install / build /
            写代码 等)时会在这里出现一行。
          </p>
        ) : (
          <ul className="flex flex-col gap-2">
            {rows.map((r) => (
              <li
                key={r.role}
                className="flex items-baseline gap-3 rounded-md bg-surface-tertiary px-3 py-2"
              >
                <span className="shrink-0 font-mono text-[11px] font-semibold text-accent-primary">
                  {r.role}
                </span>
                <span className="min-w-0 flex-1 truncate font-body text-[13px] text-foreground-primary">
                  {r.content}
                </span>
                <span className="shrink-0 font-caption text-[10px] text-foreground-tertiary">
                  {fmtAgo(r.at, nowTick)}
                </span>
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}

function LedgerCard({
  icon,
  title,
  subtitle,
  ago,
  snap,
  emptyHint,
}: {
  icon: React.ReactNode;
  title: string;
  subtitle: string;
  ago: string;
  snap: LedgerSnap;
  emptyHint: string;
}) {
  return (
    <div className="flex min-w-0 flex-1 flex-col overflow-hidden rounded-lg border border-border-subtle bg-surface-elevated">
      <div className="flex shrink-0 items-center gap-2 border-b border-border-subtle px-4 py-3">
        {icon}
        <div className="flex min-w-0 flex-1 flex-col">
          <span className="truncate font-heading text-sm font-semibold text-foreground-primary">
            {title}
          </span>
          <span className="truncate font-caption text-[11px] text-foreground-tertiary">
            {subtitle}
          </span>
        </div>
        <span className="shrink-0 font-caption text-[10px] text-foreground-tertiary">
          {ago}
        </span>
      </div>
      <div className="min-h-0 flex-1 overflow-auto px-4 py-3">
        {snap.error ? (
          <p className="font-caption text-xs text-state-danger">
            读取失败: {snap.error}
          </p>
        ) : snap.content ? (
          <article className="prose prose-sm max-w-none font-body text-[13px] text-foreground-primary">
            <ReactMarkdown remarkPlugins={[remarkGfm]}>
              {snap.content}
            </ReactMarkdown>
          </article>
        ) : (
          <p className="font-caption text-xs text-foreground-tertiary">
            {emptyHint}
          </p>
        )}
      </div>
    </div>
  );
}
