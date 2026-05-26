/**
 * DAG Flow Page — Pencil frame Z23h6o.
 *
 * Three columns:
 *   DagLeft 200 (legend + role filter) | DagCanvas (ReactFlow) | DagRight 300 (selected node detail)
 *
 * Edge semantics carried over from GraphPanel (which still serves /debug):
 *   producer → dependent, labelled with the blackboard key. Solid green when
 *   the key has been written AFTER the dependent spawned; amber-dashed otherwise.
 *
 * Layout: dagre LR. ReactFlow handles zoom/pan/minimap; node coords come
 * from dagre.graphlib via a one-shot layout pass on each agent_state /
 * blackboard_changed event.
 */

import { useCallback, useEffect, useMemo, useState } from "react";
import {
  Background,
  Controls,
  Handle,
  MiniMap,
  Position,
  ReactFlow,
  ReactFlowProvider,
  useReactFlow,
  type Edge,
  type Node,
  type NodeProps,
} from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import dagre from "@dagrejs/dagre";
import {
  Activity,
  GitBranch,
  Layers,
  Maximize2,
  RefreshCw,
  X,
  Zap,
} from "lucide-react";
import { Link } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { api } from "../api/http";
import type { AgentInfo, BlackboardEntry, SwarmEvent } from "../api/types";
import { useSwarmFeed } from "../hooks/useSwarmFeed";
import { cn } from "@/lib/cn";

const ROLE_BG: Record<string, string> = {
  planner: "bg-agent-planner",
  backend: "bg-agent-backend",
  frontend: "bg-agent-frontend",
  architect: "bg-agent-architect",
  critic: "bg-agent-critic",
  test: "bg-agent-test",
};

const ROLE_HEX: Record<string, string> = {
  planner: "#D97757",
  backend: "#7B5BB8",
  frontend: "#3B6FB8",
  architect: "#C73E3E",
  critic: "#C77A1F",
  test: "#2E8B57",
};

function roleColor(role: string) {
  return ROLE_BG[role.toLowerCase()] ?? "bg-state-idle";
}
function roleHex(role: string) {
  return ROLE_HEX[role.toLowerCase()] ?? "#8E8B85";
}

// ── Edge derivation ─────────────────────────────────────────────────────

interface DerivedEdge {
  producerId: string;
  dependentId: string;
  key: string;
  satisfied: boolean;
}

function deriveEdges(
  agents: AgentInfo[],
  bbAt: Map<string, number>,
): DerivedEdge[] {
  const producers = new Map<string, AgentInfo>(); // signal → producer
  for (const a of agents) {
    if (a.handoff_signal) producers.set(a.handoff_signal, a);
  }
  const out: DerivedEdge[] = [];
  for (const dep of agents) {
    for (const key of dep.depends_on ?? []) {
      const prod = producers.get(key);
      if (!prod) continue;
      const writtenAt = bbAt.get(key);
      const satisfied =
        writtenAt != null &&
        dep.spawned_at != null &&
        writtenAt >= dep.spawned_at;
      out.push({
        producerId: prod.agent_id,
        dependentId: dep.agent_id,
        key,
        satisfied,
      });
    }
  }
  return out;
}

// ── dagre layout ────────────────────────────────────────────────────────

const NODE_W = 200;
const NODE_H = 80;

function layout(nodes: Node[], edges: Edge[]) {
  const g = new dagre.graphlib.Graph();
  g.setGraph({ rankdir: "LR", nodesep: 40, ranksep: 80 });
  g.setDefaultEdgeLabel(() => ({}));
  for (const n of nodes) g.setNode(n.id, { width: NODE_W, height: NODE_H });
  for (const e of edges) g.setEdge(e.source, e.target);
  dagre.layout(g);
  return nodes.map((n) => {
    const p = g.node(n.id);
    return {
      ...n,
      position: { x: p.x - NODE_W / 2, y: p.y - NODE_H / 2 },
      sourcePosition: Position.Right,
      targetPosition: Position.Left,
    };
  });
}

// ── Custom node ─────────────────────────────────────────────────────────

interface AgentNodeData extends Record<string, unknown> {
  info: AgentInfo;
  selected: boolean;
}

