/**
 * AgentActivityLog — 单 worker 的步骤级活动流（第2层 UI）。
 *
 * 数据源是后端 tail 会话 JSONL 派生的 `agent_activity` swarm 事件（每个工具
 * 调用 / system 步骤一条）。本组件按「轮」分段：
 *   - 收到一条 activity → 追加到当前打开的轮（没有就新开一轮）。
 *   - 收到 agent_state 进入非运行态（idle / ready / exited / error）→ 关闭
 *     当前轮（"跑完折叠成一行"）。下一条 activity 来时再开新一轮。
 *
 * 后端没有显式的 turn_start 信号，所以用 state 转换近似轮边界——这与产品
 * 语义一致（一轮 = 一次连续运行）。最新一轮默认展开实时滚动，历史轮折叠
 * 成 "本轮 N 个工具 ▸" 一行，点开查看。
 *
 * 与 useBreadcrumbs / AgentDrawer MessagesTab 一样,组件级订阅 useSwarmFeed
 * 并按 agent_id 过滤——活动是 ephemeral 的,组件卸载即清空,不进全局 state,
 * 不污染对话历史。
 */

import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { ChevronRight, Loader2 } from "lucide-react";
import type { AgentActivity, SwarmEvent } from "../../api/types";
import { useSwarmFeed } from "../../hooks/useSwarmFeed";
import { cn } from "@/lib/cn";

interface Round {
  /** 该轮内按到达顺序的步骤。运行中的步骤(phase=running)会被同 seq 的
   *  ok/error 事件就地替换(后端先发 running 再发结算)。 */
  steps: AgentActivity[];
  /** 该轮是否已结束(agent 进入非运行态)。结束的轮默认折叠。 */
  closed: boolean;
}

/** 步骤时长文案：<1s 用 ms,否则用 s（一位小数）。running 不显示时长。 */
function fmtStepDuration(ms: number | undefined): string {
  if (ms == null) return "";
  if (ms < 1000) return `${ms}ms`;
  return `${(ms / 1000).toFixed(1)}s`;
}

