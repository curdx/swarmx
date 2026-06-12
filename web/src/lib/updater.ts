import { check } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { ask } from "@tauri-apps/plugin-dialog";

/** True only inside the Tauri desktop shell — vite dev and a plain browser lack
 *  the Tauri IPC bridge, so the updater APIs would throw there. */
function inTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

/**
 * Check for a newer release; if one exists, ask the user, then download +
 * install it and relaunch. No-op outside the Tauri shell, and best-effort —
 * any failure is logged, never thrown, so it can never block app startup.
 */
export async function checkForUpdates(): Promise<void> {
  if (!inTauri()) return;
  try {
    const update = await check();
    if (!update?.available) return;
    const ok = await ask(
      `flockmux ${update.version} 可用。现在下载并更新吗？更新后会自动重启。`,
      { title: "有新版本", kind: "info", okLabel: "更新", cancelLabel: "稍后" },
    );
    if (!ok) return;
    await update.downloadAndInstall();
    await relaunch();
  } catch (e) {
    console.warn("[updater] check/install failed:", e);
  }
}