function AgentNode({ data }: NodeProps<Node<AgentNodeData>>) {
  const a = data.info;
  const role = a.role;
  const live = a.killed_at == null && a.shim_exit == null;
  return (
    <div
      className={cn(
        "flex w-[200px] flex-col gap-1 rounded-lg border-2 bg-surface-elevated px-3 py-2 shadow-sm",
        data.selected
          ? "border-accent-primary shadow-lg"
          : "border-border-subtle",
      )}
    >
      <Handle type="target" position={Position.Left} className="!bg-foreground-tertiary" />
      <div className="flex items-center gap-2">
        <span
          className={cn(
            "flex size-6 items-center justify-center rounded-full text-[10px] font-bold text-foreground-on-accent",
            roleColor(role),
          )}
        >
          {role.slice(0, 1).toUpperCase()}
        </span>
        <span className="flex-1 truncate font-heading text-sm font-semibold text-foreground-primary">
          {role}
        </span>
        <span
          className={cn(
            "size-2 rounded-full",
            live
              ? a.shim_ready
                ? "bg-state-success"
                : "bg-state-wake"
              : "bg-state-idle",
          )}
        />
      </div>
      <div className="truncate font-mono text-[10px] text-foreground-tertiary">
        {a.cli} · {a.agent_id.slice(-8)}
      </div>
      {a.handoff_signal && (
        <div className="truncate font-mono text-[10px] text-state-success">
          → {a.handoff_signal}
        </div>
      )}
      <Handle type="source" position={Position.Right} className="!bg-foreground-tertiary" />
    </div>
  );
}

const NODE_TYPES = { agent: AgentNode };

// ── Canvas wrapper (ReactFlow needs Provider for fitView) ───────────────

interface CanvasProps {
  agents: AgentInfo[];
  bbAt: Map<string, number>;
  selectedId: string | null;
  onSelect: (id: string | null) => void;
}

function Canvas({ agents, bbAt, selectedId, onSelect }: CanvasProps) {
  const live = useMemo(
    () => agents.filter((a) => a.killed_at == null && a.shim_exit == null),
    [agents],
  );

  const edges = useMemo<Edge[]>(() => {
    const derived = deriveEdges(live, bbAt);
    return derived.map((e, i) => ({
      id: `e-${i}-${e.producerId}-${e.dependentId}`,
      source: e.producerId,
      target: e.dependentId,
      label: e.key,
      animated: !e.satisfied,
      style: {
        stroke: e.satisfied ? "#2E8B57" : "#C77A1F",
        strokeWidth: 1.75,
        strokeDasharray: e.satisfied ? undefined : "6 4",
      },
      labelStyle: {
        fill: e.satisfied ? "#2E8B57" : "#C77A1F",
        fontSize: 10,
        fontFamily: "Geist Mono, ui-monospace, monospace",
      },
      labelBgStyle: { fill: "#FAFAF7" },
    }));
  }, [live, bbAt]);

  const nodes = useMemo<Node[]>(() => {
    const raw: Node[] = live.map((a) => ({
      id: a.agent_id,
      type: "agent",
      position: { x: 0, y: 0 },
      data: { info: a, selected: a.agent_id === selectedId },
    }));
    return layout(raw, edges);
  }, [live, edges, selectedId]);

  const flow = useReactFlow();
  useEffect(() => {
    // After any topology change, fit so the user always sees the graph.
    const t = window.setTimeout(() => {
      try {
        flow.fitView({ padding: 0.15, duration: 200 });
      } catch {
        /* fitView throws on empty graph */
      }
    }, 50);
    return () => window.clearTimeout(t);
  }, [flow, nodes.length, edges.length]);

  return (
    <ReactFlow
      nodes={nodes}
      edges={edges}
      nodeTypes={NODE_TYPES}
      onNodeClick={(_, n) => onSelect(n.id)}
      onPaneClick={() => onSelect(null)}
      fitView
      proOptions={{ hideAttribution: true }}
    >
      <Background gap={20} size={1} className="!opacity-50" />
      <MiniMap
        pannable
        zoomable
        nodeColor={(n) => {
          const info = (n.data as AgentNodeData).info;
          return roleHex(info.role);
        }}
        maskColor="rgba(15,23,42,0.06)"
        className="!bg-surface-tertiary"
      />
      <Controls showInteractive={false} />
    </ReactFlow>
  );
}

// ── Route ───────────────────────────────────────────────────────────────

