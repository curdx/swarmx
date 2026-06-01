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
import { AgentChip } from "@/components/agent/AgentChip";
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

/** A blackboard key is `{workspace_id}/{thread_slug}/{file}`. Render it as
 *  human text — a friendly ledger label + the workspace/direction names —
 *  instead of the raw 32-char UUID + slug + content hash the user can't read. */
function humanizeBlackboard(
  path: string,
  workspaces: Workspace[],
  t: Tr,
): { title: string; context?: string } {
  const segs = path.split("/").filter(Boolean);
  if (segs.length < 3) {
    return { title: segs[segs.length - 1] ?? path };
  }
  const [wsid, slug] = segs;
  const file = segs.slice(2).join("/");
  const title =
    file === "task.ledger.md"
      ? t("notifications.bb.taskLedger")
      : file === "progress.ledger.md"
        ? t("notifications.bb.progressLedger")
        : t("notifications.bb.update", { name: segs[segs.length - 1] });
  // Prefer an exact workspace-id match; fall back to locating the direction by
  // its (workspace-unique) slug so this still resolves if the id scheme drifts.
  const ws =
    workspaces.find((w) => w.id === wsid) ??
    workspaces.find((w) => (w.threads ?? []).some((th) => th.slug === slug));
  const thread = (ws?.threads ?? []).find((th) => th.slug === slug);
  const dirName = thread?.name?.trim()
    ? thread.name.trim()
    : slug === "main"
      ? t("notifications.bb.mainDir")
      : thread
        ? slug
        : undefined;
  const context = [ws?.name, dirName].filter(Boolean).join(" · ");
  return { title, context: context || undefined };
}

/** `user` / `system` are pseudo-agents, not roles — rendering them through the
 *  AgentChip prints a doubled "user user" (role-prefix == short-id). Give them a
 *  plain friendly label instead. Returns null for a real agent id. */
function pseudoFrom(from: string, t: Tr): string | null {
  if (from === "user") return t("notifications.fromUser");
  if (from === "system") return t("notifications.fromSystem");
  return null;
}

interface Props {
  hasUnseen: boolean;
  onSeen: () => void;
}

function relTime(ms: number, t: (k: string, opts?: Record<string, unknown>) => string): string {
  const d = Date.now() - ms;
  if (d < 60_000) return t("notifications.time.now");
  if (d < 3_600_000) return t("notifications.time.minAgo", { n: Math.floor(d / 60_000) });
  if (d < 86_400_000) return t("notifications.time.hourAgo", { n: Math.floor(d / 3_600_000) });
  return new Date(ms).toLocaleString();
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
      setRoleLookup(buildRoleLookup(agents as AgentInfo[]));
      setWorkspaces(wss as Workspace[]);
      const fromMsgs: Item[] = (msgs as MessageRecord[]).map((m) => {
        const pseudo = pseudoFrom(m.from_agent, t);
        return {
          id: `msg-${m.id}`,
          kind: "message",
          agent: pseudo ? undefined : m.from_agent,
          workspace: wsM.get(m.from_agent),
          title: pseudo ?? m.from_agent,
          body: m.body,
          at: m.sent_at,
        };
      });
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
        const pseudo = pseudoFrom(ev.from_agent, t);
        next = {
          id: `msg-${ev.id}`,
          kind: "message",
          agent: pseudo ? undefined : ev.from_agent,
          workspace: agentWorkspaces.get(ev.from_agent),
          title: pseudo ?? ev.from_agent,
          body: ev.body,
          at: ev.sent_at,
        };
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
                        {item.agent ? (
                          <AgentChip
                            agentId={item.agent}
                            roleLookup={roleLookup}
                            size="xs"
                            showAvatar={false}
                          />
                        ) : (
                          <span className="truncate font-heading text-[11px] font-medium text-foreground-primary">
                            {item.title}
                          </span>
                        )}
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
