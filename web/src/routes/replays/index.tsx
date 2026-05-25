/**
 * Replays Library — Pencil frame SFQc8.
 *
 * Card grid (3 cols on wide screens) of every recording flockmux-recorder
 * has produced. Replaces the legacy table inside RecordingsPanel for the
 * product surface; the panel itself stays alive on /debug.
 *
 * The cast thumbnail is intentionally minimal — we render an ANSI-flavoured
 * static plate with role colour + agent id rather than fetching the cast
 * head. Decoding a `.cast` to derive the first frame is doable but adds
 * network + parsing cost per card; deferred until users ask for it.
 *
 * Cards link to /replays/:id for the fullscreen player (Pencil v1radc).
 * No inline player — that path proved too cramped (commit 005defc).
 */

import { useEffect, useMemo, useState } from "react";
import { Link } from "react-router-dom";
import { Download, Play, RefreshCw, Search } from "lucide-react";
import { api } from "../../api/http";
import type { RecordingInfo } from "../../api/types";
import { useSwarmFeed } from "../../hooks/useSwarmFeed";
import { cn } from "@/lib/cn";

const ROLE_BG: Record<string, string> = {
  planner: "bg-agent-planner",
  backend: "bg-agent-backend",
  frontend: "bg-agent-frontend",
  architect: "bg-agent-architect",
  critic: "bg-agent-critic",
  test: "bg-agent-test",
};

function inferRole(agentId: string): string {
  // agent_id format: <cli>-<hash>; role is not embedded. We only get role
  // off /api/agent, which is a heavier call. For the cards we'd rather
  // colour by cli prefix as a cheap proxy.
  const [cli] = agentId.split("-");
  return cli ?? "unknown";
}

function roleColor(agentId: string) {
  return ROLE_BG[inferRole(agentId)] ?? "bg-state-idle";
}

function formatTime(ms: number): string {
  return new Date(ms).toLocaleString();
}

function formatDuration(ms: number | null): string {
  if (ms == null) return "—";
  if (ms < 1000) return `${ms}ms`;
  const s = ms / 1000;
  if (s < 60) return `${s.toFixed(1)}s`;
  const m = Math.floor(s / 60);
  const rem = (s - m * 60).toFixed(0);
  return `${m}m${rem}s`;
}

