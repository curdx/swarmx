import { describe, it, expect } from "vitest";
import type { AgentInfo, AgentLiveState, MessageRecord } from "../api/types";
import {
  PENDING_SILENCE_GIVEUP_MS,
  computeAliveIds,
  derivePendingResponders,
  deriveVanishedTurns,
  isInterrupted,
} from "./pendingResponders";

// ── builders ────────────────────────────────────────────────────────────────
function msg(
  p: Partial<MessageRecord> &
    Pick<MessageRecord, "id" | "from_agent" | "to_agent" | "sent_at">,
): MessageRecord {
  return {
    kind: "chat",
    body: "",
    delivered_at: null,
    read_at: null,
    in_reply_to: null,
    ...p,
  };
}

function agent(
  p: Partial<AgentInfo> & Pick<AgentInfo, "agent_id">,
): AgentInfo {
  return {
    cli: "claude",
    role: "captain",
    workspace: "w",
    shim_ready: true,
    shim_exit: null,
    ...p,
  };
}

const A = "agent-a";
const U = "user";
const live = (
  state?: AgentLiveState["state"],
  activityAt?: number,
  label?: string,
): Record<string, AgentLiveState> => ({
  [A]: {
    state,
    activity: activityAt != null ? { agent_id: A, kind: "tool", label: label ?? "x", phase: "running", seq: 1, at: activityAt } : undefined,
  },
});

describe("isInterrupted", () => {
  it("matches only the exact agent+trigger id", () => {
    expect(isInterrupted({ [A]: 7 }, A, 7)).toBe(true);
    expect(isInterrupted({ [A]: 7 }, A, 8)).toBe(false); // newer turn reopens
    expect(isInterrupted({ [A]: 7 }, "other", 7)).toBe(false);
    expect(isInterrupted({}, A, 7)).toBe(false);
  });
});

describe("computeAliveIds", () => {
  it("keeps healthy agents, drops shim_exit / killed_at / error / exited", () => {
    const members = [
      agent({ agent_id: "ok" }),
      agent({ agent_id: "shimdead", shim_exit: 1 }),
      agent({ agent_id: "killed", killed_at: 123 }),
      agent({ agent_id: "errored" }),
      agent({ agent_id: "exitedone" }),
    ];
    const ids = computeAliveIds(members, {
      errored: { state: "error" },
      exitedone: { state: "exited" },
    });
    expect([...ids].sort()).toEqual(["ok"]);
  });

  it("treats undefined live-state map as all-alive (modulo shim/killed)", () => {
    const ids = computeAliveIds([agent({ agent_id: "ok" })], undefined);
    expect(ids.has("ok")).toBe(true);
  });
});

describe("derivePendingResponders", () => {
  const base = {
    aliveForInference: [agent({ agent_id: A })],
    agentLiveStateById: {} as Record<string, AgentLiveState> | undefined,
    interruptedTriggers: {} as Record<string, number>,
    now: 1_000_000,
  };

  it("shows an alive agent with an unanswered user trigger", () => {
    const out = derivePendingResponders({
      ...base,
      items: [msg({ id: 1, from_agent: U, to_agent: A, sent_at: 1_000_000 })],
    });
    expect(out).toEqual([{ agentId: A, trigger: expect.objectContaining({ id: 1 }) }]);
  });

  it("clears once the agent replied after the trigger", () => {
    const out = derivePendingResponders({
      ...base,
      items: [
        msg({ id: 1, from_agent: U, to_agent: A, sent_at: 1_000_000 }),
        msg({ id: 2, from_agent: A, to_agent: U, sent_at: 1_000_001 }), // reply
      ],
    });
    expect(out).toEqual([]);
  });

  it("drops an agent whose live state is error/exited (not alive)", () => {
    for (const state of ["error", "exited"] as const) {
      const out = derivePendingResponders({
        ...base,
        agentLiveStateById: { [A]: { state } },
        items: [msg({ id: 1, from_agent: U, to_agent: A, sent_at: 1_000_000 })],
      });
      expect(out).toEqual([]);
    }
  });

  it("drops after true silence past the give-up window (no activity)", () => {
    const trigger = 1_000_000;
    const out = derivePendingResponders({
      ...base,
      now: trigger + PENDING_SILENCE_GIVEUP_MS + 1,
      items: [msg({ id: 1, from_agent: U, to_agent: A, sent_at: trigger })],
    });
    expect(out).toEqual([]);
  });

  it("renews past the give-up window when a fresh activity event arrived", () => {
    const trigger = 1_000_000;
    const now = trigger + PENDING_SILENCE_GIVEUP_MS + 10_000;
    const out = derivePendingResponders({
      ...base,
      now,
      agentLiveStateById: live(undefined, now - 1_000), // tool event 1s ago
      items: [msg({ id: 1, from_agent: U, to_agent: A, sent_at: trigger })],
    });
    expect(out.map((p) => p.agentId)).toEqual([A]);
  });

  it("drops the exact interrupted trigger but reopens for a newer id", () => {
    const items = [msg({ id: 5, from_agent: U, to_agent: A, sent_at: 1_000_000 })];
    expect(
      derivePendingResponders({ ...base, interruptedTriggers: { [A]: 5 }, items }),
    ).toEqual([]);
    // a newer message (id 6) is a different turn → shows again
    const items2 = [msg({ id: 6, from_agent: U, to_agent: A, sent_at: 1_000_050 })];
    expect(
      derivePendingResponders({ ...base, interruptedTriggers: { [A]: 5 }, items: items2 })
        .map((p) => p.trigger.id),
    ).toEqual([6]);
  });

  it("sorts earliest-triggered first", () => {
    const B = "agent-b";
    const out = derivePendingResponders({
      ...base,
      aliveForInference: [agent({ agent_id: A }), agent({ agent_id: B })],
      items: [
        msg({ id: 1, from_agent: U, to_agent: B, sent_at: 1_000_200 }),
        msg({ id: 2, from_agent: U, to_agent: A, sent_at: 1_000_100 }),
      ],
    });
    expect(out.map((p) => p.agentId)).toEqual([A, B]); // A triggered earlier
  });

  it("returns [] when nobody is alive", () => {
    expect(
      derivePendingResponders({
        ...base,
        aliveForInference: [agent({ agent_id: A, shim_exit: 0 })],
        items: [msg({ id: 1, from_agent: U, to_agent: A, sent_at: 1_000_000 })],
      }),
    ).toEqual([]);
  });
});

