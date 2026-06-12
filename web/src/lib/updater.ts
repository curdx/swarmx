import { useSyncExternalStore } from "react";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";

export type { Update };

/** True only inside the Tauri desktop shell — vite dev and a plain browser lack
 *  the Tauri IPC bridge, so the updater APIs would throw there. */
export function inTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

/**
 * Check for a newer release WITHOUT prompting or installing. Returns the Update
 * handle (carries .version / .body release notes) when one is available, else
 * null. No-op (null) outside the Tauri shell. Never throws — a failed check
 * logs and returns null so it can never block the UI.
 */
export async function checkForUpdate(): Promise<Update | null> {
  if (!inTauri()) return null;
  try {
    const update = await check();
    return update?.available ? update : null;
  } catch (e) {
    console.warn("[updater] check failed:", e);
    return null;
  }
}

/**
 * Download + install a given update, reporting 0..100 progress, then relaunch.
 * Unlike the check, this DOES throw on failure so the caller can surface it —
 * the user explicitly asked to install, so a silent failure would be wrong.
 */
export async function installUpdate(
  update: Update,
  onProgress?: (pct: number) => void,
): Promise<void> {
  let downloaded = 0;
  let total = 0;
  await update.downloadAndInstall((event) => {
    switch (event.event) {
      case "Started":
        total = event.data.contentLength ?? 0;
        onProgress?.(0);
        break;
      case "Progress":
        downloaded += event.data.chunkLength;
        if (total > 0) {
          onProgress?.(Math.min(100, Math.round((downloaded / total) * 100)));
        }
        break;
      case "Finished":
        onProgress?.(100);
        break;
    }
  });
  await relaunch();
}

// ── background-check store (drives the "有新版本" badge on the settings entry) ──
// A tiny external store so the badge and the About panel share one source of
// truth without a context provider. Holds the Update found by the silent
// startup check; null when up-to-date / not yet checked.
let availableUpdate: Update | null = null;
const listeners = new Set<() => void>();

function setAvailableUpdate(u: Update | null): void {
  if (availableUpdate === u) return;
  availableUpdate = u;
  for (const fn of listeners) fn();
}

/**
 * Silent startup check: look for an update WITHOUT any dialog. If one exists,
 * stash it so the settings entry shows a non-intrusive badge and the About
 * panel can offer to install it. Replaces the old "ask on startup" popup.
 */
export async function checkInBackground(): Promise<void> {
  const update = await checkForUpdate();
  if (update) setAvailableUpdate(update);
}

/** Clear the stashed update (e.g. once the user has acted on it). */
export function clearAvailableUpdate(): void {
  setAvailableUpdate(null);
}

/** Reactive read of the background-checked update (null = none / unchecked). */
export function useAvailableUpdate(): Update | null {
  return useSyncExternalStore(
    (fn) => {
      listeners.add(fn);
      return () => listeners.delete(fn);
    },
    () => availableUpdate,
    () => null,
  );
}