export default function ReplaysIndex() {
  const [items, setItems] = useState<RecordingInfo[]>([]);
  const [filter, setFilter] = useState("");
  const [activeTag, setActiveTag] = useState<string>("all");
  const [error, setError] = useState<string | null>(null);

  const refresh = async () => {
    try {
      const rows = await api.listRecordings();
      setItems(rows);
      setError(null);
    } catch (e) {
      setError((e as Error).message);
    }
  };

  useEffect(() => {
    refresh();
  }, []);

  // Re-list whenever an agent exits — that's when recorder finalizes the
  // file and a previously-live cast picks up a duration.
  useSwarmFeed({
    onEvent: (ev) => {
      if (ev.type === "agent_state" && ev.state === "exited") refresh();
    },
    onReconnect: () => refresh(),
  });

  const tags = useMemo(() => {
    const set = new Set<string>();
    for (const r of items) set.add(inferRole(r.agent_id));
    return ["all", ...Array.from(set).sort()];
  }, [items]);

  const filtered = useMemo(() => {
    const q = filter.trim().toLowerCase();
    return items.filter((r) => {
      if (activeTag !== "all" && inferRole(r.agent_id) !== activeTag) return false;
      if (q && !r.agent_id.toLowerCase().includes(q) && !r.id.toLowerCase().includes(q)) {
        return false;
      }
      return true;
    });
  }, [items, filter, activeTag]);

  return (
    <div className="flex h-full flex-col bg-surface-primary">
      {/* Header */}
      <header className="flex h-14 shrink-0 items-center gap-3 border-b border-border-subtle bg-surface-elevated px-5">
        <span className="flex size-8 items-center justify-center rounded-md bg-accent-peach-soft">
          <Play className="size-4 text-accent-peach-deep" />
        </span>
        <div className="flex flex-col">
          <h1 className="font-heading text-sm font-semibold text-foreground-primary">
            录像库
          </h1>
          <span className="font-caption text-[10px] text-foreground-tertiary">
            {items.length} 个录像
          </span>
        </div>
        <span className="flex-1" />
        <div className="flex h-8 w-60 items-center gap-2 rounded-md bg-surface-tertiary px-3">
          <Search className="size-3.5 text-foreground-tertiary" />
          <input
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            placeholder="按 agent / id 搜索"
            className="min-w-0 flex-1 bg-transparent text-xs text-foreground-primary placeholder:text-foreground-tertiary focus:outline-none"
          />
        </div>
        <button
          onClick={refresh}
          className="flex size-8 items-center justify-center rounded-md bg-surface-tertiary text-foreground-secondary hover:bg-surface-secondary"
          title="刷新"
        >
          <RefreshCw className="size-4" />
        </button>
      </header>

      {/* Tag bar */}
      <div className="flex h-11 shrink-0 items-center gap-1.5 border-b border-border-subtle bg-surface-secondary px-5">
        {tags.map((t) => {
          const active = t === activeTag;
          return (
            <button
              key={t}
              onClick={() => setActiveTag(t)}
              className={cn(
                "rounded-full px-3 py-1 text-xs transition-colors",
                active
                  ? "bg-accent-peach text-foreground-on-accent"
                  : "border border-border-subtle bg-surface-elevated text-foreground-secondary hover:bg-surface-tertiary",
              )}
            >
              {t === "all" ? "全部" : t}
            </button>
          );
        })}
        <span className="flex-1" />
        <span className="font-caption text-xs text-foreground-tertiary">
          {filtered.length} / {items.length} 条
        </span>
      </div>

      {/* Grid */}
      <div className="min-h-0 flex-1 overflow-y-auto p-5">
        {error && (
          <div className="mb-3 rounded-md border border-state-danger/40 bg-status-danger-soft px-3 py-2 text-xs text-state-danger">
            {error}
          </div>
        )}
        {filtered.length === 0 ? (
          <div className="flex h-full flex-col items-center justify-center gap-2 text-foreground-tertiary">
            <Play className="size-8 opacity-40" />
            <p className="font-caption text-sm">没有匹配的录像</p>
          </div>
        ) : (
          <div className="grid grid-cols-1 gap-4 md:grid-cols-2 xl:grid-cols-3">
            {filtered.map((r) => {
              const live = r.finalized_at == null;
              const role = inferRole(r.agent_id);
              return (
                <article
                  key={r.id}
                  className="group overflow-hidden rounded-lg border border-border-subtle bg-surface-elevated transition-shadow hover:shadow-lg"
                >
                  {/* Card head: role chip + live dot */}
                  <div className="flex items-center gap-2 px-3 py-2">
                    <span
                      className={cn(
                        "flex h-5 items-center rounded-full px-2 font-mono text-[10px] font-medium uppercase text-foreground-on-accent",
                        roleColor(r.agent_id),
                      )}
                    >
                      {role}
                    </span>
                    <span className="truncate font-mono text-[10px] text-foreground-tertiary">
                      {r.agent_id}
                    </span>
                    <span className="flex-1" />
                    <span
                      className={cn(
                        "rounded-full px-2 py-0.5 font-caption text-[10px]",
                        live
                          ? "bg-status-running-soft text-status-running"
                          : "bg-surface-tertiary text-foreground-tertiary",
                      )}
                      title={live ? "录像还在进行" : "录像已完结"}
                    >
                      {live ? "● live" : "○ completed"}
                    </span>
                  </div>

                  {/* Cast thumbnail (static plate) */}
                  <Link
                    to={`/replays/${encodeURIComponent(r.id)}`}
                    className="relative flex h-32 flex-col gap-1 overflow-hidden border-y border-[#1F1F1F] bg-term-bg px-4 py-3 font-mono text-[10px] leading-snug text-term-fg"
                  >
                    <div className="text-term-green">
                      ❯ <span className="text-term-fg">flockmux replay {r.id.slice(0, 8)}</span>
                    </div>
                    <div className="text-term-dim">
                      # agent={r.agent_id}
                    </div>
                    <div className="text-term-dim">
                      # geom={r.cols}×{r.rows}
                      {r.duration_ms != null && (
                        <> · dur={formatDuration(r.duration_ms)}</>
                      )}
                    </div>
                    <div className="text-term-blue">
                      {live ? "▶ recording…" : "✓ ready to replay"}
                    </div>
                    {/* hover overlay */}
                    <div className="absolute inset-0 flex items-center justify-center bg-black/40 opacity-0 transition-opacity group-hover:opacity-100">
                      <span className="flex items-center gap-1.5 rounded-full bg-accent-peach px-4 py-2 text-xs font-medium text-foreground-on-accent">
                        <Play className="size-3.5" />
                        播放
                      </span>
                    </div>
                  </Link>

                  {/* Card foot */}
                  <div className="flex items-center gap-2 px-3 py-2 text-foreground-secondary">
                    <span className="font-caption text-[11px]">
                      {formatTime(r.started_at)}
                    </span>
                    <span className="flex-1" />
                    <Link
                      to={`/replays/${encodeURIComponent(r.id)}`}
                      className="flex h-7 items-center gap-1 rounded-md bg-accent-peach px-2.5 text-xs text-foreground-on-accent hover:bg-accent-peach-deep"
                    >
                      <Play className="size-3" />
                      播放
                    </Link>
                    <a
                      href={api.recordingCastUrl(r.id)}
                      download={`${r.id}.cast`}
                      className="flex h-7 items-center gap-1 rounded-md border border-border-subtle bg-surface-elevated px-2.5 text-xs hover:bg-surface-tertiary"
                      title="下载 .cast"
                    >
                      <Download className="size-3" />
                      下载
                    </a>
                  </div>
                </article>
              );
            })}
          </div>
        )}
      </div>
    </div>
  );
}
