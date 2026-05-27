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
import { Link, useSearchParams } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { Clock, Download, Monitor, Play, RefreshCw, Search } from "lucide-react";
import { api } from "../../api/http";
import type { AgentInfo, RecordingInfo } from "../../api/types";
import { useSwarmFeed } from "../../hooks/useSwarmFeed";
import { Button } from "@/components/ui/button";
import { AgentChip } from "@/components/agent/AgentChip";
import { WorkspaceScopeBar } from "@/components/workspace/WorkspaceScopeBar";
import { resolveRole } from "@/lib/agent";
import { cn } from "@/lib/cn";

function formatTime(ms: number): string {
  return new Date(ms).toLocaleString();
}

/** Compact "today HH:mm" / "M/D HH:mm" used in card footer; the long
 *  locale string ate horizontal space without adding info users care
 *  about (year, seconds). Full timestamp stays in the hover title. */
function formatShortTime(ms: number): string {
  const d = new Date(ms);
  const now = new Date();
  const sameDay =
    d.getFullYear() === now.getFullYear() &&
    d.getMonth() === now.getMonth() &&
    d.getDate() === now.getDate();
  return sameDay
    ? d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })
    : d.toLocaleString([], {
        month: "numeric",
        day: "numeric",
        hour: "2-digit",
        minute: "2-digit",
      });
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
  const { t } = useTranslation();
  const [searchParams] = useSearchParams();
  const wsId = searchParams.get("ws");
  const [items, setItems] = useState<RecordingInfo[]>([]);
  // recording 没 workspace 字段，得 cross-ref agents — listAgents 一次，
  // 用 agent_id → workspace 反查。ws filter 用这张表过滤 recordings。
  const [agentWorkspaceById, setAgentWorkspaceById] = useState<Map<string, string>>(
    () => new Map(),
  );
  // agent_id → role 反查，给 AgentChip 用 — 录像 API 自身没 role 字段。
  const [roleLookup, setRoleLookup] = useState<Map<string, string>>(
    () => new Map(),
  );
  const [filter, setFilter] = useState("");
  const [activeTag, setActiveTag] = useState<string>("all");
  const [error, setError] = useState<string | null>(null);

  const refresh = async () => {
    try {
      const [rows, agents] = await Promise.all([
        api.listRecordings(),
        api.listAgents(),
      ]);
      setItems(rows);
      const wsM = new Map<string, string>();
      const roleM = new Map<string, string>();
      for (const a of agents as AgentInfo[]) {
        if (a.workspace) wsM.set(a.agent_id, a.workspace);
        roleM.set(a.agent_id, a.role);
      }
      setAgentWorkspaceById(wsM);
      setRoleLookup(roleM);
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
    for (const r of items) set.add(resolveRole(r.agent_id, roleLookup));
    return ["all", ...Array.from(set).sort()];
  }, [items, roleLookup]);

  // 单独算一遍 wsId-only 过滤后的数量，给 header 副标题用 (这样 header
  // 不受 tag/query 影响，永远显示"当前工作空间下 N 个录像")。
  const scopedToWorkspace = useMemo(() => {
    if (!wsId) return items;
    return items.filter((r) => {
      const ws = agentWorkspaceById.get(r.agent_id);
      return ws && ws.slice(-8) === wsId;
    });
  }, [items, wsId, agentWorkspaceById]);

  const filtered = useMemo(() => {
    const q = filter.trim().toLowerCase();
    return scopedToWorkspace.filter((r) => {
      if (activeTag !== "all" && resolveRole(r.agent_id, roleLookup) !== activeTag)
        return false;
      if (q && !r.agent_id.toLowerCase().includes(q) && !r.id.toLowerCase().includes(q)) {
        return false;
      }
      return true;
    });
  }, [scopedToWorkspace, filter, activeTag, roleLookup]);

  return (
    <div className="flex h-full flex-col bg-surface-primary">
      <WorkspaceScopeBar wsId={wsId} />
      {/* Header */}
      <header className="flex h-14 shrink-0 items-center gap-3 border-b border-border-subtle bg-surface-elevated px-5">
        <span className="flex size-8 items-center justify-center rounded-md bg-accent-primary-soft">
          <Play className="size-4 text-accent-primary-deep" />
        </span>
        <div className="flex flex-col">
          <h1 className="font-heading text-sm font-semibold text-foreground-primary">
            {t("replays.title")}
          </h1>
          <span className="font-caption text-[10px] text-foreground-tertiary">
            {wsId
              ? t("replays.countScoped", {
                  shown: scopedToWorkspace.length,
                  total: items.length,
                })
              : t("replays.count", { count: items.length })}
          </span>
        </div>
        <span className="flex-1" />
        <div className="flex h-8 w-60 items-center gap-2 rounded-md bg-surface-tertiary px-3">
          <Search className="size-3.5 text-foreground-tertiary" />
          <input
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            placeholder={t("replays.search")}
            className="min-w-0 flex-1 bg-transparent text-xs text-foreground-primary placeholder:text-foreground-tertiary focus:outline-none"
          />
        </div>
        <Button
          variant="ghost"
          size="icon"
          onClick={refresh}
          title={t("common.refresh")}
          className="size-8"
        >
          <RefreshCw className="size-4" />
        </Button>
      </header>

      {/* Tag bar */}
      <div className="flex h-11 shrink-0 items-center gap-1.5 border-b border-border-subtle bg-surface-secondary px-5">
        {tags.map((tag) => {
          const active = tag === activeTag;
          return (
            <Button
              key={tag}
              variant={active ? "default" : "outline"}
              size="sm"
              onClick={() => setActiveTag(tag)}
              className="h-7 rounded-full"
            >
              {tag === "all" ? t("replays.tagAll") : tag}
            </Button>
          );
        })}
        <span className="flex-1" />
        <span className="font-caption text-xs text-foreground-tertiary">
          {t("replays.ratio", {
            shown: filtered.length,
            total: scopedToWorkspace.length,
          })}
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
            <p className="font-caption text-sm">{t("replays.empty")}</p>
          </div>
        ) : (
          <div className="grid grid-cols-1 gap-4 md:grid-cols-2 xl:grid-cols-3">
            {filtered.map((r) => {
              const live = r.finalized_at == null;
              return (
                <article
                  key={r.id}
                  className="group overflow-hidden rounded-lg border border-border-subtle bg-surface-elevated transition-shadow hover:shadow-lg"
                >
                  {/* Card head: agent chip + live dot */}
                  <div className="flex items-center gap-2 px-3 py-2">
                    <AgentChip
                      agentId={r.agent_id}
                      roleLookup={roleLookup}
                      size="sm"
                      className="min-w-0 flex-1"
                    />
                    <span
                      className={cn(
                        "rounded-full px-2 py-0.5 font-caption text-[10px]",
                        live
                          ? "bg-status-running-soft text-status-running"
                          : "bg-surface-tertiary text-foreground-tertiary",
                      )}
                      title={live ? t("replays.live") : t("replays.completed")}
                    >
                      {live ? t("replays.live") : t("replays.completed")}
                    </span>
                  </div>

                  {/* Cast thumbnail */}
                  <Link
                    to={`/replays/${encodeURIComponent(r.id)}${wsId ? `?ws=${wsId}` : ""}`}
                    className="relative block h-32 overflow-hidden border-y border-[#1F1F1F] bg-term-bg"
                  >
                    <CastThumb recording={r} live={live} t={t} />
                    {/* hover overlay */}
                    <div className="absolute inset-0 flex items-center justify-center bg-black/40 opacity-0 transition-opacity group-hover:opacity-100">
                      <span className="flex items-center gap-1.5 rounded-full bg-accent-primary px-4 py-2 text-xs font-medium text-foreground-on-accent">
                        <Play className="size-3.5" />
                        {t("replays.play")}
                      </span>
                    </div>
                  </Link>

                  {/* Card foot */}
                  <div className="flex items-center gap-2 px-3 py-2 text-foreground-secondary">
                    <span
                      className="font-caption text-[11px]"
                      title={`${formatTime(r.started_at)} · ${r.id}`}
                    >
                      {formatShortTime(r.started_at)}
                      {r.duration_ms != null && (
                        <span className="ml-1.5 text-foreground-tertiary">
                          · {formatDuration(r.duration_ms)}
                        </span>
                      )}
                    </span>
                    <span className="flex-1" />
                    <Link
                      to={`/replays/${encodeURIComponent(r.id)}${wsId ? `?ws=${wsId}` : ""}`}
                      className="flex h-7 items-center gap-1 rounded-md bg-accent-primary px-2.5 text-xs text-foreground-on-accent hover:bg-accent-primary-deep"
                    >
                      <Play className="size-3" />
                      {t("replays.play")}
                    </Link>
                    <a
                      href={api.recordingCastUrl(r.id)}
                      download={`${r.id}.cast`}
                      className="flex h-7 items-center gap-1 rounded-md border border-border-subtle bg-surface-elevated px-2.5 text-xs hover:bg-surface-tertiary"
                      title={t("player.downloadCast")}
                    >
                      <Download className="size-3" />
                      {t("replays.download")}
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

/**
 * Renders the dark plate inside each replay card. Used to try to decode
 * the first ~16KB of the .cast and show actual lines — the result was
 * ANSI-mangled and often unreadable (escapes leaked, whitespace got
 * eaten by the stripper, lines truncated mid-glyph). Replaced with a
 * structural badge plate: big play icon + the few stable facts a user
 * actually scans cards by (geometry / duration / live or ready).
 *
 * If we ever ship a real first-frame PNG (server-side asciinema render),
 * swap this body — keep the same call shape.
 */
function CastThumb({
  recording: r,
  live,
  t,
}: {
  recording: RecordingInfo;
  live: boolean;
  t: (k: string, opts?: Record<string, unknown>) => string;
}) {
  return (
    <div className="flex h-full flex-col items-center justify-center gap-2 px-4 py-3 text-term-fg">
      <div
        className={cn(
          "flex size-12 items-center justify-center rounded-full",
          live
            ? "bg-status-running-soft text-status-running"
            : "bg-[#1F1F1F] text-term-dim",
        )}
      >
        <Play className="size-6" />
      </div>
      <div className="flex items-center gap-2 font-mono text-[10px] text-term-dim">
        <Monitor className="size-3" />
        <span>{r.cols}×{r.rows}</span>
        {r.duration_ms != null && (
          <>
            <Clock className="size-3" />
            <span>{formatDuration(r.duration_ms)}</span>
          </>
        )}
      </div>
      <div className="font-caption text-[10px] uppercase tracking-wider text-term-dim">
        {live ? t("replays.recording") : t("replays.ready")}
      </div>
    </div>
  );
}
