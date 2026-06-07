/**
 * Usage / Cost page (`/usage`).
 *
 * flockmux has no spend data from claude/codex (PTY transport, not an API), so
 * the server scrapes per-turn token counts from each worker's session JSONL
 * into `agent_usage` and prices them at query time (GET /api/usage). This page
 * renders that: headline stat cards, a per-day token trend (pure-CSS bars, no
 * chart lib), and per-model / per-agent breakdowns. Cost is an ESTIMATE from a
 * built-in price table; unrecognised models show tokens only.
 */
import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { RefreshCw } from "lucide-react";
import {
  ResponsiveContainer,
  BarChart,
  Bar,
  XAxis,
  YAxis,
  CartesianGrid,
  Tooltip,
} from "recharts";
import { api } from "@/api/http";
import type { UsageSummary } from "@/api/types";
import { cn } from "@/lib/cn";
import { useToolWorkspaces } from "@/lib/useToolWorkspaces";
import { WorkspacePicker } from "@/components/WorkspacePicker";

function fmtTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(2)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`;
  return String(n);
}
function fmtCost(n: number): string {
  if (n === 0) return "$0";
  if (n < 0.01) return "<$0.01";
  return `$${n.toFixed(2)}`;
}
/** "152.0k / 200.0k" — peak context occupancy vs the model's cap. */
function fmtCtxPeak(peak: number, cap: number | null): string {
  if (!peak) return "—";
  return cap ? `${fmtTokens(peak)} / ${fmtTokens(cap)}` : fmtTokens(peak);
}
/** "2026-06-07" → "6/7" for a compact x-axis tick. */
function fmtDay(day: string): string {
  const p = day.split("-");
  return p.length === 3 ? `${+p[1]}/${+p[2]}` : day;
}

type DayDatum = { day: string; tokens: number; input: number; output: number };

/** Themed recharts tooltip for the daily-trend bars. */
function DayTooltip({ active, payload }: { active?: boolean; payload?: { payload: DayDatum }[] }) {
  const { t } = useTranslation();
  if (!active || !payload?.length) return null;
  const d = payload[0].payload;
  return (
    <div className="rounded-md border border-border-subtle bg-surface-elevated px-2.5 py-1.5 font-caption text-[11px] shadow-md">
      <div className="font-medium text-foreground-primary">{d.day}</div>
      <div className="mt-0.5 text-foreground-primary">{fmtTokens(d.tokens)} tokens</div>
      <div className="text-foreground-tertiary">
        {t("usage.input")} {fmtTokens(d.input)} · {t("usage.output")} {fmtTokens(d.output)}
      </div>
    </div>
  );
}

function StatCard({ label, value, hint }: { label: string; value: string; hint?: string }) {
  return (
    <div className="flex flex-col gap-1 rounded-lg border border-border-subtle bg-surface-secondary px-4 py-3">
      <span className="font-caption text-[11px] uppercase tracking-wide text-foreground-tertiary">
        {label}
      </span>
      <span className="font-mono text-xl text-foreground-primary">{value}</span>
      {hint && <span className="font-caption text-[11px] text-foreground-tertiary">{hint}</span>}
    </div>
  );
}

export default function UsageRoute() {
  const { t } = useTranslation();
  const { workspaces, wsId, setWsId } = useToolWorkspaces();
  const [data, setData] = useState<UsageSummary | null>(null);
  const [err, setErr] = useState(false);
  const [loading, setLoading] = useState(true);
  const [updatedAt, setUpdatedAt] = useState<number | null>(null);

  const load = useCallback(
    async (showSpinner = false) => {
      if (showSpinner) setLoading(true);
      try {
        const d = await api.getUsage(wsId || undefined);
        setData(d);
        setErr(false);
        setUpdatedAt(Date.now());
      } catch {
        setErr(true);
      } finally {
        if (showSpinner) setLoading(false);
      }
    },
    [wsId],
  );

  // Live-ish: first load (with spinner) on workspace change, then poll so the
  // cost/token numbers don't sit frozen while workers keep burning tokens.
  useEffect(() => {
    load(true);
    const id = window.setInterval(() => load(false), 8000);
    return () => window.clearInterval(id);
  }, [load]);

  const chartData: DayDatum[] = (data?.by_day ?? []).map((d) => ({
    day: d.day,
    tokens: d.input_tokens + d.output_tokens,
    input: d.input_tokens,
    output: d.output_tokens,
  }));

  return (
    <div className="h-full overflow-y-auto">
      <div className="mx-auto flex max-w-5xl flex-col gap-6 px-6 py-6">
        <header className="flex items-start justify-between gap-3">
          <div className="flex flex-col gap-1">
            <h1 className="font-display text-lg text-foreground-primary">{t("usage.title")}</h1>
            <p className="font-caption text-xs text-foreground-tertiary">{t("usage.subtitle")}</p>
          </div>
          <div className="flex shrink-0 items-center gap-2">
            {updatedAt && (
              <span className="font-caption text-[11px] text-foreground-tertiary">
                {t("usage.updatedAt", { time: new Date(updatedAt).toLocaleTimeString() })}
              </span>
            )}
            <button
              type="button"
              onClick={() => load(false)}
              title={t("common.refresh")}
              aria-label={t("common.refresh")}
              className="rounded border border-border-subtle p-1 text-foreground-tertiary hover:bg-surface-tertiary hover:text-foreground-secondary"
            >
              <RefreshCw className="size-3.5" />
            </button>
            <WorkspacePicker workspaces={workspaces} value={wsId} onChange={setWsId} allowAll />
          </div>
        </header>

        {loading && (
          <div className="font-caption text-sm text-foreground-tertiary">{t("common.loading")}</div>
        )}
        {err && (
          <div className="rounded-lg border border-border-subtle bg-surface-tertiary px-4 py-3 font-caption text-sm text-status-danger">
            {t("usage.loadError")}
          </div>
        )}

        {data && !loading && data.totals.events === 0 && (
          <div className="rounded-lg border border-border-subtle bg-surface-secondary px-4 py-8 text-center font-caption text-sm text-foreground-tertiary">
            {t("usage.empty")}
          </div>
        )}

        {data && !loading && data.totals.events > 0 && (
          <>
            {/* headline cards */}
            <section className="grid grid-cols-2 gap-3 md:grid-cols-4">
              <StatCard
                label={t("usage.totalCost")}
                value={fmtCost(data.totals.cost_usd)}
                hint={data.totals.priced ? t("usage.estimated") : t("usage.partialPrice")}
              />
              <StatCard
                label={t("usage.input")}
                value={fmtTokens(data.totals.input_tokens)}
                hint={`${t("usage.cacheRead")} ${fmtTokens(data.totals.cache_read_tokens)}`}
              />
              <StatCard label={t("usage.output")} value={fmtTokens(data.totals.output_tokens)} />
              <StatCard label={t("usage.events")} value={String(data.totals.events)} />
            </section>

            {/* per-day trend (recharts) */}
            {chartData.length > 0 && (
              <section className="flex flex-col gap-2">
                <h2 className="font-caption text-xs uppercase tracking-wide text-foreground-tertiary">
                  {t("usage.byDay")}
                </h2>
                <div className="h-48 rounded-lg border border-border-subtle bg-surface-secondary p-3">
                  <ResponsiveContainer width="100%" height="100%">
                    <BarChart data={chartData} margin={{ top: 4, right: 8, bottom: 0, left: 0 }}>
                      <CartesianGrid strokeDasharray="3 3" vertical={false} stroke="var(--color-border-subtle)" />
                      <XAxis
                        dataKey="day"
                        tickFormatter={fmtDay}
                        tick={{ fontSize: 10, fill: "var(--color-foreground-tertiary)" }}
                        tickLine={false}
                        axisLine={{ stroke: "var(--color-border-subtle)" }}
                        minTickGap={16}
                      />
                      <YAxis
                        tickFormatter={(v: number) => fmtTokens(v)}
                        tick={{ fontSize: 10, fill: "var(--color-foreground-tertiary)" }}
                        tickLine={false}
                        axisLine={false}
                        width={44}
                      />
                      <Tooltip
                        cursor={{ fill: "var(--color-foreground-tertiary)", opacity: 0.08 }}
                        content={<DayTooltip />}
                      />
                      <Bar
                        dataKey="tokens"
                        fill="var(--color-accent-primary)"
                        radius={[3, 3, 0, 0]}
                        maxBarSize={48}
                      />
                    </BarChart>
                  </ResponsiveContainer>
                </div>
              </section>
            )}

            {/* per-model */}
            <section className="flex flex-col gap-2">
              <h2 className="font-caption text-xs uppercase tracking-wide text-foreground-tertiary">
                {t("usage.byModel")}
              </h2>
              <UsageTable
                cols={[t("usage.model"), t("usage.input"), t("usage.output"), t("usage.cacheRead"), t("usage.ctxPeak"), t("usage.totalCost")]}
                rows={data.by_model.map((m) => [
                  m.model ?? "—",
                  fmtTokens(m.input_tokens),
                  fmtTokens(m.output_tokens),
                  fmtTokens(m.cache_read_tokens),
                  fmtCtxPeak(m.context_peak, m.context_window),
                  m.priced ? fmtCost(m.cost_usd) : t("usage.tokensOnly"),
                ])}
              />
            </section>

            {/* per-agent */}
            <section className="flex flex-col gap-2">
              <h2 className="font-caption text-xs uppercase tracking-wide text-foreground-tertiary">
                {t("usage.byAgent")}
              </h2>
              <UsageTable
                cols={[t("usage.agent"), t("usage.input"), t("usage.output"), t("usage.events")]}
                rows={data.by_agent.map((a) => [
                  a.agent_id.slice(0, 12),
                  fmtTokens(a.input_tokens),
                  fmtTokens(a.output_tokens),
                  String(a.events),
                ])}
              />
            </section>
          </>
        )}
      </div>
    </div>
  );
}

function UsageTable({ cols, rows }: { cols: string[]; rows: string[][] }) {
  return (
    <div className="overflow-hidden rounded-lg border border-border-subtle">
      <table className="w-full border-collapse font-mono text-[12px]">
        <thead>
          <tr className="bg-surface-tertiary">
            {cols.map((c, i) => (
              <th
                key={c}
                className={cn(
                  "px-3 py-2 font-caption text-[11px] uppercase tracking-wide text-foreground-tertiary",
                  i === 0 ? "text-left" : "text-right",
                )}
              >
                {c}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {rows.map((r, ri) => (
            <tr key={ri} className="border-t border-border-subtle">
              {r.map((cell, ci) => (
                <td
                  key={ci}
                  className={cn(
                    "px-3 py-1.5 text-foreground-secondary",
                    ci === 0 ? "text-left text-foreground-primary" : "text-right",
                  )}
                >
                  {cell}
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
