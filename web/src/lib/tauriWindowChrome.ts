/**
 * Shared heuristics for Tauri/macOS overlay title bars.
 *
 * With `titleBarStyle: "Overlay"` the native traffic lights live inside the
 * webview's top-left corner. We reserve a conservative left safe area so app
 * chrome doesn't visually compete with or sit underneath those controls.
 */

export const TAURI_TITLEBAR_LEFT_SAFE_PX = 96;

/**
 * True in any Tauri webview, on every platform. Use this to gate desktop-only
 * behaviour that applies equally on macOS/Windows/Linux — sidecar health
 * watching, window controls, native notifications, launch-window prefs.
 */
export function isTauriWindow(): boolean {
  return (
    typeof window !== "undefined" &&
    (window.location.protocol === "tauri:" ||
      window.location.hostname === "tauri.localhost" ||
      "__TAURI_INTERNALS__" in window)
  );
}

/** True only in a macOS webview — where the overlay title bar actually lives. */
function isMacWebview(): boolean {
  return typeof navigator !== "undefined" && /Mac/i.test(navigator.userAgent);
}

/**
 * True only when the native traffic lights sit *inside* the webview: a Tauri
 * window on macOS, where `titleBarStyle: "Overlay"` applies. Windows/Linux keep
 * a normal native title bar (the webview starts at a clean top-left), so this is
 * false there — reserving a left safe area or a webview drag region on those
 * platforms would shove all top chrome right by nothing. Use this ONLY for
 * traffic-light avoidance (left padding, drag region); use `isTauriWindow` for
 * the general "are we the desktop app" check.
 */
export function isTauriOverlayWindow(): boolean {
  return isTauriWindow() && isMacWebview();
}

export const TAURI_DRAG_REGION = isTauriOverlayWindow()
  ? { "data-tauri-drag-region": "" }
  : {};

export function tauriLeftSafePadding(basePx = 12): string | undefined {
  return isTauriOverlayWindow()
    ? `${basePx}px ${basePx}px ${basePx}px ${TAURI_TITLEBAR_LEFT_SAFE_PX}px`
    : undefined;
}
