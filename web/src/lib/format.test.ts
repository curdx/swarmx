import { describe, it, expect } from "vitest";
import { fmtTokens } from "./format";

describe("fmtTokens", () => {
  it("passes through sub-1k counts verbatim", () => {
    expect(fmtTokens(0)).toBe("0");
    expect(fmtTokens(999)).toBe("999");
  });

  it("uses k for thousands", () => {
    expect(fmtTokens(1_000)).toBe("1.0k");
    expect(fmtTokens(1_500)).toBe("1.5k");
    expect(fmtTokens(12_340)).toBe("12.3k");
  });

  it("uses M for millions", () => {
    expect(fmtTokens(1_000_000)).toBe("1.00M");
    expect(fmtTokens(2_500_000)).toBe("2.50M");
  });

  it("promotes the 999.95k–1M carry edge to M instead of '1000.0k'", () => {
    // The whole reason this is shared: the old buckets rendered these as
    // "1000.0k" (a thousand k == a megatoken, nonsense). They must read "1.00M".
    expect(fmtTokens(999_999)).toBe("1.00M");
    expect(fmtTokens(999_950)).toBe("1.00M");
    // Just below the carry stays in k.
    expect(fmtTokens(999_949)).toBe("999.9k");
  });

  it("never prints NaNk / Infinityk", () => {
    expect(fmtTokens(NaN)).toBe("NaN");
    expect(fmtTokens(Infinity)).toBe("Infinity");
  });

  it("passes negatives through without abbreviating", () => {
    expect(fmtTokens(-5)).toBe("-5");
    expect(fmtTokens(-1_500)).toBe("-1500");
  });
});
