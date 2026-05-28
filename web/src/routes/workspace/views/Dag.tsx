/**
 * Dag view — collaboration graph inside WorkspaceShell.
 *
 * The original dag.tsx route owned its own header, WorkspaceScopeBar and
 * "back to chat" link. All of that lives in Shell now; this file is just
 * the canvas + left inspector + right detail panel.
 *
 * State persistence: selectedId / roleFilter live in URL search params so
 * a tab-switch + tab-return restores them. Avoids the "click a node →
 * switch to replays → come back → selection is gone" annoyance.
 */

import { useCallback, useEffect, useMemo } from "react";
import { useSearchParams } from "react-router-dom";
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
  Layers,
  Maximize2,
  X,
  Zap,
} from "lucide-react";
import { Link } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { api } from "../../../api/http";
import type {
  AgentInfo,
  BlackboardEntry,
  SwarmEvent,
} from "../../../api/types";
import { useSwarmFeed } from "../../../hooks/useSwarmFeed";
import { useWorkspaceContext } from "../Shell";
import { cn } from "@/lib/cn";
import {
  roleColorClass as roleColor,
  roleColorHex as roleHex,
} from "@/lib/agent";
import { useState } from "react";

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
  const producers = new Map<string, AgentInfo>();
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

const NODE_W = 200;
const NODE_H = 80;

function layout(nodes: Node[], edges: Edge[]) {
  const g = new dagre.graphlib.Graph();
  g.setGraph({ rankdir: "TB", nodesep: 60, ranksep: 80 });
  g.setDefaultEdgeLabel(() => ({}));
  for (const n of nodes) g.setNode(n.id, { width: NODE_W, height: NODE_H });
  for (const e of edges) g.setEdge(e.source, e.target);
  dagre.layout(g);
  return nodes.map((n) => {
    const p = g.node(n.id);
    return {
      ...n,
      position: { x: p.x - NODE_W / 2, y: p.y - NODE_H / 2 },
      sourcePosition: Position.Bottom,
      targetPosition: Position.Top,
    };
  });
}

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

interface CanvasProps {
  agents: AgentInfo[];
  bbAt: Map<string, number>;
  selectedId: string | null;
  onSelect: (id: string | null) => void;
  showMinimap: boolean;
}

function Canvas({ agents, bbAt, selectedId, onSelect, showMinimap }: CanvasProps) {
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
      maxZoom={4}
      proOptions={{ hideAttribution: true }}
      nodesDraggable
      nodesConnectable={false}
      ariaLabelConfig={{
        "node.a11yDescription.default":
          "按 Enter 或空格选中节点，方向键移动，Esc 取消。",
        "node.a11yDescription.keyboardDisabled": "节点不可移动。",
        "node.a11yDescription.ariaLiveMessage": ({ direction, x, y }) =>
          `节点已${direction === "up" ? "上移" : direction === "down" ? "下移" : direction === "left" ? "左移" : "右移"}至 ${Math.round(x)}, ${Math.round(y)}`,
        "edge.a11yDescription.default": "agent 间的 handoff 依赖连线。",
        "controls.ariaLabel": "画布缩放控制",
        "controls.zoomIn.ariaLabel": "放大",
        "controls.zoomOut.ariaLabel": "缩小",
        "controls.fitView.ariaLabel": "适应画面",
        "controls.interactive.ariaLabel": "切换交互模式",
        "minimap.ariaLabel": "缩略图",
        "handle.ariaLabel": "连接点",
      }}
    >
      <Background gap={24} size={1.25} />
      {showMinimap && (
        <MiniMap
          pannable
          zoomable
          nodeColor={(n) => roleHex((n.data as AgentNodeData).info.role)}
          nodeStrokeColor={(n) => roleHex((n.data as AgentNodeData).info.role)}
          nodeStrokeWidth={2}
          nodeBorderRadius={4}
          maskStrokeColor="var(--color-border-strong)"
          maskStrokeWidth={1}
        />
      )}
      <Controls showInteractive={false} />
    </ReactFlow>
  );
}

export default function DagView() {
  const { t } = useTranslation();
  const { workspace } = useWorkspaceContext();
  // selectedId / roleFilter 用 URL 持久化，切走再回不丢。
  const [searchParams, setSearchParams] = useSearchParams();
  const selectedId = searchParams.get("node");
  const roleFilter = searchParams.get("role") ?? "all";

  const setSelectedId = useCallback(
    (id: string | null) => {
      setSearchParams(
        (prev) => {
          const next = new URLSearchParams(prev);
          if (id) next.set("node", id);
          else next.delete("node");
          return next;
        },
        { replace: true },
      );
    },
    [setSearchParams],
  );
  const setRoleFilter = useCallback(
    (r: string) => {
      setSearchParams(
        (prev) => {
          const next = new URLSearchParams(prev);
          if (r === "all") next.delete("role");
          else next.set("role", r);
          return next;
        },
        { replace: true },
      );
    },
    [setSearchParams],
  );

  // Shell 拿到 allAliveAgents 是 cross-workspace 的，但我们要这个 workspace
  // 范围 + 历史 (含 killed) 的边数据 — 自己 listAgents 一次。同时也独立
  // 拉 blackboard 给 edge satisfied 计算用。Shell 没有这两份数据。
  const [allAgents, setAllAgents] = useState<AgentInfo[]>([]);
  const [bb, setBb] = useState<BlackboardEntry[]>([]);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const [a, b] = await Promise.all([api.listAgents(), api.listBlackboard()]);
      setAllAgents(a);
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

  const agents = useMemo(
    () =>
      allAgents.filter((a) => a.workspace_id === workspace.workspaceId),
    [allAgents, workspace.workspaceId],
  );

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
    <div className="flex min-h-0 flex-1">
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
            {t("dag.members")}
          </h3>
          {roles.length > 2 && (
            <div className="mb-2 flex flex-wrap gap-1">
              {roles.map((r) => (
                <button
                  key={r}
                  onClick={() => setRoleFilter(r)}
                  className={cn(
                    "flex items-center gap-1 rounded-full px-2 py-0.5 text-[10px]",
                    roleFilter === r
                      ? "bg-accent-primary text-foreground-on-accent"
                      : "bg-surface-tertiary text-foreground-secondary hover:bg-surface-primary",
                  )}
                >
                  {r !== "all" && (
                    <span
                      className="size-1.5 rounded-full"
                      style={{ background: roleHex(r) }}
                    />
                  )}
                  <span>{r === "all" ? t("common.all") : r}</span>
                </button>
              ))}
            </div>
          )}
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
                  <span className="truncate font-heading text-xs text-foreground-primary">
                    {a.role}
                  </span>
                  <span className="ml-auto truncate font-mono text-[10px] text-foreground-tertiary">
                    {a.agent_id.slice(-6)}
                  </span>
                </button>
              </li>
            ))}
            {filteredAgents.length === 0 && (
              <li className="px-2 py-1 font-caption text-[11px] text-foreground-tertiary">
                {t("dag.empty")}
              </li>
            )}
          </ul>
        </section>
      </aside>

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
              showMinimap={liveAgents.length > 4}
            />
          </ReactFlowProvider>
        )}
      </div>

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
                to={`/chat/${workspace.id}?agent=${encodeURIComponent(selected.agent_id)}`}
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
  );
}

void Layers;
