import { describe, it, expect, vi, afterEach } from "vitest";
import { getClientPlatformInfo } from "./platform";

/** Stub just enough of `navigator` to drive getClientPlatformInfo().
 *  `window` stays undefined under the node test env, so the matchMedia
 *  (coarse-pointer) branch short-circuits to false — fine for desktop cases. */
function stubNavigator(nav: Partial<Navigator> & Record<string, unknown>) {
  vi.stubGlobal("navigator", nav);
}

afterEach(() => vi.unstubAllGlobals());

const MAC_SAFARI_UA =
  "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 " +
  "(KHTML, like Gecko) Version/17.0 Safari/605.1.15";

describe("getClientPlatformInfo — macOS detection (regression: \\bMac\\b never matched MacIntel)", () => {
  // The bug: navigator.platform is "MacIntel" on every Mac (Apple Silicon too),
  // and the old /\bMac\b/i had a trailing word boundary that "MacIntel" fails,
  // so all Macs fell through to os="other" and the send-hint read "Enter".

  it("Tauri / Safari (no userAgentData) → mac + Return", () => {
    stubNavigator({
      userAgent: MAC_SAFARI_UA,
      platform: "MacIntel",
      maxTouchPoints: 0,
    });
    const p = getClientPlatformInfo();
    expect(p.os).toBe("mac");
    expect(p.isApple).toBe(true);
    expect(p.enterKeyLabel).toBe("Return");
    expect(p.modifierKeyLabel).toBe("⌘");
  });

  it("Chrome (userAgentData.platform === 'macOS') → mac + Return", () => {
    stubNavigator({
      userAgent:
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 " +
        "(KHTML, like Gecko) Chrome/126.0 Safari/537.36",
      platform: "MacIntel",
      maxTouchPoints: 0,
      userAgentData: { platform: "macOS", mobile: false },
    });
    const p = getClientPlatformInfo();
    expect(p.os).toBe("mac");
    expect(p.enterKeyLabel).toBe("Return");
  });
});

describe("getClientPlatformInfo — other platforms keep 'Enter'", () => {
  it("Windows → windows + Enter", () => {
    stubNavigator({
      userAgent:
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 " +
        "(KHTML, like Gecko) Chrome/126.0 Safari/537.36",
      platform: "Win32",
      maxTouchPoints: 0,
      userAgentData: { platform: "Windows", mobile: false },
    });
    const p = getClientPlatformInfo();
    expect(p.os).toBe("windows");
    expect(p.isApple).toBe(false);
    expect(p.enterKeyLabel).toBe("Enter");
  });

  it("Linux → linux + Enter", () => {
    stubNavigator({
      userAgent:
        "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 " +
        "(KHTML, like Gecko) Chrome/126.0 Safari/537.36",
      platform: "Linux x86_64",
      maxTouchPoints: 0,
    });
    const p = getClientPlatformInfo();
    expect(p.os).toBe("linux");
    expect(p.enterKeyLabel).toBe("Enter");
  });

  it("iPadOS (reports MacIntel + touch) → ios, not mac", () => {
    stubNavigator({
      userAgent: MAC_SAFARI_UA, // iPad desktop-mode masquerades as Macintosh
      platform: "MacIntel",
      maxTouchPoints: 5,
    });
    const p = getClientPlatformInfo();
    expect(p.os).toBe("ios");
    expect(p.isMobileLike).toBe(true);
  });
});
