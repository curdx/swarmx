/**
 * useWorkspaceShellData — the data-orchestration layer behind WorkspaceShell.
 *
 * Extracted from Shell.tsx (which, after the earlier UI split, still carried
 * ~260 lines of state + fetching + the swarm subscription + the optimistic
 * cascade-delete all inline). Pulling it into a hook leaves Shell as a thin
 * layout/nav component and makes this logic independently reasoned-about.
 *
 * Owns: the agents / workspaces / unread state, the three refreshers, the
 * single `/ws/swarm` subscription (the only one in the app — child views read
 * derived data via Outlet context), and the derived view-models
 * (`workspaces` / `activeWs` / per-workspace unread). Navigation stays in the
 * component: `deleteWorkspace` performs the kill+delete+optimistic-drop and
 * RETURNS where to navigate (or null), so this hook has no router dependency.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api } from "../../api/http";
import type {
  AgentInfo,
  MessageRecord,
  SwarmEvent,
  ThreadInfo,
  Workspace,
} from "../../api/types";
import { useSwarmFeed } from "../../hooks/useSwarmFeed";
import { accentToCssVar, splitWorkspacePath } from "../../lib/workspace";
import { agentInThread, mainThreadOf } from "../../lib/thread";
import type { WorkspaceSummary } from "./types";

export interface LiveRead {
  ids: number[];
  to_agent: string;
  at: number;
}

export interface WorkspaceShellData {
  agents: AgentInfo[];
  workspaces: WorkspaceSummary[];
  activeWs: WorkspaceSummary | null;
  /** The active direction (thread) resolved from the URL `:threadSlug` param,
   *  defaulting to the workspace's main thread. `null` only for a legacy/empty
   *  workspace with no thread rows. */
  activeThread: ThreadInfo | null;
  /** The workspace's main direction (slug `main`, else oldest). `null` only for
   *  a legacy/empty workspace. Views use it to fold `thread_id == null` agents
   *  into main when scoping by direction. */
  mainThread: ThreadInfo | null;
  /** Resolved slug of the active direction — `"main"` when none/unresolved.
   *  Used to scope blackboard keys `{workspace_id}/{threadSlug}/…`. */
  activeThreadSlug: string;
  allAliveAgents: AgentInfo[];
  workspaceAgentIds: string[];
  /** Historical id set (alive + killed) of agents in the ACTIVE direction.
   *  MessagesPanel filters by it so each direction is a self-contained room.
   *  For the main direction, `thread_id == null` agents fold in. */
  threadAgentIds: string[];
  /** Alive agents in the active direction (subset of `activeWs.members`). */
  threadMembers: AgentInfo[];
  liveMessage: MessageRecord | null;
  liveRead: LiveRead | null;
  /** Unread tally already filtered to the active workspace's senders. */
  activeWorkspaceUnread: Record<string, number>;
  totalUnread: number;
  refreshAgents: () => void;
  refreshWorkspaces: () => Promise<void>;
  /** Kill the workspace's live agents, soft-delete it, optimistically drop it
   *  from local state. Returns a path to navigate to when the ACTIVE workspace
   *  was deleted (`/chat/<next>` or `/chat`), else `null` (no nav needed). */
  deleteWorkspace: (workspaceId: string) => Promise<string | null>;
}