describe("deriveVanishedTurns", () => {
  const base = {
    interruptedTriggers: {} as Record<string, number>,
  };

  it("flags a dead agent that never replied to its latest user trigger", () => {
    const out = deriveVanishedTurns({
      ...base,
      agentLiveStateById: { [A]: { state: "error", activity: { agent_id: A, kind: "system", label: "auth failed", phase: "error", seq: 1, at: 5 } } },
      items: [msg({ id: 1, from_agent: U, to_agent: A, sent_at: 1_000_000 })],
    });
    expect(out).toEqual([
      { agentId: A, trigger: expect.objectContaining({ id: 1 }), reason: "auth failed" },
    ]);
  });

  it("reason is null for a neutral exit (not an explicit error)", () => {
    const out = deriveVanishedTurns({
      ...base,
      agentLiveStateById: { [A]: { state: "exited" } },
      items: [msg({ id: 1, from_agent: U, to_agent: A, sent_at: 1_000_000 })],
    });
    expect(out).toEqual([{ agentId: A, trigger: expect.objectContaining({ id: 1 }), reason: null }]);
  });

  it("no card while the agent is still alive", () => {
    const out = deriveVanishedTurns({
      ...base,
      agentLiveStateById: { [A]: { state: "thinking" } },
      items: [msg({ id: 1, from_agent: U, to_agent: A, sent_at: 1_000_000 })],
    });
    expect(out).toEqual([]);
  });

  it("no card once the agent replied", () => {
    const out = deriveVanishedTurns({
      ...base,
      agentLiveStateById: { [A]: { state: "exited" } },
      items: [
        msg({ id: 1, from_agent: U, to_agent: A, sent_at: 1_000_000 }),
        msg({ id: 2, from_agent: A, to_agent: U, sent_at: 1_000_001 }),
      ],
    });
    expect(out).toEqual([]);
  });

  it("supersedes an old vanished card once a newer user trigger exists", () => {
    // A's stranded task is older than a brand-new user message to someone else
    const out = deriveVanishedTurns({
      ...base,
      agentLiveStateById: { [A]: { state: "exited" } },
      items: [
        msg({ id: 1, from_agent: U, to_agent: A, sent_at: 1_000_000 }),
        msg({ id: 2, from_agent: U, to_agent: "agent-b", sent_at: 1_000_500 }), // newer
      ],
    });
    expect(out).toEqual([]);
  });

  it("no card when the user interrupted that exact turn", () => {
    const out = deriveVanishedTurns({
      ...base,
      interruptedTriggers: { [A]: 1 },
      agentLiveStateById: { [A]: { state: "error" } },
      items: [msg({ id: 1, from_agent: U, to_agent: A, sent_at: 1_000_000 })],
    });
    expect(out).toEqual([]);
  });
});
