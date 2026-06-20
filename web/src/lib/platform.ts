/**
 * Client platform hints for UI copy and light interaction tweaks.
 *
 * We prefer `navigator.userAgentData` when available, and only fall back to
 * `navigator.platform` for narrow cases like modifier-key labels / iPadOS
 * quirks. MDN explicitly calls out shortcut-label hints as one of the few
 * acceptable uses for platform detection.
 */

export type ClientOS =
  | "mac"
  | "windows"
  | "ios"
  | "android"
  | "linux"
  | "other";

export interface ClientPlatformInfo {
  os: ClientOS;
  isApple: boolean;
  isMobileLike: boolean;
  modifierKeyLabel: string;
  /** Chord to SEND the composer (Enter, per universal chat convention),
   *  rendered with the native key glyph on Mac: "↩" on macOS, "Enter" else. */
  sendKeyLabel: string;
  /** Chord that inserts a newline (Shift+Enter): "⇧↩" on macOS,
   *  "Shift+Enter" elsewhere. */
  newlineKeyLabel: string;
}

interface NavigatorWithUAData extends Navigator {
  userAgentData?: {
    mobile?: boolean;
    platform?: string;
  };
}

export function getClientPlatformInfo(): ClientPlatformInfo {
  if (typeof navigator === "undefined") {
    return {
      os: "other",
      isApple: false,
      isMobileLike: false,
      modifierKeyLabel: "Ctrl",
      sendKeyLabel: "Enter",
      newlineKeyLabel: "Shift+Enter",
    };
  }

  const nav = navigator as NavigatorWithUAData;
  const ua = navigator.userAgent || "";
  const uaDataPlatform = nav.userAgentData?.platform || "";
  const fallbackPlatform = navigator.platform || "";
  const platform = `${uaDataPlatform} ${fallbackPlatform}`;
  const hasTouchPoints = (navigator.maxTouchPoints || 0) > 1;
  const hasAnyTouch = (navigator.maxTouchPoints || 0) > 0;
  const hasCoarsePointer =
    typeof window !== "undefined" &&
    typeof window.matchMedia === "function" &&
    window.matchMedia("(pointer: coarse)").matches;

  // iPadOS Safari can report itself as Mac; treat "Mac + touch" as iPad/iOS.
  const isIOS =
    /\b(iPhone|iPad|iPod)\b/i.test(ua) ||
    (/Mac/i.test(platform) && hasTouchPoints);
  const isAndroid = /\bAndroid\b/i.test(ua) || /\bAndroid\b/i.test(platform);
  const isWindows =
    /\bWindows\b/i.test(platform) || /\bWindows NT\b/i.test(ua);
  // NOTE: substring /Mac/i, NOT /\bMac\b/i. macOS reports navigator.platform
  // as "MacIntel" (Apple Silicon too) and userAgentData.platform as "macOS";
  // in both the char after "Mac" is a letter, so a trailing \b word-boundary
  // never matches and EVERY Mac was misdetected as os="other" → the desktop
  // send-hint showed "Enter" instead of "Return". MDN's own example uses
  // navigator.platform.startsWith("Mac"); we use /Mac/i because `platform`
  // here is a (possibly space-prefixed) concatenation, so startsWith won't do.
  const isMac = !isIOS && /Mac/i.test(platform);
  const isLinux =
    !isAndroid && /\bLinux\b/i.test(platform + " " + ua);

  const os: ClientOS = isIOS
    ? "ios"
    : isAndroid
      ? "android"
      : isMac
        ? "mac"
        : isWindows
          ? "windows"
          : isLinux
            ? "linux"
            : "other";

  const isMobileLike =
    os === "ios" ||
    os === "android" ||
    nav.userAgentData?.mobile === true ||
    /\bMobi\b/i.test(ua) ||
    hasCoarsePointer ||
    (hasAnyTouch && !isMac && !isWindows);

  const isApple = os === "mac" || os === "ios";

  return {
    os,
    isApple,
    isMobileLike,
    modifierKeyLabel: isApple ? "⌘" : "Ctrl",
    // Native macOS glyphs (⇧ U+21E7, ↩ U+21A9) — Apple shows key symbols, never
    // spelled-out words. Enter sends / Shift+Enter newlines (universal chat
    // convention); non-Apple desktops keep the familiar spelled-out form.
    sendKeyLabel: isApple ? "↩" : "Enter",
    newlineKeyLabel: isApple ? "⇧↩" : "Shift+Enter",
  };
}

export function formatShortcutChord(
  key: string | number,
  platform = getClientPlatformInfo(),
): string {
  return platform.isApple
    ? `${platform.modifierKeyLabel}${key}`
    : `${platform.modifierKeyLabel}+${key}`;
}
