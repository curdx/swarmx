/**
 * GraphPanel — DAG visualization of live agents and their depends_on
 * relationships. M6c step 4.
 *
 * Layout strategy (deliberately not force-directed): a topological
 * layering. Sources (agents with no depends_on) sit at the top; for
 * each agent we put it on a row strictly below every agent whose
 * handoff_signal it depends on. Within a row, agents are sorted by
 * spawned_at so spell-mate ordering is stable across re-renders.
 *
 * Edges go from `dependent → producer` (the dependent is drawn below,
 * an arrow points UP to the producer it's waiting on) labelled with
 * the blackboard key being waited on. The edge color encodes wake
 * state:
 *   - amber dashed: key not yet written, the dependent is still
 *     parked
 *   - green solid: key has been written, the dependent has been
 *     (or is about to be) woken
 *
 * Live updates: the parent forwards `blackboard_changed` and
 * `agent_state` events as prop changes; we refetch both
 * `/api/agent` and `/api/blackboard` on each. The fetches are
 * cheap enough that we don't bother diffing.
 */

import { useCallback, useEffect, useMemo, useState } from "react";
import { api } from "../api/http";
import type { AgentInfo, BlackboardEntry } from "../api/types";

interface Props {
  /** Latest blackboard write — used to recompute satisfied-edge state. */
  liveChange: { path: string; agent_id: string | null; op: string } | null;
  /** Bump this counter from the parent on any agent_state event so we
   *  refresh the node list (new spawns, exits). */
  agentTick: number;
}

