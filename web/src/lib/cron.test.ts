import { describe, it, expect } from "vitest";
import { describeCron, fmtDate, fmtTime, tzOffsetLabel, wallClock } from "./cron";

const JAN1_2021_UTC = Date.UTC(2021, 0, 1, 0, 0, 0); // 2021-01-01 00:00 UTC

describe("tzOffsetLabel", () => {
  it("formats whole-hour and half-hour offsets", () => {
    expect(tzOffsetLabel(0)).toBe("UTC");
    expect(tzOffsetLabel(480)).toBe("UTC+8");
    expect(tzOffsetLabel(-300)).toBe("UTC-5");
    expect(tzOffsetLabel(330)).toBe("UTC+5:30");
    expect(tzOffsetLabel(-270)).toBe("UTC-4:30");
  });
});

describe("describeCron", () => {
  it("returns null until the expression has 5 fields", () => {
    expect(describeCron("0 9", "zh")).toBeNull();
    expect(describeCron("", "en")).toBeNull();
  });
  it("returns null for a 5-field but unparseable expression", () => {
    expect(describeCron("bad x y z w", "en")).toBeNull();
  });
  it("describes a valid expression per locale", () => {
    expect(describeCron("0 9 * * 1-5", "en")).toMatch(/Monday/i);
    expect(describeCron("0 9 * * 1-5", "zh")).toContain("星期");
  });
});

describe("wallClock", () => {
  it("shifts a UTC instant into the offset's local wall-clock", () => {
    const w = wallClock(JAN1_2021_UTC, 480, JAN1_2021_UTC); // UTC+8
    expect([w.mo, w.da, w.hh, w.mm]).toEqual([1, 1, 8, 0]);
    expect(w.dayDiff).toBe(0);
  });
  it("rolls the date forward across midnight in the offset", () => {
    const utc2200 = JAN1_2021_UTC + 22 * 3_600_000; // 2021-01-01 22:00 UTC
    const w = wallClock(utc2200, 480, JAN1_2021_UTC); // +8h → 2021-01-02 06:00
    expect([w.mo, w.da, w.hh]).toEqual([1, 2, 6]);
    expect(w.dayDiff).toBe(1); // "tomorrow" relative to Jan 1 00:00 UTC = Jan 1 08:00 local
  });
  it("formats time and date with zero padding", () => {
    const w = wallClock(JAN1_2021_UTC, 0, JAN1_2021_UTC);
    expect(fmtTime(w)).toBe("00:00");
    expect(fmtDate(w)).toBe("01-01");
  });
});