export function AgentActivityLog({ agentId }: { agentId: string }) {
  const { t } = useTranslation();
  const [rounds, setRounds] = useState<Round[]>([]);
  // 用户手动展开的历史轮 index 集合（最新轮始终展开，不入此集合）。
  const [expanded, setExpanded] = useState<Set<number>>(new Set());
  // 每 1s tick 让运行中步骤的 elapsed 自然增长。
  const [now, setNow] = useState(Date.now());
  const scrollRef = useRef<HTMLDivElement>(null);

  // 切 agent → 清空（每个 worker 一份独立流）。
  useEffect(() => {
    setRounds([]);
    setExpanded(new Set());
  }, [agentId]);

  useEffect(() => {
    const id = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(id);
  }, []);

  useSwarmFeed({
    onEvent: (ev: SwarmEvent) => {
      if (ev.type === "agent_state") {
        if (ev.agent_id !== agentId) return;
        // 进入非运行态 → 关闭当前打开的轮（折叠）。
        const settled =
          ev.state === "idle" ||
          ev.state === "ready" ||
          ev.state === "exited" ||
          ev.state === "error";
        if (!settled) return;
        setRounds((prev) => {
          if (prev.length === 0 || prev[prev.length - 1].closed) return prev;
          const next = prev.slice();
          next[next.length - 1] = { ...next[next.length - 1], closed: true };
          return next;
        });
        return;
      }
      if (ev.type !== "agent_activity" || ev.agent_id !== agentId) return;
      const act: AgentActivity = {
        agent_id: ev.agent_id,
        kind: ev.kind,
        label: ev.label,
        phase: ev.phase,
        seq: ev.seq,
        duration_ms: ev.duration_ms,
        at: ev.at,
      };
      setRounds((prev) => {
        const next = prev.slice();
        let cur = next[next.length - 1];
        // 没有打开的轮 → 新开一轮。
        if (!cur || cur.closed) {
          cur = { steps: [], closed: false };
          next.push(cur);
        } else {
          cur = { ...cur, steps: cur.steps.slice() };
          next[next.length - 1] = cur;
        }
        // 同 seq 的结算事件(ok/error)就地替换它的 running 占位。
        const at = cur.steps.findIndex((s) => s.seq === act.seq);
        if (at >= 0) cur.steps[at] = act;
        else cur.steps.push(act);
        return next;
      });
    },
  });

  // 最新轮有新步骤时滚到底。
  useEffect(() => {
    const el = scrollRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [rounds]);

  const totalSteps = useMemo(
    () => rounds.reduce((n, r) => n + r.steps.length, 0),
    [rounds],
  );

  if (totalSteps === 0) {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-2 px-6 text-center text-foreground-tertiary">
        <span className="font-caption text-sm">{t("agent.activity.empty")}</span>
        <span className="max-w-[44ch] font-caption text-[11px] leading-relaxed">
          {t("agent.activity.emptyHint")}
        </span>
      </div>
    );
  }

  return (
    <div ref={scrollRef} className="flex h-full flex-col gap-2 overflow-y-auto p-4">
      {rounds.map((round, i) => {
        const isLatest = i === rounds.length - 1;
        const open = isLatest || !round.closed || expanded.has(i);
        return (
          <div
            key={i}
            className="rounded-md border border-border-subtle bg-surface-primary"
          >
            {/* 折叠的历史轮 → 一行摘要;展开的轮 → 头 + 步骤列表。 */}
            {round.closed && !open ? (
              <button
                type="button"
                onClick={() =>
                  setExpanded((prev) => {
                    const n = new Set(prev);
                    n.add(i);
                    return n;
                  })
                }
                className="flex w-full items-center gap-2 px-3 py-2 text-left hover:bg-surface-tertiary"
              >
                <ChevronRight className="size-3.5 shrink-0 text-foreground-tertiary" />
                <span className="font-caption text-[11px] text-foreground-secondary">
                  {t("agent.activity.roundSummary", {
                    count: round.steps.length,
                  })}
                </span>
                {round.steps.some((s) => s.phase === "error") && (
                  <span className="ml-auto font-mono text-[11px] text-status-danger">
                    ✕
                  </span>
                )}
              </button>
            ) : (
              <div className="flex flex-col">
                {round.closed && (
                  <button
                    type="button"
                    onClick={() =>
                      setExpanded((prev) => {
                        const n = new Set(prev);
                        n.delete(i);
                        return n;
                      })
                    }
                    className="flex w-full items-center gap-2 border-b border-border-subtle px-3 py-1.5 text-left hover:bg-surface-tertiary"
                  >
                    <ChevronRight className="size-3.5 shrink-0 rotate-90 text-foreground-tertiary" />
                    <span className="font-caption text-[10px] uppercase tracking-wide text-foreground-tertiary">
                      {t("agent.activity.roundSummary", {
                        count: round.steps.length,
                      })}
                    </span>
                  </button>
                )}
                <ul className="flex flex-col">
                  {round.steps.map((s, j) => (
                    <li
                      key={`${s.seq}-${j}`}
                      className="flex items-center gap-2 px-3 py-1.5"
                    >
                      <StepGlyph phase={s.phase} />
                      <span
                        className={cn(
                          "min-w-0 flex-1 truncate font-mono text-[11px]",
                          s.phase === "error"
                            ? "text-status-danger"
                            : "text-foreground-primary",
                        )}
                        title={s.label}
                      >
                        {s.label}
                      </span>
                      <span className="shrink-0 font-mono text-[10px] text-foreground-tertiary">
                        {s.phase === "running"
                          ? fmtStepDuration(Math.max(0, now - s.at))
                          : fmtStepDuration(s.duration_ms)}
                      </span>
                    </li>
                  ))}
                </ul>
              </div>
            )}
          </div>
        );
      })}
    </div>
  );
}

function StepGlyph({ phase }: { phase: AgentActivity["phase"] }) {
  if (phase === "running") {
    return (
      <Loader2 className="size-3.5 shrink-0 animate-spin text-accent-primary" />
    );
  }
  if (phase === "ok") {
    return (
      <span className="flex size-3.5 shrink-0 items-center justify-center font-mono text-[11px] text-status-success">
        ✓
      </span>
    );
  }
  return (
    <span className="flex size-3.5 shrink-0 items-center justify-center font-mono text-[11px] text-status-danger">
      ✕
    </span>
  );
}
