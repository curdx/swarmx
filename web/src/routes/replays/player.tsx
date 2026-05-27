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

import { useEffect, useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { ArrowLeft, Download, FileSearch, Share2, X } from "lucide-react";
import { api } from "../../api/http";
import type { RecordingInfo } from "../../api/types";
import { AsciicastPlayer } from "../../components/AsciicastPlayer";
import { EmptyState } from "../../components/EmptyState";

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

export default function ReplayPlayer() {
  const { t } = useTranslation();
  const { id } = useParams<{ id: string }>();
  const navigate = useNavigate();

  const [recording, setRecording] = useState<RecordingInfo | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    if (!id) return;
    let cancelled = false;
    (async () => {
      try {
        // No GET /api/recording/:id today; list + find is fine on a
        // single-user box. If the library outgrows that, add a dedicated
        // endpoint to flockmux-recorder.
        const rows = await api.listRecordings();
        if (cancelled) return;
        const r = rows.find((x) => x.id === id) ?? null;
        if (!r) setError(t("player.notFoundHint", { id }));
        setRecording(r);
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
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") navigate("/replays");
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [navigate]);

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
          primaryAction={{ label: t("player.backToLibrary"), href: "/replays" }}
          secondaryAction={{ label: t("player.goDebug"), href: "/debug" }}
        />
      </div>
    );
  }

  const live = recording.finalized_at == null;

  return (
    <div className="flex h-full flex-col bg-term-bg">
      {/* Header — Pencil kq4c9 */}
      <header
        className="flex h-14 shrink-0 items-center gap-4 border-b border-[#1F1F1F] bg-[#141414] px-6"
        style={IS_TAURI ? { paddingLeft: 88 } : undefined}
      >
        <button
          onClick={() => navigate("/replays")}
          className="flex h-9 items-center gap-1.5 rounded-md bg-[#1F1F1F] px-3 text-xs text-foreground-inverse-secondary hover:bg-[#262626] hover:text-foreground-inverse"
          title={t("player.closeEsc")}
        >
          <ArrowLeft className="size-4" />
          {t("player.back")}
        </button>
        <div className="flex min-w-0 flex-col">
          <h1 className="truncate font-mono text-sm text-foreground-inverse">
            {recording.id}
          </h1>
          <span className="truncate font-caption text-[11px] text-foreground-inverse-secondary">
            {recording.agent_id} · {recording.cols}×{recording.rows} ·{" "}
            {formatDuration(recording.duration_ms)} · {formatTime(recording.started_at)}
          </span>
        </div>
        <span className="flex-1" />
        {live && (
          <span className="rounded-full bg-status-running-soft px-3 py-1 text-[11px] text-status-running">
            {t("player.recording")}
          </span>
        )}
        <a
          href={api.recordingCastUrl(recording.id)}
          download={`${recording.id}.cast`}
          className="flex size-9 items-center justify-center rounded-md bg-[#1F1F1F] text-foreground-inverse-secondary hover:bg-[#262626] hover:text-foreground-inverse"
          title={t("player.downloadCast")}
        >
          <Download className="size-4" />
        </a>
        <a
          href={api.recordingCastUrl(recording.id)}
          target="_blank"
          rel="noreferrer"
          className="flex size-9 items-center justify-center rounded-md bg-[#1F1F1F] text-foreground-inverse-secondary hover:bg-[#262626] hover:text-foreground-inverse"
          title={t("player.rawCast")}
        >
          <Share2 className="size-4" />
        </a>
        <button
          onClick={() => navigate("/replays")}
          className="flex size-9 items-center justify-center rounded-md bg-[#1F1F1F] text-foreground-inverse-secondary hover:bg-[#262626] hover:text-foreground-inverse"
          title={t("player.closeEsc")}
        >
          <X className="size-4" />
        </button>
      </header>

      {/* Player body */}
      <div className="flex min-h-0 flex-1 items-center justify-center overflow-auto p-8">
        <div className="w-full max-w-6xl">
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
