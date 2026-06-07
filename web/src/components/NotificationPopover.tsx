/**
 * NotificationPopover — bell icon → quick inbox panel.
 *
 * 不重复 /notifications 全功能 inbox (那里有 tabs / mark-all-read /
 * filter)，popover 只回答一个问题: "有什么新的，我要不要切过去看"。
 * 8 条最近，点条目跳对应 workspace 的 chat，底部一个 "查看全部" 跳完整
 * /notifications 页。
 *
 * 数据来源跟 /notifications 一致 (listMessages + listBlackboard + 实时
 * SwarmFeed)，但只关心"最近 N 条 + 是否 to=user"，不分类。
 */

import { useCallback, useEffect, useMemo, useState } from "react";
import { useNavigate } from "react-router-dom";
import { useTranslation } from "react-i18next";
import {
  Bell,
  BellOff,
  CheckCheck,
  FileText,
  MessageSquare,
  Settings as SettingsIcon,
} from "lucide-react";
import { api } from "../api/http";
import type {
  AgentInfo,
  MessageRecord,
  SwarmEvent,
  Workspace,
} from "../api/types";
import { useSwarmFeed } from "../hooks/useSwarmFeed";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import { cn } from "@/lib/cn";
import { relTime } from "@/lib/relTime";
import {
  friendlyAgent,
  humanizeBlackboard,
  isHiddenWake,
  notifBody,
} from "@/lib/notif";
import { buildRoleLookup } from "@/lib/agent";

interface Item {
  id: string;
  kind: "message" | "blackboard" | "state";
  agent?: string;
  workspace?: string;
  title: string;
  body?: string;
  at: number;
}

const MAX_ITEMS = 12;

type Tr = (k: string, opts?: Record<string, unknown>) => string;

/** `humanizeBlackboard` now lives in `@/lib/notif` — shared with the full
 *  /notifications page so the two renderings can't drift. */

/** Build a popover Item from a message record — shared by the initial fetch and
 *  the live SwarmFeed path so the two can't drift. Sender identity goes through
 *  the shared `friendlyAgent` (role label, never the raw `codex-6fc9b645` short
 *  id), and a wake renders as "系统 唤醒 {role}" — identical to the full
 *  /notifications page (the popover used to show a bare "系统" with an empty body
 *  for manual wakes, and "orchestrator 6fc9b645" via AgentChip for the rest).
 *  `agent` is kept for real ids only, purely so a click can deep-link `?agent=`. */
function itemFromMessage(
  m: Pick<
    MessageRecord,
    "id" | "from_agent" | "to_agent" | "kind" | "body" | "sent_at"
  >,
  roleLookup: Map<string, string>,
  workspace: string | undefined,
  t: Tr,
): Item {
  const isPseudo = m.from_agent === "system" || m.from_agent === "user";
  const isWake = m.kind === "wake";
  return {
    id: `msg-${m.id}`,
    kind: isWake ? "state" : "message",
    agent: isPseudo ? undefined : m.from_agent,
    workspace,
    title: isWake
      ? t("notifications.kinds.wakeTitle", {
          from: friendlyAgent(m.from_agent, roleLookup, t),
          to: friendlyAgent(m.to_agent, roleLookup, t),
        })
      : friendlyAgent(m.from_agent, roleLookup, t),
    body: notifBody(m.kind, m.body, t),
    at: m.sent_at,
  };
}

interface Props {
  hasUnseen: boolean;
  onSeen: () => void;
}

