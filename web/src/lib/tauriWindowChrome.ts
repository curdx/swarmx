/**
 * Shared heuristics for Tauri/macOS overlay title bars.
 *
 * With `titleBarStyle: "Overlay"` the native traffic lights live inside the
 * webview's top-left corner. We reserve a conservative left safe area so app
 * chrome doesn't visually compete with or sit underneath those controls.
 */

export const TAURI_TITLEBAR_LEFT_SAFE_PX = 96;

export function isTauriOverlayWindow(): boolean {
  return (
    typeof window !== "undefined" &&
    (window.location.protocol === "tauri:" ||
      window.location.hostname === "tauri.localhost" ||
      "__TAURI_INTERNALS__" in window)
  );
}

export const TAURI_DRAG_REGION = isTauriOverlayWindow()
  ? { "data-tauri-drag-region": "" }
  : {};

export function tauriLeftSafePadding(basePx = 12): string | undefined {
  return isTauriOverlayWindow()
    ? `${basePx}px ${basePx}px ${basePx}px ${TAURI_TITLEBAR_LEFT_SAFE_PX}px`
    : undefined;
}
