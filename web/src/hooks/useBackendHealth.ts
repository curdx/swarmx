/**
 * useBackendHealth — surface the bundled backend sidecar's liveness to the UI.
 *
 * In a release Tauri build the desktop shell owns the swarmx-server sidecar
 * and supervises it (see web/src-tauri/src/lib.rs). When that process dies it
 * emits `backend-sidecar-down` (with a stderr tail + whether an auto-restart is
 * still pending) and, once a (re)spawn succeeds, `backend-sidecar-up`.
 *
 * Before this hook the frontend listened to NEITHER event: a crashed backend
 * just showed up as a wall of failed fetches / stuck spinners with no
 * explanation — exactly the "interface lies about state" footgun this project
 * forbids. This hook turns those events into honest UI state and exposes a
 * manual restart (the `restart_backend` Tauri command).
 *
 * Browser dev builds (vite, no Tauri) are a clean no-op: the listeners never
 * attach, so `down` stays null and nothing renders.
 */

import { useCallback, useEffect, useState } from "react";
import { isTauriOverlayWindow } from "@/lib/tauriWindowChrome";

/** Mirrors the Rust `SidecarDown` payload (serde camelCase). */
export interface BackendDownInfo {
  /** Human-facing reason + the captured stderr tail. */
  message: string;
  /** True once the supervisor exhausted its auto-restart budget. */
  permanent: boolean;
  /** True while an automatic back-off respawn is still scheduled. */
  willRetry: boolean;
  /** Which consecutive attempt just failed. */
  attempt: number;
}

export interface BackendHealth {
  down: BackendDownInfo | null;
  restarting: boolean;
  restartError: string | null;
  restart: () => void;
}

export function useBackendHealth(): BackendHealth {
  const [down, setDown] = useState<BackendDownInfo | null>(null);
  const [restarting, setRestarting] = useState(false);
  const [restartError, setRestartError] = useState<string | null>(null);

  useEffect(() => {
    if (!isTauriOverlayWindow()) return; // browser dev build — no sidecar to watch
    let cancelled = false;
    let unlisteners: Array<() => void> = [];

    (async () => {
      try {
        const { listen } = await import("@tauri-apps/api/event");
        const offDown = await listen<BackendDownInfo>(
          "backend-sidecar-down",
          (e) => {
            setDown(e.payload);
            // A fresh failure event means the previous restart attempt (if any)
            // is over — re-enable the button.
            setRestarting(false);
          },
        );
        const offUp = await listen("backend-sidecar-up", () => {
          // Backend (re)started — clear the banner and any error.
          setDown(null);
          setRestarting(false);
          setRestartError(null);
        });
        if (cancelled) {
          offDown();
          offUp();
          return;
        }
        unlisteners = [offDown, offUp];
      } catch {
        /* not Tauri / event API unavailable — no-op */
      }
    })();

    return () => {
      cancelled = true;
      for (const off of unlisteners) off();
    };
  }, []);

  const restart = useCallback(() => {
    setRestarting(true);
    setRestartError(null);
    void (async () => {
      try {
        const { invoke } = await import("@tauri-apps/api/core");
        await invoke("restart_backend");
        // On success the backend will emit `backend-sidecar-up` (clears the
        // banner) or a new `backend-sidecar-down` (re-enables the button); keep
        // `restarting` true until one of those lands so the button shows a
        // spinner meanwhile.
      } catch (err) {
        setRestarting(false);
        setRestartError(String(err));
      }
    })();
  }, []);

  return { down, restarting, restartError, restart };
}