export function NotificationPopover({ hasUnseen, onSeen }: Props) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const [open, setOpen] = useState(false);
  const [items, setItems] = useState<Item[]>([]);
  const [roleLookup, setRoleLookup] = useState<Map<string, string>>(
    () => new Map(),
  );
  // agent_id → workspace path 反查 (跳对应 ws chat 时要)。和 RoleLookup
  // 一次 listAgents 就能拿到。
  const [agentWorkspaces, setAgentWorkspaces] = useState<Map<string, string>>(
    () => new Map(),
  );
  // Workspaces + their directions — used to render blackboard keys as
  // "{workspace} · {direction}" instead of the raw UUID/slug path.
  const [workspaces, setWorkspaces] = useState<Workspace[]>([]);

  // 拉一次最近事件 — popover 打开瞬间加载，不订阅常住的 fetch。
  const refresh = useCallback(async () => {
    try {
      const [msgs, bb, agents, wss] = await Promise.all([
        api.listMessages({ limit: 40 }),
        api.listBlackboard(),
        api.listAgents().catch(() => [] as AgentInfo[]),
        api.listWorkspaces().catch(() => [] as Workspace[]),
      ]);
      const wsM = new Map<string, string>();
      for (const a of agents as AgentInfo[]) {
        if (a.workspace) wsM.set(a.agent_id, a.workspace);
      }
      setAgentWorkspaces(wsM);
      const rl = buildRoleLookup(agents as AgentInfo[]);
      setRoleLookup(rl);
      setWorkspaces(wss as Workspace[]);
      const fromMsgs: Item[] = (msgs as MessageRecord[])
        .filter((m) => !isHiddenWake(m))
        .map((m) => itemFromMessage(m, rl, wsM.get(m.from_agent), t));
      const fromBb: Item[] = bb
        // Skip worker heartbeats (`<wsId>/<role>.progress.md`) — they're
        // written on every milestone and would drown out real messages.
        .filter((e) => !e.path.endsWith(".progress.md"))
        .slice(0, 20)
        .map((e) => {
          const { title, context } = humanizeBlackboard(
            e.path,
            wss as Workspace[],
            t,
          );
          return {
            id: `bb-${e.path}-${e.at}`,
            kind: "blackboard" as const,
            title,
            body: context,
            at: e.at,
          };
        });
      const merged = [...fromMsgs, ...fromBb]
        .sort((a, b) => b.at - a.at)
        .slice(0, MAX_ITEMS);
      setItems(merged);
    } catch {
      /* best-effort */
    }
  }, [t]);

  // 打开 popover 时拉新数据 + 标 seen。
  useEffect(() => {
    if (!open) return;
    refresh();
    onSeen();
  }, [open, refresh, onSeen]);

  // 关着也接 live event → 让红点能动；但不重新 fetch 列表 (open 时再拉)。
  useSwarmFeed({
    onEvent: (ev: SwarmEvent) => {
      if (!open) return;
      // popover 当前打开时把新事件 prepend，用户能看到实时滚动。
      let next: Item | null = null;
      if (ev.type === "message") {
        if (isHiddenWake(ev)) return;
        next = itemFromMessage(
          ev,
          roleLookup,
          agentWorkspaces.get(ev.from_agent),
          t,
        );
      } else if (ev.type === "blackboard_changed") {
        if (ev.path.endsWith(".progress.md")) return; // skip heartbeats
        const { title, context } = humanizeBlackboard(ev.path, workspaces, t);
        next = {
          id: `bb-${ev.path}-${ev.at}`,
          kind: "blackboard",
          title,
          body: context,
          at: ev.at,
        };
      }
      if (!next) return;
      setItems((prev) =>
        prev.some((p) => p.id === next!.id)
          ? prev
          : [next!, ...prev].slice(0, MAX_ITEMS),
      );
    },
  });

  const handleItemClick = (item: Item) => {
    setOpen(false);
    if (item.workspace) {
      // 跳到对应 workspace 的 chat (wsId = path 末 8 字符)
      const wsId = item.workspace.slice(-8);
      if (item.kind === "message" && item.agent) {
        navigate(`/chat/${wsId}?agent=${encodeURIComponent(item.agent)}`);
      } else {
        navigate(`/chat/${wsId}/ledger`);
      }
    } else {
      // No resolvable workspace (e.g. a message from an agent that spawned
      // AFTER the popover last refreshed its agent→workspace map, or a
      // workspace-less blackboard event). Fall back to the full notification
      // center instead of a dead no-op click.
      navigate("/notifications");
    }
  };

  const ItemIcon = useMemo(
    () => ({
      message: MessageSquare,
      blackboard: FileText,
      state: SettingsIcon,
    }),
    [],
  );

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <button
          type="button"
          className="relative flex size-7 items-center justify-center rounded-md text-foreground-tertiary transition-colors hover:bg-surface-tertiary hover:text-foreground-primary"
          aria-label={t("nav.notifications")}
        >
          <Bell className="size-4" />
          {hasUnseen && (
            <span className="absolute right-1 top-1 size-1.5 rounded-full bg-state-danger" />
          )}
        </button>
      </PopoverTrigger>
      <PopoverContent
        align="end"
        sideOffset={6}
        className="w-[360px] p-0"
      >
        <div className="flex items-center justify-between border-b border-border-subtle px-3 py-2">
          <div className="flex items-center gap-1.5">
            <Bell className="size-3.5 text-foreground-tertiary" />
            <span className="font-heading text-xs font-semibold text-foreground-primary">
              {t("nav.notifications")}
            </span>
          </div>
          <button
            type="button"
            onClick={() => {
              setOpen(false);
              navigate("/notifications");
            }}
            className="flex items-center gap-1 rounded px-1.5 py-0.5 font-caption text-[10px] text-foreground-tertiary transition-colors hover:bg-surface-tertiary hover:text-foreground-primary"
          >
            <CheckCheck className="size-3" />
            {t("notifications.viewAll")}
          </button>
        </div>
        <ul className="max-h-[420px] overflow-y-auto py-1">
          {items.length === 0 ? (
            <li className="flex flex-col items-center gap-2 px-4 py-12 text-foreground-tertiary">
              <BellOff className="size-7 opacity-40" />
              <span className="font-caption text-xs">
                {t("notifications.empty")}
              </span>
            </li>
          ) : (
            items.map((item) => {
              const Icon = ItemIcon[item.kind];
              return (
                <li key={item.id}>
                  <button
                    type="button"
                    onClick={() => handleItemClick(item)}
                    className={cn(
                      "flex w-full items-start gap-2 px-3 py-2 text-left transition-colors hover:bg-surface-tertiary",
                    )}
                  >
                    <Icon className="mt-0.5 size-3.5 shrink-0 text-foreground-tertiary" />
                    <div className="min-w-0 flex-1">
                      <div className="flex items-center gap-1.5">
                        <span className="truncate font-heading text-[11px] font-medium text-foreground-primary">
                          {item.title}
                        </span>
                        <span className="ml-auto shrink-0 font-caption text-[10px] text-foreground-tertiary">
                          {relTime(item.at, t)}
                        </span>
                      </div>
                      {item.body && (
                        <p className="mt-0.5 line-clamp-2 font-caption text-[11px] text-foreground-secondary">
                          {item.body}
                        </p>
                      )}
                    </div>
                  </button>
                </li>
              );
            })
          )}
        </ul>
      </PopoverContent>
    </Popover>
  );
}
