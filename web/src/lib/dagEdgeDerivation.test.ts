import { describe, expect, it } from "vitest";
import {
  deriveHandoffEdges,
  deriveSpawnEdges,
  liveAgents,
} from "./dagEdgeDerivation";
import type { AgentInfo } from "../api/types";

// Minimal AgentInfo factory — only the fields the derivation reads matter.
function agent(partial: Partial<AgentInfo>): AgentInfo {
  return {
    agent_id: "a",
    killed_at: null,
    shim_exit: null,
    handoff_signal: null,
    depends_on: [],
    spawned_at: null,
    parent_agent_id: null,
    ...partial,
  } as AgentInfo;
}

describe("liveAgents", () => {
  it("keeps agents with null killed_at and shim_exit", () => {
    const a = agent({ agent_id: "a" });
    expect(liveAgents([a])).toEqual([a]);
  });

  // Regression guard: the module's header calls out `!= null` vs truthiness.
  // A `0` timestamp is a REAL value ("at epoch 0"), not "absent".
  it("treats a 0 timestamp as a real value, not absent", () => {
    expect(liveAgents([agent({ killed_at: 0 })])).toEqual([]);
    expect(liveAgents([agent({ shim_exit: 0 })])).toEqual([]);
  });
});

describe("deriveHandoffEdges", () => {
  it("links a dependent to its producer via the blackboard key", () => {
    const prod = agent({ agent_id: "p", handoff_signal: "api.spec" });
    const dep = agent({ agent_id: "d", depends_on: ["api.spec"], spawned_at: 100 });
    expect(deriveHandoffEdges([prod, dep], new Map([["api.spec", 200]]))).toEqual([
      { producerId: "p", dependentId: "d", key: "api.spec", satisfied: true },
    ]);
  });

  // Drift point #1: `>=` vs `>`. A write AT the spawn instant counts.
  it("satisfied uses >= (same-instant write counts), not >", () => {
    const prod = agent({ agent_id: "p", handoff_signal: "k" });
    const dep = agent({ agent_id: "d", depends_on: ["k"], spawned_at: 100 });
    expect(deriveHandoffEdges([prod, dep], new Map([["k", 100]]))[0].satisfied).toBe(true);
  });

  // Drift point #2: `!= null` vs `?? 0`. A null spawn time is NOT satisfied —
  // a `?? 0` regression would make `writtenAt >= 0` always true.
  it("null spawned_at is NOT satisfied", () => {
    const prod = agent({ agent_id: "p", handoff_signal: "k" });
    const dep = agent({ agent_id: "d", depends_on: ["k"], spawned_at: null });
    expect(deriveHandoffEdges([prod, dep], new Map([["k", 50]]))[0].satisfied).toBe(false);
  });

  it("unwritten key → edge drawn but not satisfied", () => {
    const prod = agent({ agent_id: "p", handoff_signal: "k" });
    const dep = agent({ agent_id: "d", depends_on: ["k"], spawned_at: 1 });
    expect(deriveHandoffEdges([prod, dep], new Map())[0].satisfied).toBe(false);
  });

  // Drift point #3: producer lookup. No producer for the key → no edge.
  it("no producer for the key → no edge", () => {
    const dep = agent({ agent_id: "d", depends_on: ["orphan"], spawned_at: 1 });
    expect(deriveHandoffEdges([dep], new Map())).toEqual([]);
  });
});

describe("deriveSpawnEdges", () => {
  it("emits parent→child only when the parent is in the displayed set", () => {
    const parent = agent({ agent_id: "p" });
    const child = agent({ agent_id: "c", parent_agent_id: "p" });
    expect(deriveSpawnEdges([parent, child])).toEqual([{ parentId: "p", childId: "c" }]);
  });

  it("orphaned child (parent absent) → no edge (renders as a root)", () => {
    const child = agent({ agent_id: "c", parent_agent_id: "gone" });
    expect(deriveSpawnEdges([child])).toEqual([]);
  });
});
