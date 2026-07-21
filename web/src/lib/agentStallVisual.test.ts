import { describe, expect, it } from "vitest";
import { resolveMemberVisual } from "./agent";
import type { AgentInfo, AgentLiveState, MessageRecord } from "../api/types";

// 3c-bis coverage: a worker that WAS active then went silent mid-task gets a
// soft amber "no recent activity" hint (UI-only backstop for the server's
// first-response-watchdog gap) — NOT a kill, and never a false alarm on a
// worker that's still producing activity or one that has no parent.

const NOW = 1_000_000_000_000;
const IDLE_AFTER_ACTIVE_MS = 1_800_000; // 必须与 agent.ts 的源值一致(30min)

const labels = {
  spawning: "启动中",
  ready: "在线",
  thinking: "",
  idle: "",
  exited: "已终止",
  waiting_dep: "等依赖",
  error: "异常退出",
  shimExit: "已下线",
  starting: "启动中",
  stalled: "可能卡住",
  noResponse: "无响应",
} as const;

function agent(partial: Partial<AgentInfo>): AgentInfo {
  return {
    agent_id: "w1",
    cli: "codex",
    role: "Backend Engineer",
    workspace: "/tmp/x",
    shim_ready: true,
    shim_exit: null,
    killed_at: null,
    spawned_at: NOW - 1_200_000,
    depends_on: [],
    handoff_signal: "",
    parent_agent_id: "orch",
    last_activity_at: null,
    last_error: null,
    last_error_kind: null,
    last_error_at: null,
    ...partial,
  } as AgentInfo;
}

// A `ready` live state (not running/ok/error) so 3a/3b don't intercept — this
// is the "spoke once, now quiet" situation the backend watchdog can't catch.
const readyLive: AgentLiveState = { state: "ready" };

function outbound(at: number): MessageRecord {
  return { from_agent: "w1", sent_at: at } as MessageRecord;
}

describe("resolveMemberVisual — mid-task stall (3c-bis)", () => {
  it("flags amber 无响应 when a worker was active but idle past the threshold", () => {
    const a = agent({ last_activity_at: NOW - IDLE_AFTER_ACTIVE_MS - 1 });
    const v = resolveMemberVisual(a, readyLive, [], labels, NOW);
    expect(v.label).toBe(labels.noResponse);
    expect(v.dotClass).toBe("bg-state-warning");
    expect(v.typing).toBe(false);
    expect(v.isError).toBe(false); // soft hint, not an error
  });

  it("uses the freshest signal (a recent outbound msg) — not stale → not flagged", () => {
    const a = agent({ last_activity_at: NOW - IDLE_AFTER_ACTIVE_MS - 1 });
    // Outbound 1 minute ago means it's NOT actually idle.
    const v = resolveMemberVisual(a, readyLive, [outbound(NOW - 60_000)], labels, NOW);
    expect(v.label).not.toBe(labels.noResponse);
  });

  it("does NOT flag just below the threshold", () => {
    const a = agent({ last_activity_at: NOW - IDLE_AFTER_ACTIVE_MS + 5_000 });
    const v = resolveMemberVisual(a, readyLive, [], labels, NOW);
    expect(v.label).not.toBe(labels.noResponse);
  });

  it("does NOT flag an orchestrator (no parent_agent_id)", () => {
    const a = agent({
      parent_agent_id: null,
      last_activity_at: NOW - IDLE_AFTER_ACTIVE_MS - 1,
    });
    const v = resolveMemberVisual(a, readyLive, [], labels, NOW);
    expect(v.label).not.toBe(labels.noResponse);
  });

  it("lets a persisted last_error (3d) win over the idle hint — red, not amber", () => {
    const a = agent({
      last_activity_at: NOW - IDLE_AFTER_ACTIVE_MS - 1,
      last_error: "启动后无响应（可能未登录或卡住）",
      last_error_at: NOW - IDLE_AFTER_ACTIVE_MS - 1,
    });
    const v = resolveMemberVisual(a, readyLive, [], labels, NOW);
    expect(v.isError).toBe(true);
    expect(v.label).toBe(labels.error);
  });

  it("does NOT flag a never-active worker via 3c-bis (that's 3c's job)", () => {
    // No activity at all + no parent-grace expiry handled by 3c; here we just
    // assert 3c-bis itself requires everActive (a worker with zero signals
    // shouldn't reach the 3c-bis amber via this path).
    const a = agent({ last_activity_at: null, spawned_at: NOW - 10_000 });
    const v = resolveMemberVisual(a, undefined, [], labels, NOW);
    // Within startup grace, optimistic typing — definitely not the idle hint.
    expect(v.label).not.toBe(labels.noResponse);
  });
});
