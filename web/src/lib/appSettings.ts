/**
 * appSettings — runtime read of the user's behavioural preferences.
 *
 * These three toggles live in the SAME `flockmux:settings:v1` localStorage blob
 * the Settings page (routes/settings.tsx) owns and writes; theme.ts / i18n read
 * their own slices of it the same way. We only READ here — Settings is the sole
 * writer — and we read fresh on every call (no subscription, no cache), so a
 * toggle flipped in another tab / since the last read takes effect immediately
 * the next time a behaviour consults it.
 *
 * Defaults mirror routes/settings.tsx DEFAULTS exactly; a missing key / corrupt
 * JSON / no-localStorage environment all fall back to them.
 */

const STORAGE_KEY = "flockmux:settings:v1";

export interface AppSettings {
  /** Show the main window on launch. false ⇒ start hidden to the tray (Tauri). */
  openMainOnLaunch: boolean;
  /** Fire a desktop notification when an agent replies to the user (Tauri). */
  desktopNotify: boolean;
  /** When an agent fails, kill the other live agents in its workspace/run. */
  killOthersOnFail: boolean;
}

const DEFAULTS: AppSettings = {
  openMainOnLaunch: true,
  desktopNotify: true,
  killOthersOnFail: false,
};

/** Read the current preference values from localStorage, defaulting any
 *  missing / unparsable field. Call this each time a behaviour needs the latest
 *  value — it's cheap and always current. */
export function loadAppSettings(): AppSettings {
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    if (!raw) return { ...DEFAULTS };
    const parsed = JSON.parse(raw) as Partial<AppSettings> | null;
    if (!parsed || typeof parsed !== "object") return { ...DEFAULTS };
    return {
      openMainOnLaunch:
        typeof parsed.openMainOnLaunch === "boolean"
          ? parsed.openMainOnLaunch
          : DEFAULTS.openMainOnLaunch,
      desktopNotify:
        typeof parsed.desktopNotify === "boolean"
          ? parsed.desktopNotify
          : DEFAULTS.desktopNotify,
      killOthersOnFail:
        typeof parsed.killOthersOnFail === "boolean"
          ? parsed.killOthersOnFail
          : DEFAULTS.killOthersOnFail,
    };
  } catch {
    return { ...DEFAULTS };
  }
}
