/**
 * BackendDownBanner — an honest, persistent strip shown when the bundled
 * backend sidecar has died (Tauri release builds only). Replaces the old
 * silent-failure behaviour where a dead backend looked like a frozen app.
 *
 * Renders nothing until `backend-sidecar-down` fires; auto-clears on
 * `backend-sidecar-up`. Shows the captured stderr tail (collapsed by default)
 * and a "Restart backend" button wired to the `restart_backend` command. While
 * the supervisor is still auto-retrying we say so; once it gives up we lean on
 * the manual button.
 */

import { useState } from "react";
import { useTranslation } from "react-i18next";
import { ChevronDown, ChevronRight, RotateCw, TriangleAlert } from "lucide-react";
import { useBackendHealth } from "@/hooks/useBackendHealth";
import {
  isTauriOverlayWindow,
  TAURI_DRAG_REGION,
  TAURI_TITLEBAR_LEFT_SAFE_PX,
} from "@/lib/tauriWindowChrome";

export function BackendDownBanner() {
  const { t } = useTranslation();
  const { down, restarting, restartError, restart } = useBackendHealth();
  const [showDetail, setShowDetail] = useState(false);
  const isTauri = isTauriOverlayWindow();

  if (!down) return null;

  const status = down.permanent
    ? t("backend.statusPermanent")
    : down.willRetry
      ? t("backend.statusRetrying", { attempt: down.attempt })
      : t("backend.statusStopped");

  return (
    <div
      role="alert"
      aria-live="assertive"
      className="flex shrink-0 items-start gap-3 border-b border-status-danger/40 bg-status-danger-soft px-4 py-2 text-status-danger"
      {...TAURI_DRAG_REGION}
      // This banner is the shell's topmost row, so under Tauri's overlay title
      // bar it sits where the macOS traffic lights live. Clear them with the same
      // left safe area + drag region <header> uses, so the ⚠ + text never tuck
      // under the window controls.
      style={isTauri ? { paddingLeft: TAURI_TITLEBAR_LEFT_SAFE_PX } : undefined}
    >
      <TriangleAlert className="mt-0.5 size-4 shrink-0" />
      <div className="min-w-0 flex-1">
        <div className="flex flex-wrap items-baseline gap-x-2">
          <span className="text-[13px] font-semibold">{t("backend.title")}</span>
          <span className="text-[12px] opacity-90">{status}</span>
        </div>

        {down.message && (
          <button
            type="button"
            onClick={() => setShowDetail((v) => !v)}
            className="mt-0.5 inline-flex items-center gap-1 text-[11px] opacity-80 transition-opacity hover:opacity-100"
            aria-expanded={showDetail}
          >
            {showDetail ? (
              <ChevronDown className="size-3" />
            ) : (
              <ChevronRight className="size-3" />
            )}
            {showDetail ? t("backend.hideDetail") : t("backend.showDetail")}
          </button>
        )}
        {showDetail && down.message && (
          <pre className="mt-1 max-h-40 overflow-auto whitespace-pre-wrap break-words rounded border border-status-danger/30 bg-surface-primary/60 p-2 font-mono text-[11px] leading-snug text-foreground-secondary">
            {down.message}
          </pre>
        )}
        {restartError && (
          <div className="mt-1 text-[11px] font-medium">
            {t("backend.restartFailed", { err: restartError })}
          </div>
        )}
      </div>

      <button
        type="button"
        onClick={restart}
        disabled={restarting}
        className="inline-flex shrink-0 items-center gap-1.5 rounded-md bg-accent-primary px-2.5 py-1.5 text-[12px] font-medium text-foreground-on-accent shadow-sm transition-colors hover:bg-accent-primary-deep disabled:cursor-not-allowed disabled:opacity-60"
      >
        <RotateCw className={`size-3.5 ${restarting ? "animate-spin" : ""}`} />
        {restarting ? t("backend.restarting") : t("backend.restart")}
      </button>
    </div>
  );
}