export function GraphPanel({ liveChange, agentTick }: Props) {
  const [agents, setAgents] = useState<AgentInfo[]>([]);
  // Key → unix-ms timestamp of the latest write. Used to color edges as
  // satisfied ONLY when the key was written AFTER the dependent spawned —
  // otherwise stale blackboard entries from previous runs would falsely
  // light up every edge green.
  const [bbAt, setBbAt] = useState<Map<string, number>>(new Map());
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const [a, bb] = await Promise.all([api.listAgents(), api.listBlackboard()]);
      setAgents(a);
      const m = new Map<string, number>();
      for (const e of bb as BlackboardEntry[]) m.set(e.path, e.at);
      setBbAt(m);
      setError(null);
    } catch (e) {
      setError(`refresh failed: ${(e as Error).message}`);
    }
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh, agentTick, liveChange?.path]);

  const liveAgents = useMemo(
    () => agents.filter((a) => !a.killed_at && a.shim_exit === null),
    [agents],
  );

  const layout = useMemo(() => layoutAgents(liveAgents), [liveAgents]);

  if (error) {
    return (
      <div style={padded}>
        <span style={errStyle}>{error}</span>
      </div>
    );
  }

  if (liveAgents.length === 0) {
    return (
      <div style={padded}>
        <p style={muted}>
          当前没有存活的 agent。启动一个法术后，这里会显示依赖图谱。
        </p>
      </div>
    );
  }

  // Compute SVG canvas size + node positions.
  const nodeW = 130;
  const nodeH = 44;
  const colGap = 18;
  const rowGap = 60;
  const padX = 16;
  const padY = 16;

  // Figure out canvas dimensions from layout (rows of cols).
  const maxCols = layout.rows.reduce((m, r) => Math.max(m, r.length), 0);
  const width = padX * 2 + maxCols * nodeW + (maxCols - 1) * colGap;
  const height = padX * 2 + layout.rows.length * nodeH + (layout.rows.length - 1) * rowGap;

  // Build node positions keyed by agent_id.
  const positions: Map<string, { x: number; y: number; row: number; col: number }> =
    new Map();
  layout.rows.forEach((row, rIdx) => {
    const rowWidth = row.length * nodeW + (row.length - 1) * colGap;
    const startX = (width - rowWidth) / 2;
    row.forEach((aid, cIdx) => {
      positions.set(aid, {
        x: startX + cIdx * (nodeW + colGap),
        y: padY + rIdx * (nodeH + rowGap),
        row: rIdx,
        col: cIdx,
      });
    });
  });

  // Build edges. For each agent's depends_on key, find the producer
  // agent (by matching handoff_signal); if found, draw an edge from
  // dependent → producer.
  type Edge = {
    fromId: string;
    toId: string;
    key: string;
    satisfied: boolean;
  };
  const edges: Edge[] = [];
  for (const a of liveAgents) {
    for (const key of a.depends_on ?? []) {
      const producer = liveAgents.find((p) => p.handoff_signal === key);
      if (!producer) continue; // key has no producer in the live set — orphan dep, skip
      const writtenAt = bbAt.get(key);
      const dependentSpawn = a.spawned_at ?? 0;
      // Edge is "satisfied" only if the key was written AFTER the
      // dependent spawned — a leftover key from yesterday's run on the
      // same workspace must NOT light up today's edges green.
      const satisfied = writtenAt !== undefined && writtenAt > dependentSpawn;
      edges.push({
        fromId: a.agent_id,
        toId: producer.agent_id,
        key,
        satisfied,
      });
    }
  }

  // Parent → child edges. parent_agent_id is server-derived from
  // spell_runs.caller_agent_id; it's non-null only for sub-agents
  // spawned via MCP swarm_run_spell from another agent. Skip the edge
  // if the parent isn't in the live set (it exited but the child
  // outlived it — still visually a root).
  type SpawnEdge = { fromId: string; toId: string };
  const spawnEdges: SpawnEdge[] = [];
  for (const a of liveAgents) {
    if (!a.parent_agent_id) continue;
    if (!liveAgents.find((x) => x.agent_id === a.parent_agent_id)) continue;
    spawnEdges.push({ fromId: a.parent_agent_id, toId: a.agent_id });
  }

  // Per-CLI accent colors — a lookup with a graceful default, not a hard
  // per-CLI `if`/branch, so an unknown CLI degrades to neutral slate instead
  // of needing a code edit. These stay frontend-side ON PURPOSE: color is a
  // theme concern, not backend-manifest data (cf. VS Code "contributes"
  // theming — semantic tokens resolve client-side, not hardcoded hex shipped
  // from the server). GraphPanel is the legacy /debug view; the primary DAG
  // (Dag.tsx) colors by ROLE. Full theme-token migration is part of the
  // GraphPanel cleanup, not this change.
  const CLI_COLORS: Record<string, string> = {
    claude: "#7c3aed", // purple
    codex: "#0ea5e9", // sky
  };
  const CLI_COLOR_DEFAULT = "#64748b"; // slate
  const cliColor = (cli: string): string => CLI_COLORS[cli] ?? CLI_COLOR_DEFAULT;

  return (
    <div style={scrollWrap}>
      <p style={legend}>
        <span style={{ ...legendDot, background: "#7c3aed" }} /> claude
        <span style={{ ...legendDot, background: "#0ea5e9", marginLeft: 12 }} />
        codex
        <span style={legendSeparator}>•</span>
        <span style={legendEdgeSlate}>—</span> 雇佣
        <span style={legendEdgeAmber}>╌</span> 等待中
        <span style={legendEdgeGreen}>—</span> 已完成
      </p>
      <svg
        width={width}
        height={height}
        viewBox={`0 0 ${width} ${height}`}
        style={svg}
      >
        <defs>
          <marker
            id="arrow-amber"
            viewBox="0 0 10 10"
            refX="9"
            refY="5"
            markerWidth="7"
            markerHeight="7"
            orient="auto-start-reverse"
          >
            <path d="M 0 0 L 10 5 L 0 10 z" fill="#fbbf24" />
          </marker>
          <marker
            id="arrow-green"
            viewBox="0 0 10 10"
            refX="9"
            refY="5"
            markerWidth="7"
            markerHeight="7"
            orient="auto-start-reverse"
          >
            <path d="M 0 0 L 10 5 L 0 10 z" fill="#22c55e" />
          </marker>
          <marker
            id="arrow-slate"
            viewBox="0 0 10 10"
            refX="9"
            refY="5"
            markerWidth="7"
            markerHeight="7"
            orient="auto-start-reverse"
          >
            <path d="M 0 0 L 10 5 L 0 10 z" fill="#94a3b8" />
          </marker>
        </defs>

        {/* Parent → child edges. Drawn FIRST (below) so depends_on
            arrows + nodes paint on top — the spawn relationship is
            context, the dependency arrows are the live signal. */}
        {spawnEdges.map((e, i) => {
          const from = positions.get(e.fromId);
          const to = positions.get(e.toId);
          if (!from || !to) return null;
          // Parent → child: line goes from parent.bottom to child.top.
          // When parent and child happen to be on the same row (rare —
          // depends_on layout doesn't know about spawn order), nudge so
          // the line isn't horizontal. Layout doesn't model spawn lineage
          // so this is best-effort.
          const x1 = from.x + nodeW / 2;
          const y1 = from.y + nodeH;
          const x2 = to.x + nodeW / 2;
          const y2 = to.y;
          return (
            <line
              key={`spawn-${i}`}
              x1={x1}
              y1={y1}
              x2={x2}
              y2={y2}
              stroke="#94a3b8"
              strokeWidth={1.5}
              opacity={0.6}
              markerEnd="url(#arrow-slate)"
            />
          );
        })}

        {/* Edges first so they render under the nodes. */}
        {edges.map((e, i) => {
          const from = positions.get(e.fromId);
          const to = positions.get(e.toId);
          if (!from || !to) return null;
          // The edge goes from the TOP of the dependent UP TO the
          // BOTTOM of the producer. (Dependents are lower in the layout
          // than their producers.) Arrowhead points at the producer.
          const x1 = from.x + nodeW / 2;
          const y1 = from.y;
          const x2 = to.x + nodeW / 2;
          const y2 = to.y + nodeH;
          const stroke = e.satisfied ? "#22c55e" : "#fbbf24";
          const dash = e.satisfied ? undefined : "5 4";
          const marker = e.satisfied ? "url(#arrow-green)" : "url(#arrow-amber)";
          // Anchor labels 70% from dependent toward producer (i.e. close
          // to the producer end). With multiple edges fanning out from
          // one dependent to several producers, this naturally spreads
          // the labels along X — they don't collide on top of each other
          // at the midpoint. Slight perpendicular offset keeps the text
          // off the line itself.
          const t = 0.7;
          const lx = x1 + t * (x2 - x1);
          const ly = y1 + t * (y2 - y1);
          // Perpendicular nudge based on edge direction so the label
          // sits to the right of an up-going line.
          const dx = x2 - x1;
          const nudge = dx >= 0 ? 8 : -8;
          return (
            <g key={`edge-${i}`}>
              <line
                x1={x1}
                y1={y1}
                x2={x2}
                y2={y2}
                stroke={stroke}
                strokeWidth={1.5}
                strokeDasharray={dash}
                markerEnd={marker}
              />
              <text
                x={lx + nudge}
                y={ly}
                fill={e.satisfied ? "#86efac" : "#fcd34d"}
                fontSize={10}
                fontFamily="ui-monospace, monospace"
                textAnchor={dx >= 0 ? "start" : "end"}
              >
                {e.key}
              </text>
            </g>
          );
        })}

        {/* Nodes. */}
        {liveAgents.map((a) => {
          const p = positions.get(a.agent_id);
          if (!p) return null;
          const color = cliColor(a.cli);
          const idShort = a.agent_id.slice(a.agent_id.indexOf("-") + 1, a.agent_id.indexOf("-") + 9);
          const paused = !!a.paused;
          // Paused agents get a dashed border + a "⏸" glyph next to the
          // role so the operator can spot them from across the room. The
          // node fill stays the same — we're paused, not dead.
          return (
            <g key={a.agent_id}>
              <rect
                x={p.x}
                y={p.y}
                width={nodeW}
                height={nodeH}
                rx={6}
                fill="#1e293b"
                stroke={color}
                strokeWidth={1.5}
                strokeDasharray={paused ? "4 3" : undefined}
                opacity={paused ? 0.75 : 1}
              />
              <text
                x={p.x + nodeW / 2}
                y={p.y + 16}
                fill="#e2e8f0"
                fontSize={12}
                fontWeight={600}
                textAnchor="middle"
                fontFamily="inherit"
              >
                {paused ? "⏸ " : ""}
                {a.role || "(no role)"}
              </text>
              <text
                x={p.x + nodeW / 2}
                y={p.y + 32}
                fill="#94a3b8"
                fontSize={10}
                textAnchor="middle"
                fontFamily="ui-monospace, monospace"
              >
                {idShort}
              </text>
            </g>
          );
        })}
      </svg>

      {edges.length === 0 && (
        <p style={muted}>
          这些 agent 没有声明任何 <code>depends_on</code> 依赖边，
          图谱里只显示节点网格。
        </p>
      )}
    </div>
  );
}

