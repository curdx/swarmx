import type { AgentInfo, AgentLiveState, MessageRecord } from "../api/types";

/**
 * The honesty-critical "正在响应 / 消失的回合" inference, pulled out of
 * MessagesPanel as pure, clock-injectable functions so the rules can be
 * unit-tested without a DOM or a live agent. The hook/memos own the React
 * wiring (the 5s tick, the deps array); these functions own the logic:
 * when does a responding bubble show, when does it give up, and when did a
 * turn vanish. Extracted verbatim from the pendingResponders / vanishedTurns
 * memos — same filters, same ordering, same edge cases.
 */

/** Drop a still-pending bubble once the agent has been TRULY silent this long
 *  since its last sign of life (the trigger, or its last real tool event). A
 *  captain mid a long model-thinking turn keeps its bubble; each real activity
 *  event renews it. Re-evaluated by the panel's 5s tick so it fires even with
 *  no new WS traffic. */
export const PENDING_SILENCE_GIVEUP_MS = 180_000;

const USER_SENDER = "user";

/**
 * Clock-skew-proof interrupt match: a turn counts as interrupted only when the
 * user cancelled THAT exact trigger, matched by the server-assigned monotonic
 * message id. A newer message has a new id so it reopens naturally; wall-clock
 * drift can never affect an id equality.
 */
export function isInterrupted(
  interruptedTriggers: Record<string, number>,
  agentId: string,
  triggerId: number,
): boolean {
  return interruptedTriggers[agentId] === triggerId;
}

/**
 * Agents alive enough to still be "responding": not shim-exited, not killed,
 * and whose live swarm state hasn't gone error/exited. (P0-3: bind to the REAL
 * swarm state so a dead agent's typing placeholder vanishes in ≤1 render
 * instead of lying for the whole give-up window.)
 */
export function computeAliveIds(
  aliveForInference: AgentInfo[],
  agentLiveStateById: Record<string, AgentLiveState> | undefined,
): Set<string> {
  return new Set(
    aliveForInference
      .filter((m) => {
        if (m.shim_exit != null || m.killed_at != null) return false;
        const st = agentLiveStateById?.[m.agent_id]?.state;
        if (st === "error" || st === "exited") return false;
        return true;
      })
      .map((m) => m.agent_id),
  );
}

export interface PendingResponder {
  agentId: string;
  trigger: MessageRecord;
}

/**
 * Who is currently showing a "正在响应" bubble. An agent qualifies when it is
 * alive, has an unanswered user→agent trigger (the latest message TO it is
 * newer than the latest message FROM it), hasn't been truly silent past the
 * give-up window, and the user hasn't interrupted that exact turn. Sorted
 * earliest-triggered first so older waiters appear above. `now` is injected so
 * the give-up cutoff is testable without a real clock.
 */
export function derivePendingResponders(input: {
  items: MessageRecord[];
  aliveForInference: AgentInfo[];
  agentLiveStateById: Record<string, AgentLiveState> | undefined;
  interruptedTriggers: Record<string, number>;
  now: number;
}): PendingResponder[] {
  const { items, aliveForInference, agentLiveStateById, interruptedTriggers, now } =
    input;
  const aliveIds = computeAliveIds(aliveForInference, agentLiveStateById);
  if (aliveIds.size === 0) return [];
  const lastSent = new Map<string, number>();
  const lastReceived = new Map<string, MessageRecord>();
  for (const m of items) {
    if (aliveIds.has(m.from_agent)) {
      const prev = lastSent.get(m.from_agent) ?? 0;
      if (m.sent_at > prev) lastSent.set(m.from_agent, m.sent_at);
    }
    if (aliveIds.has(m.to_agent)) {
      const prev = lastReceived.get(m.to_agent);
      if (!prev || m.sent_at > prev.sent_at) {
        lastReceived.set(m.to_agent, m);
      }
    }
  }
  const out: PendingResponder[] = [];
  for (const [agentId, trigger] of lastReceived) {
    const sentAt = lastSent.get(agentId) ?? 0;
    if (trigger.sent_at <= sentAt) continue;
    const lastActivityAt = agentLiveStateById?.[agentId]?.activity?.at ?? 0;
    const lastSignOfLife = Math.max(trigger.sent_at, lastActivityAt);
    if (now - lastSignOfLife > PENDING_SILENCE_GIVEUP_MS) continue;
    if (isInterrupted(interruptedTriggers, agentId, trigger.id)) continue;
    out.push({ agentId, trigger });
  }
  out.sort((a, b) => a.trigger.sent_at - b.trigger.sent_at);
  return out;
}

export interface VanishedTurn {
  agentId: string;
  trigger: MessageRecord;
  reason: string | null;
}

/**
 * Turns that vanished: an agent received a user task as its LATEST user trigger,
 * then went error/exited without replying and without being interrupted. A
 * newer user message anywhere (including 重新发送) supersedes an old vanished
 * card so a dead agent's stranded task doesn't linger forever. `reason` carries
 * the error label only for explicit-error deaths; a neutral kill/exit is null.
 */
export function deriveVanishedTurns(input: {
  items: MessageRecord[];
  agentLiveStateById: Record<string, AgentLiveState> | undefined;
  interruptedTriggers: Record<string, number>;
}): VanishedTurn[] {
  const { items, agentLiveStateById, interruptedTriggers } = input;
  const lastTrigger = new Map<string, MessageRecord>();
  const lastReplyAt = new Map<string, number>();
  let latestUserTriggerAt = 0;
  for (const m of items) {
    if (m.from_agent === USER_SENDER && m.to_agent !== USER_SENDER) {
      if (m.sent_at > latestUserTriggerAt) latestUserTriggerAt = m.sent_at;
      const prev = lastTrigger.get(m.to_agent);
      if (!prev || m.sent_at > prev.sent_at) lastTrigger.set(m.to_agent, m);
    } else if (m.to_agent === USER_SENDER && m.from_agent !== USER_SENDER) {
      const prev = lastReplyAt.get(m.from_agent) ?? 0;
      if (m.sent_at > prev) lastReplyAt.set(m.from_agent, m.sent_at);
    }
  }
  const out: VanishedTurn[] = [];
  for (const [agentId, trigger] of lastTrigger) {
    if (trigger.sent_at < latestUserTriggerAt) continue; // 用户已发更新消息 → 取代旧卡
    const live = agentLiveStateById?.[agentId];
    const dead = live?.state === "error" || live?.state === "exited";
    if (!dead) continue; // 还活着(含"慢但在跑")→ 不喊狼来了
    if ((lastReplyAt.get(agentId) ?? 0) >= trigger.sent_at) continue; // 已回复
    if (isInterrupted(interruptedTriggers, agentId, trigger.id)) continue; // 用户主动打断
    out.push({
      agentId,
      trigger,
      reason: live?.state === "error" ? live.activity?.label ?? null : null,
    });
  }
  return out;
}
