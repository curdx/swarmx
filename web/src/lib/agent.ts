/**
 * Agent label / color helpers shared across MessagesPanel, RecordingsPanel,
 * replay player, context history and the DAG. Centralises three things that
 * used to be duplicated (and drifted):
 *   1. role → tailwind color class
 *   2. agent_id → role lookup (with prefix fallback when /api/agent hasn't
 *      resolved yet)
 *   3. last-8-char short id for compact display
 *
 * Pair with `<AgentChip>` for visual rendering; this module is the
 * data-layer half so non-React callers can also format an agent label.
 */

import type { AgentInfo } from "../api/types";

export const ROLE_COLOR_CLASS: Record<string, string> = {
  planner: "bg-agent-planner",
  backend: "bg-agent-backend",
  frontend: "bg-agent-frontend",
  architect: "bg-agent-architect",
  critic: "bg-agent-critic",
  test: "bg-agent-test",
  scout: "bg-agent-scout",
  fixer: "bg-agent-fixer",
};

/** Hex versions of the same role palette. Needed for non-Tailwind sites
 *  that take a raw color (ReactFlow MiniMap nodeColor, inline SVG legend,
 *  inline style fills). Must stay in sync with --color-agent-* CSS vars
 *  in global.css. */
export const ROLE_COLOR_HEX: Record<string, string> = {
  planner: "#2563EB",
  backend: "#7C3AED",
  frontend: "#0891B2",
  architect: "#DC2626",
  critic: "#EA580C",
  test: "#16A34A",
  scout: "#0D9488",
  fixer: "#CA8A04",
};

export function roleColorClass(role: string | null | undefined): string {
  if (!role) return "bg-state-idle";
  return ROLE_COLOR_CLASS[role.toLowerCase()] ?? "bg-state-idle";
}

export function roleColorHex(role: string | null | undefined): string {
  if (!role) return "#64748B";
  return ROLE_COLOR_HEX[role.toLowerCase()] ?? "#64748B";
}

export function shortAgentId(agentId: string, n = 8): string {
  return agentId.length <= n ? agentId : agentId.slice(-n);
}

export function roleInitial(role: string): string {
  return (role.charAt(0) || "?").toUpperCase();
}

/** Resolve a role label for an agent_id.
 *
 *  Lookup map first (built from /api/agent — covers exited agents too), then
 *  fall back to the cli/role-ish prefix embedded in the id (`scout-abc…` →
 *  "scout"). The fallback is intentionally lossy: it lets the first paint
 *  render *something* role-shaped before listAgents() resolves, then the
 *  real value replaces it.
 */
export function resolveRole(
  agentId: string | null | undefined,
  lookup?: Map<string, string> | null,
): string {
  if (!agentId) return "system";
  if (agentId === "user") return "user";
  if (agentId === "system") return "system";
  const hit = lookup?.get(agentId);
  if (hit) return hit;
  const seg = agentId.replace(/^_+/, "").split(/[-_]/)[0];
  return seg || "agent";
}

export function buildRoleLookup(agents: AgentInfo[]): Map<string, string> {
  const m = new Map<string, string>();
  for (const a of agents) m.set(a.agent_id, a.role);
  return m;
}