export function useWorkspaceShellData(
  wsId: string | undefined,
  threadSlug: string | undefined,
): WorkspaceShellData {
  const [agents, setAgents] = useState<AgentInfo[]>([]);
  const [workspaceRows, setWorkspaceRows] = useState<Workspace[]>([]);
  const [liveMessage, setLiveMessage] = useState<MessageRecord | null>(null);
  const [liveRead, setLiveRead] = useState<LiveRead | null>(null);
  const [unreadByFrom, setUnreadByFrom] = useState<Record<string, number>>({});
  const idToFromRef = useRef<Map<number, string>>(new Map());

  // F19: drop async results that resolve after the Shell unmounts.
  const mountedRef = useRef(true);
  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  const refreshWorkspaces = useCallback(async () => {
    try {
      const items = await api.listWorkspaces();
      if (mountedRef.current) setWorkspaceRows(items);
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn("listWorkspaces failed", err);
    }
  }, []);

  const refreshAgents = useCallback(async () => {
    try {
      const items = await api.listAgents();
      if (mountedRef.current) setAgents(items);
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn("listAgents failed", err);
    }
  }, []);

  const recomputeUnread = useCallback(async () => {
    try {
      const rows = await api.listMessages({ limit: 200 });
      const counts: Record<string, number> = {};
      const ids = new Map<number, string>();
      for (const m of rows) {
        ids.set(m.id, m.from_agent);
        if (m.read_at === null && m.to_agent === "user") {
          counts[m.from_agent] = (counts[m.from_agent] ?? 0) + 1;
        }
      }
      idToFromRef.current = ids;
      if (mountedRef.current) setUnreadByFrom(counts);
    } catch {
      /* best-effort */
    }
  }, []);

  useEffect(() => {
    refreshAgents();
    recomputeUnread();
    refreshWorkspaces();
  }, [refreshAgents, recomputeUnread, refreshWorkspaces]);

  const refreshTimerRef = useRef<number | null>(null);
  const scheduleRefresh = useCallback(() => {
    if (refreshTimerRef.current != null) {
      window.clearTimeout(refreshTimerRef.current);
    }
    refreshTimerRef.current = window.setTimeout(() => {
      refreshTimerRef.current = null;
      refreshAgents();
    }, 200);
  }, [refreshAgents]);

  useSwarmFeed({
    onEvent: (ev: SwarmEvent) => {
      switch (ev.type) {
        case "agent_state":
          scheduleRefresh();
          break;
        case "message": {
          const rec: MessageRecord = {
            id: ev.id,
            from_agent: ev.from_agent,
            to_agent: ev.to_agent,
            kind: ev.kind,
            body: ev.body,
            sent_at: ev.sent_at,
            delivered_at: null,
            read_at: null,
            in_reply_to: ev.in_reply_to ?? null,
          };
          setLiveMessage(rec);
          idToFromRef.current.set(ev.id, ev.from_agent);
          if (ev.to_agent === "user") {
            setUnreadByFrom((prev) => ({
              ...prev,
              [ev.from_agent]: (prev[ev.from_agent] ?? 0) + 1,
            }));
          }
          break;
        }
        case "message_read":
          setLiveRead({ ids: ev.ids, to_agent: ev.to_agent, at: ev.at });
          setUnreadByFrom((prev) => {
            const next = { ...prev };
            for (const id of ev.ids) {
              const from = idToFromRef.current.get(id);
              if (!from) continue;
              const cur = next[from] ?? 0;
              const dec = Math.max(0, cur - 1);
              if (dec === 0) delete next[from];
              else next[from] = dec;
            }
            return next;
          });
          break;
        case "blackboard_changed":
          // workspace name / accent now live in the `workspaces` table,
          // not the blackboard, so we don't react to blackboard events
          // for that any more. Member-count changes are picked up via
          // `agent_state` → scheduleRefresh → refreshAgents → recompute.
          break;
      }
    },
    onReconnect: () => {
      scheduleRefresh();
      recomputeUnread();
      refreshWorkspaces();
    },
  });

  // ── Workspaces (server-side, alive only) ────────────────────────────
  // Source of truth: GET /api/workspaces (deleted_at IS NULL only).
  // Agents are grouped onto these via `agent.workspace_id`.
  const workspaces = useMemo<WorkspaceSummary[]>(() => {
    const aliveByWsId = new Map<string, AgentInfo[]>();
    for (const a of agents) {
      if (a.killed_at != null || a.shim_exit != null) continue;
      if (!a.workspace_id) continue;
      const arr = aliveByWsId.get(a.workspace_id) ?? [];
      arr.push(a);
      aliveByWsId.set(a.workspace_id, arr);
    }
    return workspaceRows.map<WorkspaceSummary>((w) => {
      const { parent } = splitWorkspacePath(w.cwd);
      return {
        id: w.slug,
        workspaceId: w.id,
        path: w.cwd,
        name: w.name,
        parent,
        accentColor: accentToCssVar(w.accent),
        members: aliveByWsId.get(w.id) ?? [],
        roots: w.roots ?? [],
        threads: w.threads ?? [],
      };
    });
  }, [workspaceRows, agents]);

  const activeWs = useMemo(
    () => workspaces.find((w) => w.id === wsId) ?? null,
    [workspaces, wsId],
  );

  const allAliveAgents = useMemo(
    () => agents.filter((a) => a.killed_at == null && a.shim_exit == null),
    [agents],
  );

  const workspaceAgentIds = useMemo(() => {
    if (!activeWs) return [];
    return agents
      .filter((a) => a.workspace_id === activeWs.workspaceId)
      .map((a) => a.agent_id);
  }, [agents, activeWs]);

  // ── Active direction (thread) resolution ────────────────────────────
  // Default to the main thread (slug "main", else the oldest row). `null`
  // only for a legacy/empty workspace with no thread rows — callers then fall
  // back to plain workspace-wide scoping (single implicit direction).
  const mainThread = useMemo<ThreadInfo | null>(
    () => (activeWs ? mainThreadOf(activeWs.threads) : null),
    [activeWs],
  );

  const activeThread = useMemo<ThreadInfo | null>(() => {
    if (!activeWs || activeWs.threads.length === 0) return null;
    if (threadSlug) {
      return activeWs.threads.find((th) => th.slug === threadSlug) ?? mainThread;
    }
    return mainThread;
  }, [activeWs, threadSlug, mainThread]);

  const activeThreadSlug = activeThread?.slug ?? "main";

  const agentInActiveThread = useCallback(
    (a: AgentInfo): boolean =>
      !!activeWs && agentInThread(a, activeWs.workspaceId, activeThread, mainThread),
    [activeWs, activeThread, mainThread],
  );

  const threadAgentIds = useMemo(
    () => (activeWs ? agents.filter(agentInActiveThread).map((a) => a.agent_id) : []),
    [agents, activeWs, agentInActiveThread],
  );

  const threadMembers = useMemo(
    () => (activeWs ? activeWs.members.filter(agentInActiveThread) : []),
    [activeWs, agentInActiveThread],
  );

  // Unread is scoped to the ACTIVE direction (not the whole workspace) so the
  // toolbar badge + per-member counts match the room the user is looking at —
  // a sibling direction's unread doesn't leak into this view. (For a main-only
  // workspace threadAgentIds == workspaceAgentIds, so counts are unchanged.)
  const activeWorkspaceUnread = useMemo(() => {
    if (!activeWs) return {} as Record<string, number>;
    const threadSet = new Set(threadAgentIds);
    return Object.fromEntries(
      Object.entries(unreadByFrom).filter(([from]) => threadSet.has(from)),
    );
  }, [unreadByFrom, activeWs, threadAgentIds]);
  const totalUnread = Object.values(activeWorkspaceUnread).reduce((a, b) => a + b, 0);

  const deleteWorkspace = useCallback(
    async (workspaceId: string): Promise<string | null> => {
      // Kill any live agents belonging to this workspace before deleting the
      // row, otherwise their PTYs survive and keep burning tokens with no UI
      // handle. Per-agent failure is logged but doesn't abort the batch.
      try {
        const all = await api.listAgents();
        const live = all.filter(
          (a) =>
            a.workspace_id === workspaceId &&
            a.killed_at == null &&
            a.shim_exit == null,
        );
        await Promise.all(
          live.map((a) =>
            api.killAgent(a.agent_id).catch((e) => {
              // eslint-disable-next-line no-console
              console.warn("killAgent failed", a.agent_id, e);
            }),
          ),
        );
      } catch (err) {
        // eslint-disable-next-line no-console
        console.warn("listAgents before delete failed", err);
      }
      try {
        await api.deleteWorkspace(workspaceId);
      } catch (err) {
        // eslint-disable-next-line no-console
        console.warn("deleteWorkspace failed", err);
        return null;
      }
      // Optimistically drop it locally — the next listWorkspaces refresh would
      // catch it anyway but the UI shouldn't lag a roundtrip.
      const remaining = workspaceRows.filter((w) => w.id !== workspaceId);
      if (mountedRef.current) setWorkspaceRows(remaining);
      // Tell the caller where to navigate if the ACTIVE workspace went away.
      if (activeWs?.workspaceId === workspaceId) {
        const next = remaining[0];
        return next ? `/chat/${next.slug}` : "/chat";
      }
      return null;
    },
    [workspaceRows, activeWs],
  );

  return {
    agents,
    workspaces,
    activeWs,
    activeThread,
    mainThread,
    activeThreadSlug,
    allAliveAgents,
    workspaceAgentIds,
    threadAgentIds,
    threadMembers,
    liveMessage,
    liveRead,
    activeWorkspaceUnread,
    totalUnread,
    refreshAgents,
    refreshWorkspaces,
    deleteWorkspace,
  };
}
