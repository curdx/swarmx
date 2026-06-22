/**
 * Shared compact-number formatting. Previously fmtTokens was copy-pasted
 * verbatim in usage.tsx, UsageTrendChart.tsx, and goals.tsx (as formatBudget);
 * three identical copies that also shared one rounding bug.
 */

/**
 * Compact token/count label: 1_500 → "1.5k", 2_500_000 → "2.50M".
 *
 * Carry-aware: the naive `n >= 1e6 ? M : n >= 1e3 ? k : n` buckets, then
 * toFixed, would render 999_999 as "1000.0k" (toFixed rounds 999.999 up to
 * 1000.0) — a self-contradictory "a thousand k". We format first, then if the
 * rounded value reached the next bucket's threshold (e.g. "1000.0k"), promote
 * it to that bucket ("1.0M"). Negatives and NaN pass through as plain strings
 * (token counts shouldn't be either, but we never want to print "NaNk").
 */
export function fmtTokens(n: number): string {
  if (!Number.isFinite(n) || n < 1_000) return String(n);
  if (n < 1_000_000) {
    const k = n / 1_000;
    // 999_999/1000 = 999.999 → toFixed(1) = "1000.0" → promote to M.
    if (k >= 999.95) return `${(n / 1_000_000).toFixed(2)}M`;
    return `${k.toFixed(1)}k`;
  }
  return `${(n / 1_000_000).toFixed(2)}M`;
}
