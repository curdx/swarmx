/**
 * Replays view — recording library inside WorkspaceShell.
 *
 * Strips the previous /replays route's own header / WorkspaceScopeBar
 * (Shell owns them now). Keeps the tag bar + card grid + search box;
 * tag and search live in URL state so navigating away and back preserves
 * filters.
 *
 * Cards link to /chat/:wsId/replays/:recId for the fullscreen player —
 * that route is OUTSIDE the Shell (mounted directly under App routes)
 * because the player wants a fully dark, chrome-less surface.
 */

import { useEffect, useMemo, useState } from "react";
import { Link, useSearchParams } from "react-router-dom";
import { useTranslation } from "react-i18next";
import {
  Clock,
  Download,
  Monitor,
  Play,
  RefreshCw,
  Search,
} from "lucide-react";
import { api } from "../../../api/http";
import type { AgentInfo, RecordingInfo } from "../../../api/types";
import { useSwarmFeed } from "../../../hooks/useSwarmFeed";
import { Button } from "@/components/ui/button";
import { AgentChip } from "@/components/agent/AgentChip";
import { resolveRole } from "@/lib/agent";
import { cn } from "@/lib/cn";
import { useWorkspaceContext } from "../Shell";

function formatTime(ms: number): string {
  return new Date(ms).toLocaleString();
}

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

export default function ReplaysView() {
  const { t } = useTranslation();
  const { workspace } = useWorkspaceContext();
  const [searchParams, setSearchParams] = useSearchParams();
  const activeTag = searchParams.get("tag") ?? "all";
  const filter = searchParams.get("q") ?? "";

  const setActiveTag = (tag: string) => {
    setSearchParams(
      (prev) => {
        const next = new URLSearchParams(prev);
        if (tag === "all") next.delete("tag");
        else next.set("tag", tag);
        return next;
      },
      { replace: true },
    );
  };
  const setFilter = (q: string) => {
    setSearchParams(
      (prev) => {
        const next = new URLSearchParams(prev);
        if (!q) next.delete("q");
        else next.set("q", q);
        return next;
      },
      { replace: true },
    );
  };

  const [items, setItems] = useState<RecordingInfo[]>([]);
  // 录像 cross-ref：agent_id → role (给 AgentChip) + agent_id → workspace_id
  // (给 ws 过滤)。Shell 已有 allAliveAgents 但不含 killed agents 的历史
  // workspace_id，录像可能属于已退出的 agent，所以这里独立 listAgents 一次。
  const [agentWsIdById, setAgentWsIdById] = useState<Map<string, string>>(
    () => new Map(),
  );
  const [roleLookup, setRoleLookup] = useState<Map<string, string>>(
    () => new Map(),
  );
  const [error, setError] = useState<string | null>(null);

  const refresh = async () => {
    try {
      const [rows, agents] = await Promise.all([
        api.listRecordings(),
        api.listAgents(),
      ]);
      setItems(rows);
      const wsIdM = new Map<string, string>();
      const roleM = new Map<string, string>();
      for (const a of agents as AgentInfo[]) {
        if (a.workspace_id) wsIdM.set(a.agent_id, a.workspace_id);
        roleM.set(a.agent_id, a.role);
      }
      setAgentWsIdById(wsIdM);
      setRoleLookup(roleM);
      setError(null);
    } catch (e) {
      setError((e as Error).message);
    }
  };

  useEffect(() => {
    refresh();
  }, []);

  useSwarmFeed({
    onEvent: (ev) => {
      if (ev.type === "agent_state" && ev.state === "exited") refresh();
    },
    onReconnect: () => refresh(),
  });

  const scopedToWorkspace = useMemo(() => {
    return items.filter((r) => {
      const wsId = agentWsIdById.get(r.agent_id);
      return wsId === workspace.workspaceId;
    });
  }, [items, workspace.workspaceId, agentWsIdById]);

  const tags = useMemo(() => {
    const set = new Set<string>();
    for (const r of scopedToWorkspace) set.add(resolveRole(r.agent_id, roleLookup));
    return ["all", ...Array.from(set).sort()];
  }, [scopedToWorkspace, roleLookup]);

  const filtered = useMemo(() => {
    const q = filter.trim().toLowerCase();
    return scopedToWorkspace.filter((r) => {
      if (activeTag !== "all" && resolveRole(r.agent_id, roleLookup) !== activeTag)
        return false;
      if (
        q &&
        !r.agent_id.toLowerCase().includes(q) &&
        !r.id.toLowerCase().includes(q)
      ) {
        return false;
      }
      return true;
    });
  }, [scopedToWorkspace, filter, activeTag, roleLookup]);

  return (
    <div className="flex min-h-0 flex-1 flex-col bg-surface-primary">
      {/* sub-header: search + refresh + tag bar — Shell-level channel
          header already shows the workspace name / path / unread; this
          row is view-internal toolbar only. */}
      <div className="flex h-11 shrink-0 items-center gap-2 border-b border-border-subtle bg-surface-secondary px-5">
        <div className="flex h-8 w-60 items-center gap-2 rounded-md bg-surface-primary px-3">
          <Search className="size-3.5 text-foreground-tertiary" />
          <input
            name="replay-search"
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
          className="size-7"
        >
          <RefreshCw className="size-3.5" />
        </Button>
        <span className="flex-1" />
        <div className="flex items-center gap-1">
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
        </div>
        <span className="ml-2 font-caption text-xs text-foreground-tertiary">
          {t("replays.ratio", {
            shown: filtered.length,
            total: scopedToWorkspace.length,
          })}
        </span>
      </div>

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

                  <Link
                    to={`/chat/${workspace.id}/replays/${encodeURIComponent(r.id)}`}
                    className="relative block h-32 overflow-hidden border-y border-[#1F1F1F] bg-term-bg"
                  >
                    <CastThumb recording={r} live={live} t={t} />
                    <div className="absolute inset-0 flex items-center justify-center bg-black/40 opacity-0 transition-opacity group-hover:opacity-100">
                      <span className="flex items-center gap-1.5 rounded-full bg-accent-primary px-4 py-2 text-xs font-medium text-foreground-on-accent">
                        <Play className="size-3.5" />
                        {t("replays.play")}
                      </span>
                    </div>
                  </Link>

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
                      to={`/chat/${workspace.id}/replays/${encodeURIComponent(r.id)}`}
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
