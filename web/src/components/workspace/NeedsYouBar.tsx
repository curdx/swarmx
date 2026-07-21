/**
 * NeedsYouBar — 「需要我」全局收件箱,钉在工作区工具栏下。
 *
 * 品类共识(值班台/Devin/GitHub Agent HQ):人的注意力是被路由的稀缺
 * 资源——多 agent 编排的用法天然是「发起 → 离开 → 回来验收」,召回通道
 * 必须显眼且一键直达。这里把 error / stalled / handoff_missing 三类聚成
 * 一条,点击直达对应 agent 抽屉;没有需要处理的 agent 时整条消失,不占
 * 视觉资源(避免狼来了)。
 */

import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { AlertTriangle, PackageX, Zap } from "lucide-react";
import { cn } from "@/lib/cn";
import { deriveNeedsYou, type NeedsYouItem, type NeedsYouKind } from "@/lib/needsYou";
import { roleDisplayName } from "@/lib/agent";
import type { AgentInfo, AgentLiveState, MessageRecord } from "@/api/types";

const KIND_META: Record<
  NeedsYouKind,
  { icon: typeof AlertTriangle; chip: string; text: string; key: string }
> = {
  error: {
    icon: AlertTriangle,
    chip: "border-status-danger/40 bg-status-danger-soft",
    text: "text-status-danger",
    key: "needsYou.kind.error",
  },
  handoff: {
    icon: PackageX,
    chip: "border-accent-primary/40 bg-accent-primary-soft",
    text: "text-accent-primary-deep",
    key: "needsYou.kind.handoff",
  },
};

interface Props {
  members: AgentInfo[];
  liveById: Record<string, AgentLiveState | undefined>;
  messages: MessageRecord[];
  onOpenAgent: (agentId: string) => void;
}

export function NeedsYouBar({ members, liveById, messages, onOpenAgent }: Props) {
  const { t } = useTranslation();
  // 卡死判定是时间函数(resolveMemberVisual 的阈值),没事件进来也要让条目
  // 随时间自然出现/消失 — 5s tick 足够(成员栏同款节奏)。
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    const id = window.setInterval(() => setNow(Date.now()), 5000);
    return () => window.clearInterval(id);
  }, []);

  const items = useMemo(
    () => deriveNeedsYou(members, liveById, messages, now),
    [members, liveById, messages, now],
  );
  if (items.length === 0) return null;

  const shown = items.slice(0, 4);
  const extra = items.length - shown.length;

  return (
    <div className="flex shrink-0 items-center gap-2 border-b border-status-warning/30 bg-status-warning-soft/60 px-4 py-1.5">
      <span className="flex shrink-0 items-center gap-1.5 font-caption text-[11px] font-semibold text-status-warning">
        <Zap className="size-3.5" />
        {t("needsYou.title", { count: items.length })}
      </span>
      <div className="flex min-w-0 flex-1 items-center gap-1.5 overflow-x-auto">
        {shown.map((item) => (
          <NeedsYouChip key={item.agent.agent_id} item={item} onOpen={onOpenAgent} />
        ))}
        {extra > 0 && (
          <span className="shrink-0 font-caption text-[10px] text-foreground-tertiary">
            {t("needsYou.more", { count: extra })}
          </span>
        )}
      </div>
    </div>
  );
}

function NeedsYouChip({
  item,
  onOpen,
}: {
  item: NeedsYouItem;
  onOpen: (agentId: string) => void;
}) {
  const { t } = useTranslation();
  const meta = KIND_META[item.kind];
  const Icon = meta.icon;
  return (
    <button
      type="button"
      onClick={() => onOpen(item.agent.agent_id)}
      className={cn(
        "flex shrink-0 items-center gap-1.5 rounded-full border px-2 py-0.5 transition-colors hover:brightness-95",
        meta.chip,
      )}
      title={t("needsYou.openAgent", { role: roleDisplayName(item.agent.role) })}
    >
      <Icon className={cn("size-3", meta.text)} />
      <span className="max-w-[140px] truncate font-body text-[11px] font-medium text-foreground-primary">
        {roleDisplayName(item.agent.role)}
      </span>
      <span className={cn("font-caption text-[10px]", meta.text)}>
        {t(meta.key)}
      </span>
    </button>
  );
}
