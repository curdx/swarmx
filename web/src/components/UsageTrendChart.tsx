import { useTranslation } from "react-i18next";
import { fmtTokens } from "@/lib/format";
import {
  ResponsiveContainer,
  BarChart,
  Bar,
  XAxis,
  YAxis,
  CartesianGrid,
  Tooltip,
} from "recharts";

export type DayDatum = { day: string; tokens: number; input: number; output: number };

const INITIAL_CHART_SIZE = { width: 640, height: 168 };

function fmtDay(day: string): string {
  const p = day.split("-");
  return p.length === 3 ? `${+p[1]}/${+p[2]}` : day;
}

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

export function UsageTrendChart({ data }: { data: DayDatum[] }) {
  return (
    <ResponsiveContainer
      width="100%"
      height="100%"
      minWidth={0}
      minHeight={1}
      initialDimension={INITIAL_CHART_SIZE}
    >
      <BarChart data={data} margin={{ top: 4, right: 8, bottom: 0, left: 0 }}>
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
  );
}
