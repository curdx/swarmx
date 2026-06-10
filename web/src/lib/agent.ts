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

import type { AgentInfo, AgentLiveState, SwarmAgentState } from "../api/types";

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

/** Thresholds for the activity-aware honesty layer in `resolveMemberVisual`
 *  (F3). Tuned to err toward NOT crying wolf — a soft amber "可能卡住" is fine
 *  to surface late, a false alarm on a legit long build is not.
 *  - STALL_RUNNING: a single tool stuck in "running" this long (no ok/error
 *    event) reads as wedged. Set above a normal build/test so a real long tool
 *    doesn't trip it.
 *  - STARTUP_GRACE: a freshly-spawned worker gets this long to produce its
 *    first tool event before "no signal at all" stops reading as "booting".
 *  - NO_RESPONSE: a ready worker that has produced ZERO observable activity
 *    this long after spawn is almost certainly wedged / never started. */
const STALL_RUNNING_MS = 300_000;
const STARTUP_GRACE_MS = 45_000;
const NO_RESPONSE_MS = 300_000;

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

  // A worker that's alive but hasn't spoken used to be ASSUMED "working" for
  // 10 minutes here (so a typing dot showed while npm install / build ran).
  // That was a lie source — a silently-wedged or already-done worker also
  // shows no outbound, so it kept "typing" for 10 minutes. We now decide
  // worker liveness from real tool-level activity (see resolveMemberVisual's
  // activity layer), so this function just reports the honest message-stream
  // verdict: no recent inbound/outbound → idle.
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

// ── 成员栏实时态推导（真实 state + activity，含向后兼容回退）─────────────

/** 成员栏一行的视觉决策。`tone` 决定色点 / typing / 文字,`label` 是
 *  "已终止" / "等依赖" 这类状态词(可空 → 不显示文字,只显示色点或 typing)。
 *  `typing` 为 true 时调用方渲染三点动画替代色点。`isError` 用于把出错
 *  成员顶到列表最前。 */
export interface MemberVisual {
  /** 色点颜色类(如 "bg-status-danger")。typing=true 或纯文字态时为空串。 */
  dotClass: string;
  /** 状态文字("已终止"/"等依赖"/…)。空串 = 不显示文字。 */
  label: string;
  /** 是否渲染三点 typing 动画(运行中)。 */
  typing: boolean;
  /** 是否为异常退出(未被主动 kill)。调用方据此把该成员顶到最前。 */
  isError: boolean;
}

/** 把 agent 的硬状态(killed/shim_exit) + 真实 swarm state(若有) + 消息流
 *  推导,合成成员栏一行的视觉。优先级(高→低):
 *    killed_at  → "已终止" 灰(主动 kill,绝不判红,即便 state=error)
 *    shim_exit  → "已下线" 灰
 *    !shim_ready→ "启动中" 黄(还在拉起 PTY)
 *    state=error→ 红点 + "异常退出",并 isError=true(顶到最前)
 *    state=waiting_dep → 灰点 + "等依赖"
 *    state=running/thinking/spawning → typing 动画
 *    state=ready/idle 或 state 缺失 → 回退 inferAgentStatus(向后兼容)
 *
 *  `live` 是 swarm WS 累积的真实状态切片;缺失(老 server / 事件未到)时整体
 *  回退到基于消息流的 inferAgentStatus,行为与改造前一致。 */
