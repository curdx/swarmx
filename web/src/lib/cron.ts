/**
 * Cron presentation helpers for the `/cron` page — pure functions, unit-tested.
 *
 * The scheduler evaluates expressions in a fixed UTC offset (see the server's
 * `cron.rs`); the UI captures the browser's offset and shows everything in that
 * local wall-clock. cronstrue turns the raw 5-field expression into a sentence
 * so a non-expert can sanity-check what they typed.
 */
import cronstrue from "cronstrue";
import "cronstrue/locales/zh_CN";

/** Minutes east of UTC for the current browser, e.g. +480 for UTC+8. */
export function localOffsetMinutes(): number {
  return -new Date().getTimezoneOffset();
}

/** "UTC+8", "UTC-5:30", "UTC". */
export function tzOffsetLabel(offsetMin: number): string {
  if (offsetMin === 0) return "UTC";
  const sign = offsetMin > 0 ? "+" : "-";
  const abs = Math.abs(offsetMin);
  const h = Math.floor(abs / 60);
  const m = abs % 60;
  return `UTC${sign}${h}${m ? `:${String(m).padStart(2, "0")}` : ""}`;
}

/**
 * Human-readable description of a cron expression, or `null` when it isn't a
 * complete 5-field expression yet — so a half-typed expr doesn't flash a
 * parse-error string while the user is still typing.
 */
export function describeCron(expr: string, lang: string): string | null {
  const e = expr.trim();
  if (e.split(/\s+/).length !== 5) return null;
  try {
    return cronstrue.toString(e, {
      locale: lang.startsWith("zh") ? "zh_CN" : "en",
      use24HourTimeFormat: true,
      throwExceptionOnParseError: true,
    });
  } catch {
    return null;
  }
}

export interface WallClock {
  mo: number;
  da: number;
  hh: number;
  mm: number;
  /** Whole-day offset from `nowMs` in the same fixed offset: 0 = today, 1 = tomorrow, -1 = yesterday. */
  dayDiff: number;
}

/** Decompose a UTC instant into the wall-clock fields of a fixed offset. */
export function wallClock(utcMs: number, offsetMin: number, nowMs: number): WallClock {
  const shift = offsetMin * 60_000;
  const d = new Date(utcMs + shift);
  // UTC getters on the shifted Date read the local wall-clock for `offsetMin`.
  const dayOf = (ms: number) => Math.floor((ms + shift) / 86_400_000);
  return {
    mo: d.getUTCMonth() + 1,
    da: d.getUTCDate(),
    hh: d.getUTCHours(),
    mm: d.getUTCMinutes(),
    dayDiff: dayOf(utcMs) - dayOf(nowMs),
  };
}

const pad2 = (n: number) => String(n).padStart(2, "0");

/** "09:00" */
export function fmtTime(w: WallClock): string {
  return `${pad2(w.hh)}:${pad2(w.mm)}`;
}

/** "06-14" */
export function fmtDate(w: WallClock): string {
  return `${pad2(w.mo)}-${pad2(w.da)}`;
}

/** Localized "in 3 hours" / "3 小时前" via Intl.RelativeTimeFormat (numeric:auto). */
export function relativeFromNow(utcMs: number, nowMs: number, lang: string): string {
  const rtf = new Intl.RelativeTimeFormat(lang.startsWith("zh") ? "zh-CN" : "en", {
    numeric: "auto",
  });
  const diffMs = utcMs - nowMs;
  const mins = Math.round(diffMs / 60_000);
  if (Math.abs(mins) < 60) return rtf.format(mins, "minute");
  const hours = Math.round(diffMs / 3_600_000);
  if (Math.abs(hours) < 24) return rtf.format(hours, "hour");
  return rtf.format(Math.round(diffMs / 86_400_000), "day");
}