export default function DagRoute() {
  const { t } = useTranslation();
  const [agents, setAgents] = useState<AgentInfo[]>([]);
  const [bb, setBb] = useState<BlackboardEntry[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [roleFilter, setRoleFilter] = useState<string>("all");

  const refresh = useCallback(async () => {
    try {
      const [a, b] = await Promise.all([api.listAgents(), api.listBlackboard()]);
      setAgents(a);
      setBb(b);
      setError(null);
    } catch (e) {
      setError((e as Error).message);
    }
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  useSwarmFeed({
    onEvent: (ev: SwarmEvent) => {
      if (ev.type === "agent_state" || ev.type === "blackboard_changed") {
        refresh();
      }
    },
    onReconnect: () => refresh(),
  });

  const bbAt = useMemo(() => {
    const m = new Map<string, number>();
    for (const e of bb) m.set(e.path, e.at);
    return m;
  }, [bb]);

  const liveAgents = useMemo(
    () => agents.filter((a) => a.killed_at == null && a.shim_exit == null),
    [agents],
  );

  const roles = useMemo(() => {
    const s = new Set<string>();
    for (const a of liveAgents) s.add(a.role.toLowerCase());
    return ["all", ...Array.from(s).sort()];
  }, [liveAgents]);

  const filteredAgents = useMemo(
    () =>
      roleFilter === "all"
        ? agents
        : agents.filter((a) => a.role.toLowerCase() === roleFilter),
    [agents, roleFilter],
  );

  const selected = useMemo(
    () => agents.find((a) => a.agent_id === selectedId) ?? null,
    [agents, selectedId],
  );

  return (
    <div className="flex h-full flex-col bg-surface-primary">
      {/* Header */}
      <header className="flex h-14 shrink-0 items-center gap-3 border-b border-border-subtle bg-surface-elevated px-5">
        <span className="flex size-8 items-center justify-center rounded-md bg-accent-primary-soft">
          <GitBranch className="size-4 text-accent-primary-deep" />
        </span>
        <div className="flex flex-col">
          <h1 className="font-heading text-sm font-semibold text-foreground-primary">
            {t("dag.title")}
          </h1>
          <span className="font-caption text-[10px] text-foreground-tertiary">
            {t("dag.subtitle", { count: liveAgents.length })}
          </span>
        </div>
        <span className="flex-1" />
        <button
          onClick={refresh}
          className="flex size-8 items-center justify-center rounded-md bg-surface-tertiary text-foreground-secondary hover:bg-surface-secondary"
          title={t("common.refresh")}
        >
          <RefreshCw className="size-4" />
        </button>
      </header>

      {/* Body */}
      <div className="flex min-h-0 flex-1">
        {/* Left: legend + filter */}
        <aside className="flex w-[200px] shrink-0 flex-col gap-5 border-r border-border-subtle bg-surface-secondary p-4">
          <section>
            <h3 className="mb-2 font-heading text-[11px] font-semibold uppercase tracking-wider text-foreground-tertiary">
              {t("dag.legend")}
            </h3>
            <div className="flex flex-col gap-2 font-caption text-xs">
              <div className="flex items-center gap-2">
                <svg width="36" height="10">
                  <line x1="0" y1="5" x2="36" y2="5" stroke="#2E8B57" strokeWidth="1.75" />
                </svg>
                <span className="text-foreground-secondary">{t("dag.satisfied")}</span>
              </div>
              <div className="flex items-center gap-2">
                <svg width="36" height="10">
                  <line
                    x1="0"
                    y1="5"
                    x2="36"
                    y2="5"
                    stroke="#C77A1F"
                    strokeWidth="1.75"
                    strokeDasharray="6 4"
                  />
                </svg>
                <span className="text-foreground-secondary">{t("dag.waiting")}</span>
              </div>
            </div>
          </section>
          <section>
            <h3 className="mb-2 font-heading text-[11px] font-semibold uppercase tracking-wider text-foreground-tertiary">
              {t("dag.filter")}
            </h3>
            <div className="flex flex-col gap-1">
              {roles.map((r) => (
                <button
                  key={r}
                  onClick={() => setRoleFilter(r)}
                  className={cn(
                    "flex items-center gap-2 rounded px-2 py-1 text-left text-xs",
                    roleFilter === r
                      ? "bg-accent-primary-soft text-foreground-primary"
                      : "text-foreground-secondary hover:bg-surface-tertiary",
                  )}
                >
                  {r !== "all" && (
                    <span
                      className="size-2.5 rounded-full"
                      style={{ background: roleHex(r) }}
                    />
                  )}
                  <span>{r === "all" ? t("common.all") : r}</span>
                </button>
              ))}
            </div>
          </section>
          <section>
            <h3 className="mb-2 font-heading text-[11px] font-semibold uppercase tracking-wider text-foreground-tertiary">
              {t("dag.members")}
            </h3>
            <ul className="flex flex-col gap-1">
              {filteredAgents.map((a) => (
                <li key={a.agent_id}>
                  <button
                    onClick={() => setSelectedId(a.agent_id)}
                    className={cn(
                      "flex w-full items-center gap-2 rounded px-2 py-1 text-left",
                      selectedId === a.agent_id
                        ? "bg-accent-primary-soft"
                        : "hover:bg-surface-tertiary",
                    )}
                  >
                    <span
                      className="size-2 rounded-full"
                      style={{ background: roleHex(a.role) }}
                    />
                    <span className="truncate font-mono text-[11px] text-foreground-primary">
                      {a.role}
                    </span>
                  </button>
                </li>
              ))}
            </ul>
          </section>
        </aside>

        {/* Canvas */}
        <div className="relative min-w-0 flex-1">
          {error && (
            <div className="absolute top-4 left-4 z-10 rounded-md border border-state-danger/40 bg-status-danger-soft px-3 py-2 text-xs text-state-danger">
              {error}
            </div>
          )}
          {liveAgents.length === 0 ? (
            <div className="flex h-full flex-col items-center justify-center gap-3 text-foreground-tertiary">
              <Activity className="size-10 opacity-40" />
              <p className="font-caption text-sm">{t("dag.empty")}</p>
            </div>
          ) : (
            <ReactFlowProvider>
              <Canvas
                agents={agents}
                bbAt={bbAt}
                selectedId={selectedId}
                onSelect={setSelectedId}
              />
            </ReactFlowProvider>
          )}
        </div>

        {/* Right: selected agent detail */}
        <aside className="flex w-[300px] shrink-0 flex-col gap-4 border-l border-border-subtle bg-surface-elevated p-5">
          {!selected ? (
            <div className="flex h-full flex-col items-center justify-center gap-2 text-center text-foreground-tertiary">
              <Maximize2 className="size-8 opacity-40" />
              <p className="font-caption text-sm">{t("dag.selectNode")}</p>
            </div>
          ) : (
            <>
              <div className="flex items-center gap-3">
                <span
                  className={cn(
                    "flex size-10 items-center justify-center rounded-full text-sm font-bold text-foreground-on-accent",
                    roleColor(selected.role),
                  )}
                >
                  {selected.role.slice(0, 1).toUpperCase()}
                </span>
                <div className="flex min-w-0 flex-1 flex-col">
                  <span className="font-heading text-sm font-bold text-foreground-primary">
                    {selected.role}
                  </span>
                  <span className="truncate font-mono text-[10px] text-foreground-tertiary">
                    {selected.agent_id}
                  </span>
                </div>
                <button
                  onClick={() => setSelectedId(null)}
                  className="flex size-7 items-center justify-center rounded-md text-foreground-tertiary hover:bg-surface-tertiary"
                >
                  <X className="size-4" />
                </button>
              </div>
              <dl className="grid grid-cols-[80px_1fr] gap-y-2 font-caption text-xs">
                <dt className="text-foreground-tertiary">{t("dag.cli")}</dt>
                <dd className="font-mono text-foreground-primary">{selected.cli}</dd>
                <dt className="text-foreground-tertiary">{t("dag.status")}</dt>
                <dd className="text-foreground-primary">
                  {selected.shim_ready
                    ? t("dag.ready")
                    : selected.killed_at
                      ? t("dag.killed")
                      : t("dag.startingShort")}
                </dd>
                <dt className="text-foreground-tertiary">{t("dag.handoff")}</dt>
                <dd className="break-all font-mono text-state-success">
                  {selected.handoff_signal || "—"}
                </dd>
                <dt className="text-foreground-tertiary">{t("dag.dependsOn")}</dt>
                <dd className="flex flex-wrap gap-1">
                  {(selected.depends_on ?? []).length === 0 ? (
                    <span className="text-foreground-tertiary">—</span>
                  ) : (
                    (selected.depends_on ?? []).map((k) => (
                      <span
                        key={k}
                        className="rounded bg-surface-tertiary px-1.5 py-0.5 font-mono text-foreground-primary"
                      >
                        {k}
                      </span>
                    ))
                  )}
                </dd>
              </dl>
              <div className="mt-auto flex flex-col gap-2">
                <Link
                  to={`/chat?agent=${encodeURIComponent(selected.agent_id)}`}
                  className="flex h-9 items-center justify-center gap-1.5 rounded-md bg-accent-primary text-xs font-bold text-foreground-on-accent hover:bg-accent-primary-deep"
                >
                  {t("dag.openDrawer")}
                </Link>
                <button
                  onClick={() => api.wakeAgent(selected.agent_id).catch(() => {})}
                  className="flex h-9 items-center justify-center gap-1.5 rounded-md border border-border-subtle bg-surface-elevated text-xs text-foreground-secondary hover:bg-surface-tertiary"
                >
                  <Zap className="size-3.5" />
                  {t("agent.wake")}
                </button>
              </div>
            </>
          )}
        </aside>
      </div>
    </div>
  );
}

// Avoid unused-import warning when Layers becomes useful later.
void Layers;
