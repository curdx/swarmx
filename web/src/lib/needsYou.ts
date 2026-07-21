/**
 * needsYou — 把「需要用户看一眼」的 agent 聚合成一个全局收件箱( NeedsYouBar
 * 的数据源)。判定全部复用成员栏同一套视觉管线(`resolveMemberVisual`),
 * 杜绝「成员点显示红、收件箱却没看见」两处真相。
 *
 * 两类(按严重度排序,不重复计数)——只放「需要你做个决定」的事:
 *   1. error        — 异常退出/持久错误(未登录、限流、看门狗),用户要决策
 *                     (重拉 / kill / 去登录)。
 *   2. handoff      — worker 退出了但没交付约定的产出(server 算的
 *                     `handoff_missing`),需要人或队长补位。
 *
 * 刻意不收「疑似卡住」:现代 agent 的回合可以合法跑十几二十分钟(长思考、
 * 大构建、API 重试),阈值再宽也是猜 —— 误报一次(实测:codex 明明还在
 * 出 exec 事件,被标「疑似卡住」)用户就再也不信这条收件箱了。慢着的 agent
 * 不是「需要你做的决定」,软提示留在成员栏的琥珀点里(那边只是提示、不要
 * 求行动)。waiting_dep / paused 同理不算。
 */

import type { AgentInfo, AgentLiveState } from "@/api/types";
import type { MessageRecord } from "@/api/types";
import { resolveMemberVisual } from "./agent";

export type NeedsYouKind = "error" | "handoff";

export interface NeedsYouItem {
  agent: AgentInfo;
  kind: NeedsYouKind;
}

// resolveMemberVisual 需要一份 label 表(它给成员栏显示用);这里只要视觉
// 分类(isError),label 字符串用不上,传占位。labels 是它的第 4 个参数
// (下标 3)。
const PLACEHOLDER_LABELS = new Proxy(
  {} as Record<string, string>,
  { get: () => "" },
) as Parameters<typeof resolveMemberVisual>[3];

export function deriveNeedsYou(
  members: AgentInfo[],
  liveById: Record<string, AgentLiveState | undefined>,
  messages: MessageRecord[],
  now: number = Date.now(),
): NeedsYouItem[] {
  const out: NeedsYouItem[] = [];
  for (const a of members) {
    if (a.killed_at != null || a.shim_exit != null) continue;
    const v = resolveMemberVisual(a, liveById[a.agent_id], messages, PLACEHOLDER_LABELS, now);
    if (v.isError) {
      out.push({ agent: a, kind: "error" });
      continue;
    }
    if (a.handoff_missing) {
      out.push({ agent: a, kind: "handoff" });
    }
  }
  const rank: Record<NeedsYouKind, number> = { error: 0, handoff: 1 };
  return out.sort((x, y) => rank[x.kind] - rank[y.kind]);
}
