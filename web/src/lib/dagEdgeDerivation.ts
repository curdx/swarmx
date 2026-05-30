/**
 * Canonical DAG edge derivation — the SINGLE source of truth for turning a
 * set of agents (+ blackboard write times) into the collaboration graph's
 * edges. Previously this logic was implemented twice (Dag.tsx's ReactFlow view
 * and the legacy GraphPanel.tsx SVG view) and had drifted: the two disagreed on
 * the `satisfied` comparison (`>=` vs `>`), on missing-`spawned_at` handling
 * (`!= null` vs `?? 0`), on producer lookup (Map vs linear find), and on the
 * live-agent filter. GraphPanel was deleted; this module is what remains, so
 * there is exactly one definition and it can't drift again.
 *
 * Pure + dependency-free so it's trivially reusable and reasoned-about.
 */

import type { AgentInfo } from "../api/types";

export interface DerivedHandoffEdge {
  /** Agent whose `handoff_signal` produces `key`. Edge source. */
  producerId: string;
  /** Agent that lists `key` in its `depends_on`. Edge target. */
  dependentId: string;
  /** The blackboard key linking them. */
  key: string;
  /** True once the key was written at/after the dependent spawned — i.e. the
   *  dependent's wait is satisfied. Renders solid/green vs dashed/amber. */
  satisfied: boolean;
}

export interface DerivedSpawnEdge {
  parentId: string;
  childId: string;
}

/** Filter to the agents the graph draws: still live (not killed, PTY not
 *  exited). Canonical strict null checks — a `0` timestamp is a real value,
 *  not "absent", so never use truthiness here. */
export function liveAgents(agents: AgentInfo[]): AgentInfo[] {
  return agents.filter((a) => a.killed_at == null && a.shim_exit == null);
}

/**
 * Handoff edges: dependent → producer, keyed on blackboard signals.
 *
 * @param agents already-filtered live agents (call {@link liveAgents} first).
 * @param bbAt   blackboard key → latest write timestamp (unix-ms).
 *
 * Canonical decisions (the ones Dag.tsx already used, now the only ones):
 * - producer lookup via a Map (O(1)), not a linear `find`.
 * - `satisfied` := the key was written AND the dependent has a spawn time AND
 *   `writtenAt >= spawned_at` (accepts the same-instant case).
 */
export function deriveHandoffEdges(
  agents: AgentInfo[],
  bbAt: Map<string, number>,
): DerivedHandoffEdge[] {
  const producers = new Map<string, AgentInfo>();
  for (const a of agents) {
    if (a.handoff_signal) producers.set(a.handoff_signal, a);
  }
  const out: DerivedHandoffEdge[] = [];
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

/**
 * Parent → child edges from `parent_agent_id` (server fills it from
 * `spell_runs.caller_agent_id`). Only emitted when the parent is also in the
 * displayed set — orphaned children (parent already exited) render as roots,
 * matching the layout's "no incoming = top" behavior.
 */
export function deriveSpawnEdges(agents: AgentInfo[]): DerivedSpawnEdge[] {
  const idSet = new Set(agents.map((a) => a.agent_id));
  const out: DerivedSpawnEdge[] = [];
  for (const a of agents) {
    if (!a.parent_agent_id) continue;
    if (!idSet.has(a.parent_agent_id)) continue;
    out.push({ parentId: a.parent_agent_id, childId: a.agent_id });
  }
  return out;
}
