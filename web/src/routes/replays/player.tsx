/**
 * Replay Fullscreen Player — Pencil frame v1radc.
 *
 * Dark fullscreen surface: header (back + meta) on top, AsciicastPlayer
 * fills the middle. Player ships its own controls bar (WASM-rendered),
 * so the bottom strip in the Pencil mock is intentionally not reproduced
 * verbatim — duplicating controls just to match the visual would split
 * keyboard focus and lose the player's built-in shortcuts.
 *
 * Recording metadata isn't fetchable by id today (no GET /api/recording/:id);
 * we filter the list response instead. Cheap on a single-user box; if the
 * library grows we can add a dedicated endpoint to recorder later.
 */

import { useEffect, useMemo, useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { ArrowLeft, Download, FileSearch, Share2, X } from "lucide-react";
import { api } from "../../api/http";
import { downloadRecordingCast } from "@/lib/download";
import { toast } from "@/lib/toast";
import type { AgentInfo, RecordingInfo } from "../../api/types";
import { AsciicastPlayer } from "../../components/AsciicastPlayer";
import { AgentChip } from "../../components/agent/AgentChip";
import { EmptyState } from "../../components/EmptyState";
import { buildRoleLookup } from "@/lib/agent";

// Tauri 用 titleBarStyle:"Overlay"，OS 在窗口左上角约 (0,0)→(78,28) 画
// 红黄绿三个原生按钮，浮在 webview 内容之上。任何 fullscreen 路由 (escape
// AppShell 的) 都得自己给 toolbar 左边留 ~80px 空避免被红黄绿压住。
// AppShell.tsx 同名常量做了同样的事；下次再加 fullscreen 路由可以抽到
// lib/tauri.ts。
const IS_TAURI =
  typeof window !== "undefined" &&
  (window.location.protocol === "tauri:" ||
    window.location.hostname === "tauri.localhost" ||
    "__TAURI_INTERNALS__" in window);
const TAURI_DRAG_REGION = IS_TAURI ? { "data-tauri-drag-region": "" } : {};

function formatTime(ms: number): string {
  return new Date(ms).toLocaleString();
}

/** Short human label: "15:12" today / "5/27 15:12" otherwise. Used in the
 *  player header title where the OS-locale long form is too noisy. */
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

export default function ReplayPlayer() {
  const { t } = useTranslation();
  // Canonical URL is /chat/:wsId/replays/:recId — both params are required.
  const { wsId, recId } = useParams<{ wsId: string; recId: string }>();
  const id = recId;
  const navigate = useNavigate();
  // Esc / 返回按钮跳回该 workspace 的 Replays tab，保持上下文。
  const backTo = useMemo(
    () => (wsId ? `/chat/${wsId}/replays` : "/chat"),
    [wsId],
  );

  const [recording, setRecording] = useState<RecordingInfo | null>(null);
  const [roleLookup, setRoleLookup] = useState<Map<string, string>>(
    () => new Map(),
  );
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    if (!id) return;
    let cancelled = false;
    (async () => {
      try {
        // No GET /api/recording/:id today; list + find is fine on a
        // single-user box. If the library outgrows that, add a dedicated
        // endpoint to swarmx-recorder.
        const [rows, agents] = await Promise.all([
          api.listRecordings(),
          api.listAgents().catch(() => [] as AgentInfo[]),
        ]);
        if (cancelled) return;
        const r = rows.find((x) => x.id === id) ?? null;
        if (!r) setError(t("player.notFoundHint", { id }));
        setRecording(r);
        setRoleLookup(buildRoleLookup(agents));
      } catch (e) {
        if (!cancelled) setError((e as Error).message);
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [id]);

  // Esc returns to library, matching legacy RecordingsPanel modal UX.
  // P2-2：asciinema-player 自己在它的 wrapper 上绑了 keydown，命中 Escape 后
  // 退出它内部的 fullscreen 态并 `e.stopPropagation()`，于是冒泡到 window 的
  // 监听永远收不到 Esc —— 底部「Esc 返回库」就成了摆设。改用 **捕获阶段**
  // 在 window 上监听：捕获自上而下先于到达 player 元素触发，player 在冒泡阶段
  // 的 stopPropagation 拦不住它，Esc 真能返回库。
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        navigate(backTo);
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [navigate, backTo]);

  if (loading) {
    return (
      <div className="flex h-full items-center justify-center bg-term-bg text-foreground-inverse-secondary">
        {t("player.loading")}
      </div>
    );
  }

  if (error || !recording) {
    return (
      <div className="h-full bg-surface-primary">
        <EmptyState
          variant="notfound"
          icon={<FileSearch className="size-8" />}
          title={t("player.notFoundTitle")}
          hint={
            error
              ? t("player.requestFailed", { error })
              : t("player.notFoundHint", { id })
          }
          primaryAction={{ label: t("player.backToLibrary"), href: backTo }}
        />
      </div>
    );
  }

  const live = recording.finalized_at == null;

  // Raw .cast 在打包版里是跨域的（webview 是 tauri.localhost，后端是
  // 127.0.0.1:7777，且以 application/x-asciicast 无 Content-Disposition 返回）。
  // 直接 <a target=_blank> 在 webview 里会原地导航/打不开。改成 fetch 成 blob
  // 再用同源 blob: URL 在新窗口打开（同 ChatMarkdown.openExternal 的做法），
  // 失败用 toast 显式告知，绝不静默。
  const openRawCast = async () => {
    try {
      const res = await fetch(api.recordingCastUrl(recording.id));
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      const blob = await res.blob();
      const url = URL.createObjectURL(blob);
      const w = window.open(url, "_blank", "noopener,noreferrer");
      if (!w) throw new Error(t("player.popupBlocked", { defaultValue: "弹窗被拦截" }));
      window.setTimeout(() => URL.revokeObjectURL(url), 30_000);
    } catch (e) {
      toast.error(t("replays.openRawFailed", { defaultValue: "打开原始 cast 失败" }), {
        description: (e as Error)?.message,
      });
    }
  };

  return (
    <div className="flex h-full flex-col bg-term-bg">
      {/* Header — Pencil kq4c9 */}
      <header
        className="flex h-14 shrink-0 items-center gap-4 border-b border-[#1F1F1F] bg-[#141414] px-6"
        {...TAURI_DRAG_REGION}
        style={IS_TAURI ? { paddingLeft: 88 } : undefined}
      >
        <button
          onClick={() => navigate(backTo)}
          className="flex h-9 items-center gap-1.5 rounded-md bg-[#1F1F1F] px-3 text-xs text-foreground-inverse-secondary hover:bg-[#262626] hover:text-foreground-inverse"
          title={t("player.closeEsc")}
        >
          <ArrowLeft className="size-4" />
          {t("player.back")}
        </button>
        <div className="flex min-w-0 flex-col" {...TAURI_DRAG_REGION}>
          <div className="flex items-center gap-2">
            <AgentChip
              agentId={recording.agent_id}
              roleLookup={roleLookup}
              size="sm"
              tone="inverse"
            />
            <span
              className="truncate font-caption text-xs text-foreground-inverse"
              {...TAURI_DRAG_REGION}
            >
              {formatShortTime(recording.started_at)}
              {recording.duration_ms != null && (
                <> · {formatDuration(recording.duration_ms)}</>
              )}
            </span>
          </div>
          <span
            className="truncate font-mono text-[10px] text-foreground-inverse-secondary"
            {...TAURI_DRAG_REGION}
            title={`${recording.id} · ${formatTime(recording.started_at)}`}
          >
            {recording.cols}×{recording.rows} · {recording.id}
          </span>
        </div>
        <span className="flex-1 self-stretch" {...TAURI_DRAG_REGION} />
        {live && (
          <span className="rounded-full bg-status-running-soft px-3 py-1 text-[11px] text-status-running">
            {t("player.recording")}
          </span>
        )}
        <button
          type="button"
          onClick={() => downloadRecordingCast(recording.id)}
          className="flex size-9 items-center justify-center rounded-md bg-[#1F1F1F] text-foreground-inverse-secondary hover:bg-[#262626] hover:text-foreground-inverse"
          title={t("player.downloadCast")}
        >
          <Download className="size-4" />
        </button>
        <button
          type="button"
          onClick={openRawCast}
          className="flex size-9 items-center justify-center rounded-md bg-[#1F1F1F] text-foreground-inverse-secondary hover:bg-[#262626] hover:text-foreground-inverse"
          title={t("player.rawCast")}
          aria-label={t("player.rawCast")}
        >
          <Share2 className="size-4" />
        </button>
        <button
          onClick={() => navigate(backTo)}
          className="flex size-9 items-center justify-center rounded-md bg-[#1F1F1F] text-foreground-inverse-secondary hover:bg-[#262626] hover:text-foreground-inverse"
          title={t("player.closeEsc")}
        >
          <X className="size-4" />
        </button>
      </header>

      {/* Player body */}
      <div className="flex min-h-0 flex-1 flex-col items-center justify-center overflow-auto p-8">
        <div className="w-full max-w-6xl">
          {/* P2-3：live 录像还在写，回放只能播到「列出时已落盘的字节」为止，
              到尾部会无预兆截断。提前告知「录制中 / 内容不完整」，避免用户以为
              播放器坏了。 */}
          {live && (
            <div className="mb-3 flex items-center gap-2 rounded-md border border-status-running/30 bg-status-running-soft px-3 py-2 font-caption text-xs text-status-running">
              <span className="inline-block size-2 animate-pulse rounded-full bg-status-running" />
              {t("player.liveTruncatedHint", {
                defaultValue:
                  "录制中，内容不完整：回放只到当前已落盘的部分，结尾可能突然截断。完结后重新打开可看到完整录像。",
              })}
            </div>
          )}
          <AsciicastPlayer
            src={api.recordingCastUrl(recording.id)}
            cols={recording.cols}
            rows={recording.rows}
            autoPlay
          />
        </div>
      </div>

      {/* Footer hint */}
      <footer className="flex h-10 shrink-0 items-center justify-center gap-4 border-t border-[#1F1F1F] bg-[#141414] font-caption text-[11px] text-foreground-inverse-secondary">
        <span>
          <kbd className="rounded bg-[#262626] px-1.5 py-0.5 text-[10px]">␣</kbd> {t("player.shortcuts.playPause")}
        </span>
        <span>
          <kbd className="rounded bg-[#262626] px-1.5 py-0.5 text-[10px]">←/→</kbd> {t("player.shortcuts.skip")}
        </span>
        <span>
          <kbd className="rounded bg-[#262626] px-1.5 py-0.5 text-[10px]">.</kbd> {t("player.shortcuts.frame")}
        </span>
        <span>
          <kbd className="rounded bg-[#262626] px-1.5 py-0.5 text-[10px]">Esc</kbd> {t("player.shortcuts.back")}
        </span>
      </footer>
    </div>
  );
}
