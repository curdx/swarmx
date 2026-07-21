import { describe, expect, it } from "vitest";
import { deriveNeedsYou } from "./needsYou";
import type { AgentInfo, AgentLiveState } from "@/api/types";

const NOW = 1_800_000_000_000;

function agent(partial: Partial<AgentInfo> = {}): AgentInfo {
  return {
    agent_id: "kimi-aaaa",
    cli: "kimi",
    role: "backend",
    workspace: "/tmp/ws",
    shim_ready: true,
    shim_exit: null,
    killed_at: null,
    spawned_at: NOW - 3_600_000,
    ...partial,
  };
}

describe("deriveNeedsYou", () => {
  it("live error state → error", () => {
    const a = agent();
    const items = deriveNeedsYou(
      [a],
      { [a.agent_id]: { state: "error" } },
      [],
      NOW,
    );
    expect(items).toEqual([{ agent: a, kind: "error" }]);
  });

  it("persistent last_error without a newer recovery signal → error", () => {
    const a = agent({ last_error: "未登录", last_error_at: NOW - 60_000 });
    const items = deriveNeedsYou([a], {}, [], NOW);
    expect(items[0]?.kind).toBe("error");
  });

  it("recovery: activity strictly newer than last_error_at clears the error", () => {
    const a = agent({
      last_error: "未登录",
      last_error_at: NOW - 120_000,
      last_activity_at: NOW - 60_000,
    });
    const items = deriveNeedsYou([a], {}, [], NOW);
    expect(items).toEqual([]);
  });

  it("killed agent with error state is excluded (intentional kill is not needs-you)", () => {
    const a = agent({ killed_at: NOW - 1000 });
    const items = deriveNeedsYou(
      [a],
      { [a.agent_id]: { state: "error" } },
      [],
      NOW,
    );
    expect(items).toEqual([]);
  });

  it("stalled-looking worker is NOT needs-you (slow-but-alive is not a decision)", () => {
    // 误报教训:codex 还在正常出 exec 事件,只是回合长 —— 「疑似卡住」不许进
    // 收件箱(软提示留在成员栏琥珀点)。
    const a = agent({ parent_agent_id: "orch-1" });
    const live: AgentLiveState = {
      activity: {
        agent_id: a.agent_id,
        kind: "tool",
        label: "Bash npm run build",
        phase: "running",
        seq: 1,
        at: NOW - 301_000,
      },
    };
    expect(deriveNeedsYou([a], { [a.agent_id]: live }, [], NOW)).toEqual([]);
  });

  it("worker that was active then went silent → still not needs-you", () => {
    const a = agent({
      parent_agent_id: "orch-1",
      last_activity_at: NOW - 601_000,
    });
    expect(deriveNeedsYou([a], {}, [], NOW)).toEqual([]);
  });

  it("handoff_missing → handoff", () => {
    const a = agent({ handoff_missing: true, handoff_signal: "backend.done" });
    const items = deriveNeedsYou([a], {}, [], NOW);
    expect(items).toEqual([{ agent: a, kind: "handoff" }]);
  });

  it("healthy orchestrator with no error/stall/handoff → empty", () => {
    const a = agent({ role: "orchestrator" });
    expect(deriveNeedsYou([a], {}, [], NOW)).toEqual([]);
  });

  it("paused agent is excluded (user-caused wait, not needs-you)", () => {
    const a = agent({ paused: true });
    expect(deriveNeedsYou([a], {}, [], NOW)).toEqual([]);
  });

  it("waiting_dep state is excluded (system wait, not needs-you)", () => {
    const a = agent();
    expect(
      deriveNeedsYou([a], { [a.agent_id]: { state: "waiting_dep" } }, [], NOW),
    ).toEqual([]);
  });

  it("orders error before handoff, one item per agent", () => {
    const err = agent({ agent_id: "a-err", role: "reviewer" });
    const miss = agent({
      agent_id: "a-miss",
      handoff_missing: true,
      handoff_signal: "x.done",
    });
    const items = deriveNeedsYou(
      [miss, err],
      { [err.agent_id]: { state: "error" } },
      [],
      NOW,
    );
    expect(items.map((i) => i.kind)).toEqual(["error", "handoff"]);
    expect(new Set(items.map((i) => i.agent.agent_id)).size).toBe(2);
  });
});