/**
 * Topological layering. Each agent's row index is `1 + max(row of every
 * agent it depends on)` — so producers always sit above their dependents.
 * Agents with no producers in the live set start at row 0. Within a row,
 * agents are sorted by spawned_at (most-recently-spawned last) for stable
 * left-to-right ordering across re-renders.
 *
 * If `depends_on` forms a cycle through this set, layout falls back to a
 * single row to avoid infinite recursion. cycle detection at spell launch
 * (wake::detect_depends_on_cycles) already rules out cycles in real spells.
 */
function layoutAgents(agents: AgentInfo[]): { rows: string[][] } {
  if (agents.length === 0) return { rows: [] };

  // Build producer index: which agent_id produces which key?
  const producerOf: Map<string, string> = new Map();
  for (const a of agents) {
    if (a.handoff_signal) producerOf.set(a.handoff_signal, a.agent_id);
  }

  // Memoized row computation; returns -1 if a cycle is detected.
  const rowCache: Map<string, number> = new Map();
  const visiting: Set<string> = new Set();
  const rowOf = (id: string): number => {
    if (rowCache.has(id)) return rowCache.get(id)!;
    if (visiting.has(id)) return -1;
    visiting.add(id);
    const a = agents.find((x) => x.agent_id === id);
    if (!a) {
      visiting.delete(id);
      rowCache.set(id, 0);
      return 0;
    }
    let max = -1;
    for (const key of a.depends_on ?? []) {
      const prodId = producerOf.get(key);
      if (!prodId) continue;
      const r = rowOf(prodId);
      if (r < 0) {
        visiting.delete(id);
        return -1;
      }
      if (r > max) max = r;
    }
    visiting.delete(id);
    const myRow = max + 1; // sources end up at row 0
    rowCache.set(id, myRow);
    return myRow;
  };

  // First pass: any cycle? fall back to a single row.
  for (const a of agents) {
    if (rowOf(a.agent_id) < 0) {
      return { rows: [agents.map((x) => x.agent_id)] };
    }
  }

  // Group by row.
  const byRow: Map<number, AgentInfo[]> = new Map();
  for (const a of agents) {
    const r = rowOf(a.agent_id);
    if (!byRow.has(r)) byRow.set(r, []);
    byRow.get(r)!.push(a);
  }
  const maxRow = Math.max(...byRow.keys());
  const rows: string[][] = [];
  for (let r = 0; r <= maxRow; r++) {
    const list = byRow.get(r) ?? [];
    list.sort((x, y) => (x.spawned_at ?? 0) - (y.spawned_at ?? 0));
    rows.push(list.map((a) => a.agent_id));
  }
  return { rows };
}