export function resolveMemberVisual(
  agent: AgentInfo,
  live: AgentLiveState | undefined,
  messages: MessageRecord[],
  labels: Record<
    SwarmAgentState | "exited" | "shimExit" | "starting" | "stalled" | "noResponse",
    string
  >,
  now: number = Date.now(),
): MemberVisual {
  // 1) 硬状态优先——killed_at 永远先于 error,避免主动 kill 误判红。
  if (agent.killed_at != null) {
    return { dotClass: "bg-state-idle", label: labels.exited, typing: false, isError: false };
  }
  if (agent.shim_exit != null) {
    return { dotClass: "bg-state-idle", label: labels.shimExit, typing: false, isError: false };
  }
  if (!agent.shim_ready) {
    return { dotClass: "bg-state-wake", label: labels.starting, typing: false, isError: false };
  }

  // 恢复守卫:一个比"记录的错误时刻"更新的生命信号,说明 agent 在软错误
  // (终端内 /login、慢首响)后已恢复干活。后端 tailer 会清 last_error 并 publish
  // 非 error 状态,这里只兜住"恢复活动已到、后端清除事件尚未到/被 lossy WS 丢掉"
  // 的窗口。绝不会造成假绿:仅当存在严格新于 last_error_at 的活动时才放行。
  const errAt = agent.last_error_at ?? null;
  const freshSignalAt = Math.max(agent.last_activity_at ?? 0, live?.activity?.at ?? 0);
  const recoveredSinceError = errAt != null && freshSignalAt > errAt;

  // 2) 真实 swarm state(若已收到事件)。
  const st = live?.state;
  if (st === "error" && !recoveredSinceError) {
    return { dotClass: "bg-status-danger", label: labels.error, typing: false, isError: true };
  }
  if (st === "waiting_dep") {
    return { dotClass: "bg-state-idle", label: labels.waiting_dep, typing: false, isError: false };
  }
  if (st === "thinking" || st === "spawning") {
    return { dotClass: "", label: "", typing: true, isError: false };
  }

  // 3) 活动诚实层(F3):ready/idle 这类 state 不表达"是否真在干活"。用工具级
  //    activity + 持久化的 last_activity_at + 消息流,把"卡住/无响应"与"idle"
  //    区分开,不再一律绿点/typing 撒谎。
  const stalled = (label: string): MemberVisual => ({
    dotClass: "bg-state-warning",
    label,
    typing: false,
    isError: false,
  });
  const act = live?.activity;
  // 3a) 某个工具"running"卡太久 → 大概率卡死,别再 typing 假装在跑。
  if (act && act.phase === "running") {
    if (now - act.at >= STALL_RUNNING_MS) return stalled(labels.stalled);
    return { dotClass: "", label: "", typing: true, isError: false }; // 真在跑某工具
  }
  // 3b) 工具刚结束(ok/error)→ 仍在活跃干活。
  if (act && (act.phase === "ok" || act.phase === "error") && now - act.at < WORKING_WINDOW_MS) {
    return { dotClass: "", label: "", typing: true, isError: false };
  }

  // 3c) 从未产生任何"活着"的信号:区分"从未起跑/卡死"与"idle"。只对 worker
  //     (有 parent)判定——orchestrator 不被 tail,其活性由消息流体现(走兜底)。
  const hasOutbound = messages.some((m) => m.from_agent === agent.agent_id);
  const everActive = act != null || agent.last_activity_at != null || hasOutbound;
  if (agent.parent_agent_id && !everActive) {
    const spawnedAt = agent.spawned_at ?? null;
    const age = spawnedAt == null ? 0 : now - spawnedAt;
    // 启动宽限期内:乐观显示在跑(正在拉起第一个工具)。
    if (age < STARTUP_GRACE_MS) return { dotClass: "", label: "", typing: true, isError: false };
    // 超过无响应阈值仍零活动 → 卡死/没起来。这就是 QA 要的"从未起跑/卡死"信号。
    if (age >= NO_RESPONSE_MS) return stalled(labels.noResponse);
    // 中间地带:安静但还不到报警,给中性绿点(不撒谎说在干活)。
    return { dotClass: "bg-state-success", label: "", typing: false, isError: false };
  }

  // 3d) 冷加载诚实层:live error 事件在刷新后丢失(WS 无 resume),但持久化的
  //     last_error(未登录/限流/看门狗)说明这个 agent 起来了却干不了活。走到
  //     这里 = 没有更新的 live 活动信号,所以照 last_error 判红,和主视图失败卡
  //     同步(否则刷新后成员点会假装变绿)。recoveredSinceError 守住"已恢复但
  //     last_error 还没被后端清掉"的窗口(否则恢复后 idle>60s 的 agent 会误显红)。
  if (agent.last_error && !recoveredSinceError) {
    return { dotClass: "bg-status-danger", label: labels.error, typing: false, isError: true };
  }

  // 4) 回退:消息流语义推导(orchestrator + 已说过话的 worker;state 缺失也走这里)。
  const semantic = inferAgentStatus(agent, messages, now);
  return {
    dotClass: agentStatusDotClass(semantic),
    label: agentStatusLabel(semantic),
    typing: agentStatusIsTyping(semantic),
    isError: false,
  };
}

/** 把一条 activity 渲染成成员栏第二行的 "此刻在干嘛" 文案 + 是否仍在跑。
 *  running 时无 duration_ms,用 at→now 算 elapsed;ok/error 用 duration_ms。
 *  返回 null = 没有可展示的活动(回退显示 cli·id)。 */
export function formatActivityLine(
  live: AgentLiveState | undefined,
  now: number = Date.now(),
): { label: string; phase: "running" | "ok" | "error"; elapsedMs: number } | null {
  const act = live?.activity;
  if (!act) return null;
  const elapsedMs =
    act.phase === "running" ? Math.max(0, now - act.at) : (act.duration_ms ?? 0);
  return { label: act.label, phase: act.phase, elapsedMs };
}
