/**
 * Theme runtime — reads the persisted preference from the same
 * localStorage key Settings writes to, applies it to
 * `document.documentElement.dataset.theme`, and (when mode = "system")
 * tracks the OS-level prefers-color-scheme so the surface flips with
 * the system without a re-mount.
 *
 * Two consumers:
 *   - main.tsx calls applyInitialTheme() before React render to avoid a
 *     light-flash on first paint.
 *   - useTheme() is the hook Settings (and anywhere else) wires to.
 */

const STORAGE_KEY = "swarmx:settings:v1";

export type ThemeMode = "light" | "dark" | "system";

/** What actually gets written to data-theme — never "system". */
type EffectiveTheme = "light" | "dark";

function readMode(): ThemeMode {
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    if (!raw) return "light";
    const parsed = JSON.parse(raw) as { theme?: ThemeMode };
    return parsed.theme ?? "light";
  } catch {
    return "light";
  }
}

function systemPrefersDark(): boolean {
  try {
    return window.matchMedia("(prefers-color-scheme: dark)").matches;
  } catch {
    return false;
  }
}

function resolve(mode: ThemeMode): EffectiveTheme {
  if (mode === "system") return systemPrefersDark() ? "dark" : "light";
  return mode;
}

function apply(effective: EffectiveTheme) {
  document.documentElement.dataset.theme = effective;
}

/** Called from main.tsx before render to set the initial theme synchronously. */
export function applyInitialTheme() {
  apply(resolve(readMode()));
}

let systemListener: ((e: MediaQueryListEvent) => void) | null = null;
function bindSystemTracker(active: boolean) {
  const mq = window.matchMedia("(prefers-color-scheme: dark)");
  if (systemListener) {
    mq.removeEventListener("change", systemListener);
    systemListener = null;
  }
  if (active) {
    systemListener = (e) => apply(e.matches ? "dark" : "light");
    mq.addEventListener("change", systemListener);
  }
}

/**
 * Apply a mode programmatically. Used by Settings on toggle so the page
 * doesn't have to wait for a re-mount cycle to reflect the change. Also
 * (un)binds the system-pref listener depending on whether mode is "system".
 */
export function setTheme(mode: ThemeMode) {
  apply(resolve(mode));
  bindSystemTracker(mode === "system");
}

/** Read the current persisted mode. */
export function getThemeMode(): ThemeMode {
  return readMode();
}