const scrollWrap: React.CSSProperties = {
  flex: 1,
  minHeight: 0,
  overflow: "auto",
  padding: 12,
  background: "#0b1220",
};

const svg: React.CSSProperties = {
  display: "block",
  margin: "8px auto",
};

const padded: React.CSSProperties = {
  padding: 16,
};

const legend: React.CSSProperties = {
  color: "#94a3b8",
  fontSize: 11,
  margin: 0,
  paddingBottom: 8,
  borderBottom: "1px solid #1f2937",
  display: "flex",
  alignItems: "center",
  gap: 6,
};

const legendDot: React.CSSProperties = {
  display: "inline-block",
  width: 10,
  height: 10,
  borderRadius: 2,
  marginRight: 4,
};

const legendSeparator: React.CSSProperties = {
  color: "#475569",
  margin: "0 8px",
};

const legendEdgeSlate: React.CSSProperties = {
  color: "#94a3b8",
  fontFamily: "ui-monospace, monospace",
  marginRight: 4,
};

const legendEdgeAmber: React.CSSProperties = {
  color: "#fbbf24",
  fontFamily: "ui-monospace, monospace",
  marginLeft: 12,
  marginRight: 4,
};

const legendEdgeGreen: React.CSSProperties = {
  color: "#22c55e",
  fontFamily: "ui-monospace, monospace",
  marginLeft: 12,
  marginRight: 4,
};

const muted: React.CSSProperties = {
  color: "#64748b",
  fontSize: 12,
};

const errStyle: React.CSSProperties = {
  color: "#ef4444",
  fontSize: 12,
};
