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

// ── Agent 语义状态推导 ────────────────────────────────────────────────────

import type { MessageRecord } from "../api/types";

/** Semantic states beyond the raw shim_ready / killed_at / paused
 *  triple. Inferred from the recent message stream so the UI can
 *  show "等你回复" / "正在响应" instead of a generic "在线" dot.
 *
 *  Priority (highest first): exited > paused > responding > awaiting_user
 *  > working > idle. The picker picks the first that matches. */
export type AgentSemanticStatus =
  | "exited" // killed_at or shim_exit non-null
  | "paused" // operator hit interrupt; auto-wake skipped
  | "responding" // received a message recently, hasn't replied yet
  | "awaiting_user" // last spoke TO user, no inbound since → ball in user's court
  | "working" // sent a message TO another agent in the last <WORKING_WINDOW>
  | "idle"; // nothing happening lately

/** Time windows used by the inference heuristic. Tuned for "what feels
 *  right to a human watching the chat", not for protocol correctness.
 *  - RESPONDING: how long an inbound message keeps the "responding"
 *    indicator alive before we give up and call the agent stuck.
 *  - WORKING: window during which a recent outbound to another agent
 *    means "this agent is actively doing something."
 *  Both are conservative — better to under-report than to leave a stale
 *  "正在响应" indicator on a dead agent for hours. */
const RESPONDING_WINDOW_MS = 60_000;
const WORKING_WINDOW_MS = 60_000;

/** Pure function: given an agent and recent message history, return
 *  the best-guess semantic state. Caller is responsible for passing
 *  the *right slice* of messages — typically the last ~200 of the
 *  current workspace (what MessagesPanel already loads). */
export function inferAgentStatus(
  agent: AgentInfo,
  messages: MessageRecord[],
  now: number = Date.now(),
): AgentSemanticStatus {
  // Hard states from the agent row.
  if (agent.killed_at != null || agent.shim_exit != null) return "exited";
  if (agent.paused) return "paused";

  // Find the agent's most recent inbound + outbound from the message log.
  // We iterate from newest backwards so we can short-circuit once both are
  // found. `messages` may come in either order; we don't assume sort.
  let lastInbound: MessageRecord | null = null;
  let lastOutbound: MessageRecord | null = null;
  for (let i = messages.length - 1; i >= 0; i--) {
    const m = messages[i];
    if (!lastInbound && m.to_agent === agent.agent_id) {
      // Wake messages from the system mailbox don't count as a real
      // "the user is waiting for me to reply" signal — they're internal
      // kicks. Otherwise every blackboard write would flash "responding".
      if (m.kind !== "wake") lastInbound = m;
    }
    if (!lastOutbound && m.from_agent === agent.agent_id) {
      lastOutbound = m;
    }
    if (lastInbound && lastOutbound) break;
  }

  // Responding: an inbound arrived recently, and the agent hasn't sent
  // anything AFTER it yet. Note both timestamps are unix-ms; if the
  // outbound was newer than the inbound, the agent has already replied.
  if (lastInbound && now - lastInbound.sent_at < RESPONDING_WINDOW_MS) {
    const hasRepliedAfter =
      lastOutbound != null && lastOutbound.sent_at > lastInbound.sent_at;
    if (!hasRepliedAfter) return "responding";
  }

  // Awaiting user: last outbound was TO user, AND no inbound since. The
  // canonical "scout greeted, now waiting" case. We do NOT require the
  // outbound to be recent — scout's GREET STOPped 30 minutes ago is
  // still "等你回复" until user actually says something.
  if (lastOutbound && lastOutbound.to_agent === "user") {
    const hasNewInbound =
      lastInbound != null && lastInbound.sent_at > lastOutbound.sent_at;
    if (!hasNewInbound) return "awaiting_user";
  }

  // Working: just sent something to another agent — likely mid-task.
  if (lastOutbound && now - lastOutbound.sent_at < WORKING_WINDOW_MS) {
    return "working";
  }

  // Magentic-One worker default: if this agent was spawned by another
  // agent (parent_agent_id set) and hasn't yet sent its first outbound
  // message, it's almost certainly running Bash/Read/Write tool calls
  // — those don't show up in the message stream so the previous rules
  // would mis-classify it as `idle`. A worker that's alive + shim_ready
  // and never spoken should read as "working" so the user sees a typing
  // dot in the member list while npm install / build / etc. run.
  // Once the worker sends its first message (usually the completion
  // report back to the orchestrator) the rules above take over.
  if (agent.parent_agent_id && !lastOutbound) {
    return "working";
  }

  return "idle";
}

/** 文字 label —— 只在 agent 真的"不可用"或"被操作过"时显示。
 *  日常 idle/awaiting_user/responding/working 全部用色点 + typing 动画
 *  传达,不要文字,跟微信成员列表一样简洁。 */
export function agentStatusLabel(s: AgentSemanticStatus): string {
  switch (s) {
    case "exited":
      return "已结束";
    case "paused":
      return "已暂停";
    case "responding":
    case "awaiting_user":
    case "working":
    case "idle":
      return "";
  }
}

/** 当前状态是否该渲染 typing 三点动画(像微信"对方正在输入"那样)。
 *  responding / working 都属于"AI 正在干活",视觉上是同一回事。 */
export function agentStatusIsTyping(s: AgentSemanticStatus): boolean {
  return s === "responding" || s === "working";
}

/** 色点样式。typing 状态下色点被三点动画替代,这里返回空字符串调用方
 *  自然不渲染。 */
export function agentStatusDotClass(s: AgentSemanticStatus): string {
  switch (s) {
    case "exited":
      return "bg-state-idle";
    case "paused":
      return "bg-state-idle";
    case "responding":
    case "working":
      return ""; // typing animation 替代
    case "awaiting_user":
    case "idle":
      return "bg-state-success";
  }
}
