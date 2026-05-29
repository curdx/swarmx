/**
 * WorkspaceShell — 4 个工作空间内 view (chat / dag / replays / context) 的
 * 共享 chrome。React Router 拿它当 layout route：所有 /chat/:wsId/* 都进这
 * 一层，子 view 通过 <Outlet/> 渲染。
 *
 * 解决的核心问题：之前每个子 view 是独立的 top-level route，切 tab → 整页
 * 卸载 + 重画，连工作空间列表都不在了，用户感觉自己"跳了页面"而不是
 * "切了视图"。Shell 化之后切 tab 只重渲染 Outlet：
 *   - 左侧工作空间列表常驻
 *   - Channel header（workspace 名 + 路径 + 未读 + 复制）常驻
 *   - Tab bar 常驻
 *   - swarm event 订阅常驻 → 切走再回来 unread/agent state 不丢
 *
 * Outlet context 把 activeWs / agents / liveMessage 等下发给子 view，
 * 避免每个 view 自己 listAgents / 开 swarm subscription（之前协作图
 * 和录像页各开了一个，重复请求 + 重复 ws 连接）。
 */

import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import {
  NavLink,
  Outlet,
  useLocation,
  useNavigate,
  useOutletContext,
  useParams,
  useSearchParams,
} from "react-router-dom";
import { useTranslation } from "react-i18next";
import {
  ChevronDown,
  ChevronRight,
  ClipboardList,
  Folder,
  FolderOpen,
  FolderPlus,
  GitBranch,
  MessageSquare,
  Play,
  Plus,
  Trash2,
  X,
} from "lucide-react";
import { api, ApiError } from "../../api/http";
import type {
  AgentInfo,
  MessageRecord,
  SwarmEvent,
  Workspace,
  WorkspaceRoot,
} from "../../api/types";
import { AgentDrawer } from "../../components/agent/AgentDrawer";
import { CreateWizard } from "../../components/workspace/CreateWizard";
import { ErrorBoundary } from "../../components/ErrorBoundary";
import { Welcome } from "../../components/Welcome";
import { useSwarmFeed } from "../../hooks/useSwarmFeed";
import { accentToCssVar } from "../../lib/workspace";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { cn } from "@/lib/cn";

// ── Types ──────────────────────────────────────────────────────────────

export interface WorkspaceSummary {
  /** URL slug used by `/chat/:slug`. Now = first 8 chars of the
   *  workspaces table UUID (stable, collision-free). */
  id: string;
  /** Full workspaces.id (UUID). Used by data joins (e.g. agent
   *  filtering, DELETE endpoint). */
  workspaceId: string;
  /** The workspace's cwd. */
  path: string;
  /** Human name from CreateWizard (workspaces.name). */
  name: string;
  /** Path parent for the small mono caption under the name. */
  parent: string;
  /** Accent color CSS var; comes from workspaces.accent or defaults
   *  to peach. */
  accentColor: string;
  /** Alive agents whose workspace_id points at this workspace. */
  members: AgentInfo[];
  /** Attached dependency-source roots (excludes the primary `path`).
   *  Rendered as the workspace's file-tree children in the sidebar. */
  roots: WorkspaceRoot[];
}

/** Threaded down to children via <Outlet context={...}/>. Anything a child
 *  view needs that the Shell already computed lives here so we don't run
 *  redundant fetches / subscriptions. */
export interface ShellOutletContext {
  workspace: WorkspaceSummary;
  /** Alive agents in the active workspace (= workspace.members alias). */
  activeMembers: AgentInfo[];
  /** Every alive agent across all workspaces — composer needs it to
   *  resolve cross-workspace mentions ("planner is responding…"). */
  allAliveAgents: AgentInfo[];
  /** Historical id set of agents that ever lived in this workspace
   *  (alive + killed). MessagesPanel filters by it so each workspace
   *  is a self-contained room. */
  workspaceAgentIds: string[];
  /** Latest swarm message event, or null. Child re-broadcasts. */
  liveMessage: MessageRecord | null;
  /** Latest message_read event, or null. */
  liveRead: { ids: number[]; to_agent: string; at: number } | null;
  /** Unread tally, already filtered to this workspace's senders. */
  unreadByFrom: Record<string, number>;
  /** Click → bump this counter, MessagesPanel scrolls to first unread. */
  jumpUnreadTick: number;
  /** Open the right-side AgentDrawer (writes ?agent=<id> into URL). */
  openAgent: (agentId: string) => void;
  /** Imperative refresh handle child views can call after mutations
   *  (e.g. wake-agent button → listAgents() to update spinner state). */
  refreshAgents: () => void;
}

/** Convenience hook so child views don't import the context object. */
export function useWorkspaceContext(): ShellOutletContext {
  return useOutletContext<ShellOutletContext>();
}

// ── Helpers ────────────────────────────────────────────────────────────

function splitWorkspacePath(path: string): { name: string; parent: string } {
  if (!path || path === "(no workspace)") return { name: path || "", parent: "" };
  const trimmed = path.replace(/[\\/]+$/, "");
  const idx = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  if (idx < 0) return { name: trimmed, parent: "" };
  return { name: trimmed.slice(idx + 1) || trimmed, parent: trimmed.slice(0, idx) };
}

/** Tiny coloured tag on an attached source root: blue for a code dependency,
 *  violet for a tool/utility project. Two distinct hues so the two kinds of
 *  attachment are scannable at a glance in the tree. */
function RoleChip({ role, label }: { role: string; label: string }) {
  // Four distinct tints so node kinds are scannable at a glance: the primary
  // project is the loud solid accent; peer projects soft-blue; tools violet;
  // dependencies a quiet neutral (the most common kind, kept low-key).
  const cls =
    role === "main"
      ? "bg-accent-primary text-foreground-on-accent"
      : role === "project"
        ? "bg-accent-primary-soft text-accent-primary"
        : role === "tool"
          ? "bg-accent-purple-soft text-accent-purple"
          : "bg-surface-tertiary text-foreground-secondary";
  return (
    <span
      className={cn(
        "shrink-0 rounded px-1 py-px font-caption text-[9px] font-medium uppercase tracking-wide",
        cls,
      )}
    >
      {label}
    </span>
  );
}

// ── Workspace root tree (logical, parent_id based) ─────────────────────

interface RootNode {
  root: WorkspaceRoot;
  name: string;
  parent: string;
  children: RootNode[];
  /** The synthetic node standing for the workspace's primary project (cwd).
   *  Not a workspace_roots row (no server id); never removable. */
  isMain?: boolean;
}

/** Build the logical forest shown UNDER a workspace row. The PRIMARY project
 *  (the cwd) is an explicit synthetic node first, carrying its mounted source
 *  roots (role≠"project", parent_id=null) as children — so the real project
 *  folder (e.g. `backend`) is visible and its deps clearly hang under IT, not
 *  under the workspace. Then the top-level peer projects, each recursively
 *  carrying its own mounts (parent_id = that node's id). Logical tree — a
 *  node's `path` can live anywhere; nesting follows parent_id. */
function buildWorkspaceRootForest(ws: WorkspaceSummary): RootNode[] {
  const make = (r: WorkspaceRoot): RootNode => {
    const { name, parent } = splitWorkspacePath(r.path);
    return {
      root: r,
      name,
      parent,
      children: r.id
        ? ws.roots.filter((c) => c.parent_id === r.id).map(make)
        : [],
    };
  };
  const topLevel = ws.roots.filter((r) => (r.parent_id ?? null) === null);
  const mainDeps = topLevel.filter((r) => r.role !== "project").map(make);
  const peers = topLevel.filter((r) => r.role === "project").map(make);
  const { name, parent } = splitWorkspacePath(ws.path);
  const mainNode: RootNode = {
    root: { path: ws.path, role: "project", parent_id: null },
    name,
    parent,
    children: mainDeps,
    isMain: true,
  };
  return [mainNode, ...peers];
}

/** Recursive renderer for the sidebar root tree. Display-only — all edits go
 *  through ManageRootsDialog. Peer projects render bolder (with a folder icon)
 *  and are collapsible; source mounts carry a role chip. */
function RootTree({
  nodes,
  collapsed,
  toggle,
}: {
  nodes: RootNode[];
  collapsed: Set<string>;
  toggle: (id: string) => void;
}) {
  const { t } = useTranslation();
  return (
    <>
      {nodes.map((node) => {
        const isProject = node.root.role === "project";
        const hasKids = node.children.length > 0;
        const nodeId = node.root.id ?? node.root.path;
        const open = !collapsed.has(nodeId);
        return (
          <div key={nodeId} className="flex flex-col">
            <div
              className="flex items-center gap-1 rounded px-1 py-1 hover:bg-surface-tertiary"
              title={node.root.path}
            >
              {hasKids ? (
                <button
                  type="button"
                  onClick={() => toggle(nodeId)}
                  className="flex size-4 shrink-0 items-center justify-center text-foreground-tertiary hover:text-foreground-primary"
                  aria-label={open ? t("chat.collapse") : t("chat.expand")}
                >
                  {open ? (
                    <ChevronDown className="size-3" />
                  ) : (
                    <ChevronRight className="size-3" />
                  )}
                </button>
              ) : (
                <span className="size-4 shrink-0" aria-hidden />
              )}
              {node.isMain ? (
                <FolderOpen className="size-3.5 shrink-0 text-accent-primary" />
              ) : (
                <Folder
                  className={cn(
                    "size-3.5 shrink-0",
                    isProject
                      ? "text-foreground-secondary"
                      : "text-foreground-tertiary",
                  )}
                />
              )}
              <span className="flex min-w-0 flex-1 flex-col">
                <span
                  className={cn(
                    "truncate font-mono text-[11px]",
                    isProject
                      ? "font-semibold text-foreground-primary"
                      : "text-foreground-secondary",
                  )}
                >
                  {node.name}
                </span>
                {node.parent && (
                  <span className="truncate font-mono text-[9px] leading-tight text-foreground-tertiary">
                    {node.parent}
                  </span>
                )}
              </span>
              <RoleChip
                role={node.isMain ? "main" : node.root.role}
                label={
                  node.isMain
                    ? t("chat.primaryProject")
                    : node.root.role === "project"
                      ? t("chat.roleProject")
                      : node.root.role === "tool"
                        ? t("chat.roleTool")
                        : t("chat.roleDependency")
                }
              />
            </div>
            {hasKids && open && (
              <div className="ml-[0.6rem] flex flex-col border-l border-border-subtle pl-1.5">
                <RootTree
                  nodes={node.children}
                  collapsed={collapsed}
                  toggle={toggle}
                />
              </div>
            )}
          </div>
        );
      })}
    </>
  );
}

// ── Workspaces list (left sidebar, also re-used by /chat home) ─────────

export function WorkspaceList({
  workspaces,
  activeId,
  onOpenWizard,
  onDelete,
  onRootsChanged,
}: {
  workspaces: WorkspaceSummary[];
  activeId: string | null;
  onOpenWizard: () => void;
  /** Soft-delete handler. Receives the full workspace UUID (NOT the slug)
   *  so the parent can call `DELETE /api/workspaces/:id` directly. */
  onDelete?: (workspaceId: string) => void;
  /** Called after attached roots are added/removed so the parent refetches
   *  workspaces (keeps the sidebar tree in sync). */
  onRootsChanged?: () => void;
}) {
  const { t } = useTranslation();
  // App-native delete confirm — replaces window.confirm() (which looked like
  // an OS popup, out of place in this UI, and behaves inconsistently inside
  // the Tauri shell). Holds the workspace pending deletion; null = closed.
  const [pendingDelete, setPendingDelete] = useState<WorkspaceSummary | null>(
    null,
  );
  // Workspace whose attached-source roots are being managed (Dialog open).
  const [manageRoots, setManageRoots] = useState<WorkspaceSummary | null>(null);
  // Workspace ids whose attached-source subtree is collapsed. Default is
  // expanded (a fresh id is absent from the set), so newly-attached deps are
  // visible without a click.
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set());
  const toggleCollapsed = (id: string) =>
    setCollapsed((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  return (
    <aside className="flex w-[264px] shrink-0 flex-col gap-3 border-r border-border-subtle bg-surface-secondary px-2 py-3">
      <div className="flex items-center justify-between px-2">
        <h2 className="font-heading text-xs font-semibold uppercase tracking-wider text-foreground-tertiary">
          {t("chat.workspaces")}
        </h2>
        <Tooltip>
          <TooltipTrigger asChild>
            <Button
              type="button"
              variant="ghost"
              size="icon"
              onClick={onOpenWizard}
              className="size-7 text-foreground-tertiary hover:text-foreground-primary"
            >
              <Plus className="size-4" />
            </Button>
          </TooltipTrigger>
          <TooltipContent side="bottom">{t("chat.newWorkspace")}</TooltipContent>
        </Tooltip>
      </div>
      <nav className="flex flex-col gap-0.5 overflow-y-auto">
        {workspaces.length === 0 && (
          // sidebar empty 只放一行安静提示，避免跟中间 Welcome 屏的大
          // CTA 撞车。"+ 工作空间" 入口仍在 sidebar 顶部 (heading 旁的
          // 小 + 按钮)，足够新建，不需要再画一个虚线大卡片。
          <p className="mx-3 mt-2 font-caption text-[11px] leading-relaxed text-foreground-tertiary">
            {t("welcome.sidebarEmpty")}
          </p>
        )}
        {workspaces.map((ws) => {
          const active = ws.id === activeId;
          const hasRoots = ws.roots.length > 0;
          const expanded = hasRoots && !collapsed.has(ws.id);
          return (
            <div key={ws.id} className="flex flex-col">
              {/* ── primary project row ─────────────────────────────── */}
              <div
                className={cn(
                  "group relative flex items-center rounded-md transition-colors",
                  active
                    ? "bg-accent-primary-soft"
                    : "hover:bg-surface-tertiary",
                )}
              >
                {/* disclosure chevron — only when the workspace has
                 *  attached source roots; otherwise a same-width spacer so
                 *  every workspace name lines up. */}
                {hasRoots ? (
                  <button
                    type="button"
                    onClick={() => toggleCollapsed(ws.id)}
                    className="flex size-5 shrink-0 items-center justify-center self-start rounded text-foreground-tertiary hover:text-foreground-primary"
                    style={{ marginTop: "0.4rem" }}
                    aria-label={expanded ? t("chat.collapse") : t("chat.expand")}
                    aria-expanded={expanded}
                  >
                    {expanded ? (
                      <ChevronDown className="size-3.5" />
                    ) : (
                      <ChevronRight className="size-3.5" />
                    )}
                  </button>
                ) : (
                  <span className="size-5 shrink-0" aria-hidden />
                )}
                {/* NavLink 而不是 button+navigate — 浏览器中键 / cmd+click
                 *  自然开新 tab，URL 在 hover 时显示在状态栏。
                 *  pr-8 留出 hover 删除按钮的空间。 */}
                <NavLink
                  to={`/chat/${ws.id}`}
                  title={ws.path}
                  className={cn(
                    "flex flex-1 items-center gap-2 py-1.5 pr-8 text-left",
                    active
                      ? "text-foreground-primary"
                      : "text-foreground-secondary",
                  )}
                >
                  <span
                    className="mt-1 size-2 shrink-0 self-start rounded-full"
                    style={{ background: ws.accentColor }}
                  />
                  <span className="flex min-w-0 flex-1 flex-col gap-0.5">
                    <span className="truncate font-heading text-[13px] font-semibold text-foreground-primary">
                      {ws.name}
                    </span>
                    {ws.parent && !hasRoots && (
                      // When the tree is shown, the explicit primary-project
                      // node already carries the folder + path, so the row
                      // drops the redundant parent caption.
                      <span className="truncate font-mono text-[10px] leading-tight text-foreground-tertiary">
                        {ws.parent}
                      </span>
                    )}
                  </span>
                  <span className="self-start font-caption text-[10px] font-semibold text-foreground-tertiary">
                    {ws.members.length}
                  </span>
                </NavLink>
                {/* Hover-only 管理挂载源码按钮（事后增删依赖/工具源码根）。 */}
                {onRootsChanged && (
                  <Tooltip>
                    <TooltipTrigger asChild>
                      <Button
                        type="button"
                        variant="ghost"
                        size="icon"
                        onClick={(e) => {
                          e.preventDefault();
                          e.stopPropagation();
                          setManageRoots(ws);
                        }}
                        className="absolute right-7 top-1 size-6 text-foreground-tertiary opacity-0 transition-opacity group-hover:opacity-100 hover:text-foreground-primary"
                        aria-label={t("chat.manageRoots", { name: ws.name })}
                      >
                        <FolderPlus className="size-3.5" />
                      </Button>
                    </TooltipTrigger>
                    <TooltipContent side="right">
                      {t("chat.manageRootsTooltip")}
                    </TooltipContent>
                  </Tooltip>
                )}
                {/* Hover-only 删除按钮。软删除：workspace 卡片消失，但 PTY /
                 *  agent 保留（用户可能仍在用某个 pane）。第一次点带 confirm，
                 *  防误删。 */}
                {onDelete && (
                  <Tooltip>
                    <TooltipTrigger asChild>
                      <Button
                        type="button"
                        variant="ghost"
                        size="icon"
                        onClick={(e) => {
                          e.preventDefault();
                          e.stopPropagation();
                          setPendingDelete(ws);
                        }}
                        className="absolute right-1 top-1 size-6 text-foreground-tertiary opacity-0 transition-opacity group-hover:opacity-100 hover:text-state-danger"
                        aria-label={t("chat.deleteWorkspace", { name: ws.name })}
                      >
                        <Trash2 className="size-3.5" />
                      </Button>
                    </TooltipTrigger>
                    <TooltipContent side="right">
                      {t("chat.deleteWorkspaceTooltip")}
                    </TooltipContent>
                  </Tooltip>
                )}
              </div>

              {/* ── logical tree: deps under the primary project + peer
               *  projects + arbitrary nesting (parent_id based) ────────── */}
              {expanded && (
                <div className="ml-[0.9rem] flex flex-col border-l border-border-subtle pl-1.5 pt-0.5">
                  <RootTree
                    nodes={buildWorkspaceRootForest(ws)}
                    collapsed={collapsed}
                    toggle={toggleCollapsed}
                  />
                </div>
              )}
            </div>
          );
        })}
      </nav>
      {/* 底部主 CTA = 新建工作空间。点开就是创建向导（没有 spell 选择器了
       *  —— Magentic-One 模型下每个 workspace 就是一个 orchestrator 临时
       *  派 worker），所以不再叫"运行配方"，名实相符直接叫"新建工作空间"。
       *  空状态下 hide：sidebar 顶部 heading 旁的小 + 已经够建第一个。 */}
      {workspaces.length > 0 && (
        <div className="mt-auto px-2 pt-3">
          <Button onClick={onOpenWizard} className="w-full">
            <Plus className="size-4" />
            {t("chat.newWorkspace")}
          </Button>
        </div>
      )}

      {/* Delete confirm — app-native Dialog instead of window.confirm(). */}
      <Dialog
        open={pendingDelete != null}
        onOpenChange={(next) => {
          if (!next) setPendingDelete(null);
        }}
      >
        <DialogContent showCloseButton={false} className="sm:max-w-md">
          <DialogHeader>
            <DialogTitle>
              {t("chat.deleteConfirmTitle", { name: pendingDelete?.name ?? "" })}
            </DialogTitle>
            <DialogDescription>
              {t("chat.deleteConfirmBody")}
            </DialogDescription>
          </DialogHeader>
          <div className="flex justify-end gap-2 pt-2">
            <Button variant="outline" onClick={() => setPendingDelete(null)}>
              {t("common.cancel")}
            </Button>
            <Button
              variant="destructive"
              onClick={() => {
                const target = pendingDelete;
                setPendingDelete(null);
                if (target) onDelete?.(target.workspaceId);
              }}
            >
              <Trash2 className="size-3.5" />
              {t("common.delete")}
            </Button>
          </div>
        </DialogContent>
      </Dialog>

      {/* Manage attached source roots — add/remove deps post-create. */}
      {manageRoots && (
        <ManageRootsDialog
          workspace={manageRoots}
          onClose={() => setManageRoots(null)}
          onChanged={() => onRootsChanged?.()}
        />
      )}
    </aside>
  );
}

/** Add / remove a workspace's attached dependency-source roots after
 *  creation. Lists current roots (with remove), shows manifest-derived
 *  suggestions as one-click chips, and a manual path + role add row. Each
 *  mutation optimistically updates the local list AND calls `onChanged` so
 *  the sidebar tree refetches. */
function ManageRootsDialog({
  workspace,
  onClose,
  onChanged,
}: {
  workspace: WorkspaceSummary;
  onClose: () => void;
  onChanged: () => void;
}) {
  const { t } = useTranslation();
  const [roots, setRoots] = useState<WorkspaceRoot[]>(workspace.roots);
  const [suggestions, setSuggestions] = useState<WorkspaceRoot[]>([]);
  const [path, setPath] = useState("");
  // What to add: a top-level peer "project", or a "dependency"/"tool" source
  // mount placed under a chosen parent project.
  const [role, setRole] = useState("dependency");
  // Parent project for a source mount: "" = the primary project (cwd), else a
  // peer project's id. Ignored when role==="project" (peers are top-level).
  const [parentId, setParentId] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Projects that can be a mount parent: the primary (cwd) + every peer
  // project root. Value "" denotes the primary (parent_id=null on the wire).
  const projectOptions = useMemo(
    () => [
      // Primary = the cwd's folder name, matching the tree's primary node.
      { id: "", name: splitWorkspacePath(workspace.path).name },
      ...roots
        .filter((r) => r.role === "project" && r.id)
        .map((r) => ({ id: r.id!, name: splitWorkspacePath(r.path).name })),
    ],
    [roots, workspace.path],
  );
  const parentPath =
    parentId === ""
      ? workspace.path
      : (roots.find((r) => r.id === parentId)?.path ?? workspace.path);

  // Suggestions are scoped to the chosen parent project's manifests; refetch
  // when the parent changes. Skipped while adding a peer project.
  useEffect(() => {
    if (role === "project") {
      setSuggestions([]);
      return;
    }
    let alive = true;
    api
      .rootSuggestions(workspace.workspaceId, parentPath)
      .then((s) => {
        if (alive)
          setSuggestions(s.filter((x) => !roots.some((r) => r.path === x.path)));
      })
      .catch(() => {});
    return () => {
      alive = false;
    };
    // roots/parentPath read but not deps — only refetch on parent/role change.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [workspace.workspaceId, parentId, role]);

  const add = async (
    p: string,
    r: string,
    pid: string | null,
    label?: string | null,
  ) => {
    const trimmed = p.trim();
    if (!trimmed || busy) return;
    setBusy(true);
    setError(null);
    try {
      const added = await api.addWorkspaceRoot(workspace.workspaceId, {
        path: trimmed,
        role: r,
        label: label ?? undefined,
        parent_id: pid ?? undefined,
      });
      setRoots((prev) => [...prev, added]);
      setSuggestions((prev) => prev.filter((s) => s.path !== added.path));
      setPath("");
      onChanged();
    } catch (e) {
      setError(e instanceof ApiError ? e.detail : (e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  // Remove a node + (optimistically) its whole subtree; backend cascades.
  const remove = async (id: string) => {
    if (busy || !id) return;
    const doomed = new Set([id]);
    for (let grew = true; grew; ) {
      grew = false;
      for (const r of roots) {
        if (r.id && r.parent_id && doomed.has(r.parent_id) && !doomed.has(r.id)) {
          doomed.add(r.id);
          grew = true;
        }
      }
    }
    setBusy(true);
    setError(null);
    try {
      await api.deleteWorkspaceRoot(workspace.workspaceId, id);
      setRoots((prev) => prev.filter((x) => !(x.id && doomed.has(x.id))));
      onChanged();
    } catch (e) {
      setError(e instanceof ApiError ? e.detail : (e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  const forest = buildWorkspaceRootForest({ ...workspace, roots });
  const renderNodes = (nodes: RootNode[], depth: number): ReactNode =>
    nodes.map((node) => (
      <div key={node.root.id ?? node.root.path} className="flex flex-col">
        <div
          className="flex items-center gap-2 rounded-md px-2 py-1.5 hover:bg-surface-tertiary"
          style={{ marginLeft: depth * 14 }}
          title={node.root.path}
        >
          {node.isMain ? (
            <FolderOpen className="size-3.5 shrink-0 text-accent-primary" />
          ) : (
            <Folder
              className={cn(
                "size-3.5 shrink-0",
                node.root.role === "project"
                  ? "text-foreground-secondary"
                  : "text-foreground-tertiary",
              )}
            />
          )}
          <span className="flex min-w-0 flex-1 flex-col">
            <span
              className={cn(
                "truncate font-mono text-[12px]",
                node.root.role === "project"
                  ? "font-semibold text-foreground-primary"
                  : "text-foreground-secondary",
              )}
            >
              {node.name}
            </span>
            <span className="truncate font-mono text-[10px] leading-tight text-foreground-tertiary">
              {node.parent}
            </span>
          </span>
          <RoleChip
            role={node.isMain ? "main" : node.root.role}
            label={
              node.isMain
                ? t("chat.primaryProject")
                : node.root.role === "project"
                  ? t("chat.roleProject")
                  : node.root.role === "tool"
                    ? t("chat.roleTool")
                    : t("chat.roleDependency")
            }
          />
          {/* primary project is the workspace cwd — not removable here. */}
          {!node.isMain && (
            <Button
              type="button"
              variant="ghost"
              size="icon"
              disabled={busy}
              onClick={() => node.root.id && remove(node.root.id)}
              className="size-6 shrink-0 text-foreground-tertiary hover:text-state-danger"
              aria-label={t("common.delete")}
            >
              <X className="size-3.5" />
            </Button>
          )}
        </div>
        {node.children.length > 0 && renderNodes(node.children, depth + 1)}
      </div>
    ));

  return (
    <Dialog
      open
      onOpenChange={(next) => {
        if (!next) onClose();
      }}
    >
      <DialogContent className="sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>
            {t("chat.manageRoots", { name: workspace.name })}
          </DialogTitle>
          <DialogDescription>{t("chat.manageRootsHint")}</DialogDescription>
        </DialogHeader>

        {/* current tree: primary project node (non-removable) + peer
         *  projects + their mounts. The primary is the first forest node. */}
        <div className="flex flex-col gap-0.5">
          {renderNodes(forest, 0)}
          {roots.length === 0 && (
            <p className="px-2 py-1 font-caption text-[11px] text-foreground-tertiary">
              {t("chat.noRoots")}
            </p>
          )}
        </div>

        {/* manifest-derived suggestions (scoped to the chosen parent) */}
        {role !== "project" && suggestions.length > 0 && (
          <div className="flex flex-col gap-1.5">
            <span className="px-1 font-caption text-[10px] font-semibold uppercase tracking-wide text-foreground-tertiary">
              {t("chat.suggestedRoots")}
            </span>
            <div className="flex flex-wrap gap-1.5">
              {suggestions.map((s) => {
                const { name: base } = splitWorkspacePath(s.path);
                return (
                  <button
                    key={s.path}
                    type="button"
                    disabled={busy}
                    onClick={() =>
                      add(s.path, role, parentId || null, s.label)
                    }
                    title={s.path}
                    className="flex items-center gap-1 rounded-full border border-border-subtle bg-surface-secondary px-2 py-0.5 font-mono text-[11px] text-foreground-secondary transition-colors hover:bg-surface-tertiary disabled:opacity-50"
                  >
                    <Plus className="size-3" />
                    {base}
                  </button>
                );
              })}
            </div>
          </div>
        )}

        {/* add row: type (+ parent project when a source) + path */}
        <div className="flex flex-col gap-2 border-t border-border-subtle pt-3">
          <div className="flex items-center gap-2">
            <select
              value={role}
              onChange={(e) => setRole(e.target.value)}
              className="h-8 shrink-0 rounded-md border border-border-subtle bg-surface-primary px-2 text-xs text-foreground-secondary focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-accent-primary"
            >
              <option value="project">{t("chat.roleProject")}</option>
              <option value="dependency">{t("wizard.roleDependency")}</option>
              <option value="tool">{t("wizard.roleTool")}</option>
            </select>
            {role !== "project" && (
              <>
                <span className="shrink-0 font-caption text-[11px] text-foreground-tertiary">
                  {t("chat.mountUnder")}
                </span>
                <select
                  value={parentId}
                  onChange={(e) => setParentId(e.target.value)}
                  className="h-8 min-w-0 flex-1 rounded-md border border-border-subtle bg-surface-primary px-2 text-xs text-foreground-secondary focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-accent-primary"
                >
                  {projectOptions.map((p) => (
                    <option key={p.id} value={p.id}>
                      {p.name}
                    </option>
                  ))}
                </select>
              </>
            )}
          </div>
          <div className="flex items-center gap-2">
            <Input
              value={path}
              onChange={(e) => setPath(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter")
                  add(path, role, role === "project" ? null : parentId || null);
              }}
              placeholder={t("chat.rootPathPlaceholder")}
              className="h-8 flex-1 font-mono text-xs"
            />
            <Button
              type="button"
              size="sm"
              disabled={busy || !path.trim()}
              onClick={() =>
                add(path, role, role === "project" ? null : parentId || null)
              }
              className="h-8"
            >
              {t("chat.addRoot")}
            </Button>
          </div>
        </div>
        {error && (
          <p className="font-caption text-[11px] text-state-danger">{error}</p>
        )}
      </DialogContent>
    </Dialog>
  );
}

// ── Channel header (workspace name + path + unread + copy) ─────────────

// ── View toolbar (tabs + workspace actions) ────────────────────────────
//
// Replaces the old 2-row ChannelHeader. workspace 身份 (name / path /
// accent) 都在左侧 sidebar 已经显示了，header 重复一遍是浪费 64px 垂直
// 空间。这里把真正不重复的 4 个 action (LIVE / agent count / 未读跳转
// / 复制路径) 合并到 tab bar 末端，单行高度 ~36px。Slack / Linear /
// Discord 都用这种风格。

interface TabDef {
  to: string;
  labelKey: string;
  icon: typeof MessageSquare;
  // ⌘1 / ⌘2 / ⌘3 / ⌘4 shortcut (1-based). Shell registers a global
  // keydown handler that maps Meta/Ctrl + digit → navigate(tab.to).
  shortcut: number;
}

function buildTabs(wsId: string): TabDef[] {
  return [
    { to: `/chat/${wsId}`, labelKey: "chat.tabs.chat", icon: MessageSquare, shortcut: 1 },
    { to: `/chat/${wsId}/dag`, labelKey: "chat.tabs.dag", icon: GitBranch, shortcut: 2 },
    { to: `/chat/${wsId}/ledger`, labelKey: "chat.tabs.ledger", icon: ClipboardList, shortcut: 3 },
    { to: `/chat/${wsId}/replays`, labelKey: "chat.tabs.replays", icon: Play, shortcut: 4 },
  ];
}

function WorkspaceToolbar({
  workspace,
  agentCount,
  totalUnread,
  onJumpUnread,
}: {
  workspace: WorkspaceSummary;
  agentCount: number;
  totalUnread: number;
  onJumpUnread: () => void;
}) {
  const { t } = useTranslation();
  const tabs = buildTabs(workspace.id);
  const isMac =
    typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform);
  const modKey = isMac ? "⌘" : "Ctrl";

  return (
    <nav className="flex h-10 shrink-0 items-center gap-1 border-b border-border-subtle px-3">
      {tabs.map((tab) => {
        const Icon = tab.icon;
        return (
          <NavLink
            key={tab.to}
            to={tab.to}
            // index route 必须 end，否则 /chat/:wsId 在 /chat/:wsId/dag 时
            // 也算 active。其他 tab 路径足够独特，end 无所谓但保持一致。
            end
            className={({ isActive }) =>
              cn(
                "relative flex items-center gap-1.5 px-3 py-2 text-xs transition-colors",
                isActive
                  ? "text-foreground-primary after:absolute after:inset-x-0 after:-bottom-px after:h-0.5 after:bg-accent-primary"
                  : "text-foreground-secondary hover:text-foreground-primary",
              )
            }
            title={`${t(tab.labelKey)}  ${modKey}${tab.shortcut}`}
          >
            <Icon className="size-3.5" />
            {t(tab.labelKey)}
          </NavLink>
        );
      })}

      <span className="flex-1" />

      {/* workspace actions — 全部 shrink-0 + 小尺寸，跟 tab 行高保持一致 */}
      {agentCount > 0 && (
        <Tooltip>
          <TooltipTrigger asChild>
            <span
              className="flex h-5 shrink-0 items-center gap-1 rounded-full bg-status-running-soft px-2 font-caption text-[10px] font-semibold uppercase tracking-wide text-status-running"
              title={t("chat.memberCount", { count: agentCount })}
            >
              <span
                className="size-1.5 rounded-full bg-status-running"
                aria-hidden
              />
              {t("common.live")}
            </span>
          </TooltipTrigger>
          <TooltipContent side="bottom">
            {t("chat.memberCount", { count: agentCount })}
          </TooltipContent>
        </Tooltip>
      )}

      {totalUnread > 0 && (
        <button
          type="button"
          onClick={onJumpUnread}
          title={t("chat.jumpUnread")}
          className="flex shrink-0 cursor-pointer items-center"
        >
          <Badge className="rounded-full px-2 py-0.5 text-[10px] transition-transform hover:scale-105">
            {t("chat.unread", { count: totalUnread })}
          </Badge>
        </button>
      )}
    </nav>
  );
}

// ── View transition wrapper ────────────────────────────────────────────

/** 60-80ms cross-fade on Outlet child swap. Long enough to feel soft, short
 *  enough that quick tab-juggling doesn't stack delays. The `key` ties the
 *  fade to the location, so navigating to the same path doesn't replay. */
function ViewTransition({ children }: { children: ReactNode }) {
  const location = useLocation();
  return (
    <div
      key={location.pathname}
      className="flex h-full min-h-0 flex-1 flex-col animate-in fade-in duration-75"
    >
      {children}
    </div>
  );
}

// ── Shell ──────────────────────────────────────────────────────────────

export default function WorkspaceShell() {
  const { t } = useTranslation();
  const { wsId } = useParams<{ wsId: string }>();
  const navigate = useNavigate();
  const location = useLocation();
  const [searchParams, setSearchParams] = useSearchParams();

  // Right-side AgentDrawer state lives in URL (?agent=<id>) so the user
  // can deep-link / refresh. Shell owns it so any view can open it.
  const drawerAgentId = searchParams.get("agent");
  const openAgent = useCallback(
    (id: string) => {
      setSearchParams((prev) => {
        const next = new URLSearchParams(prev);
        next.set("agent", id);
        return next;
      });
    },
    [setSearchParams],
  );
  const closeAgent = useCallback(() => {
    setSearchParams((prev) => {
      const next = new URLSearchParams(prev);
      next.delete("agent");
      return next;
    });
  }, [setSearchParams]);

  // CreateWizard opens from sidebar + ⌘K (window event).
  const [wizardOpen, setWizardOpen] = useState(false);
  useEffect(() => {
    const onOpen = () => setWizardOpen(true);
    window.addEventListener("flockmux:open-wizard", onOpen as EventListener);
    return () =>
      window.removeEventListener("flockmux:open-wizard", onOpen as EventListener);
  }, []);

  // ── Shared state (was per-route before, now per-Shell) ──────────────
  const [agents, setAgents] = useState<AgentInfo[]>([]);
  const [workspaceRows, setWorkspaceRows] = useState<Workspace[]>([]);
  const [liveMessage, setLiveMessage] = useState<MessageRecord | null>(null);
  const [liveRead, setLiveRead] = useState<
    { ids: number[]; to_agent: string; at: number } | null
  >(null);
  const [unreadByFrom, setUnreadByFrom] = useState<Record<string, number>>({});
  const [jumpUnreadTick, setJumpUnreadTick] = useState(0);
  const idToFromRef = useRef<Map<number, string>>(new Map());

  const refreshWorkspaces = useCallback(async () => {
    try {
      const items = await api.listWorkspaces();
      setWorkspaceRows(items);
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn("listWorkspaces failed", err);
    }
  }, []);

  const refreshAgents = useCallback(async () => {
    try {
      const items = await api.listAgents();
      setAgents(items);
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
      setUnreadByFrom(counts);
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
  // Agents are grouped onto these via `agent.workspace_id`. The old
  // "group by cwd path" trick is gone — that was the bug.
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
      };
    });
  }, [workspaceRows, agents]);

  const activeWs = useMemo(
    () => workspaces.find((w) => w.id === wsId) ?? null,
    [workspaces, wsId],
  );

  // ── Per-workspace derivations passed down via OutletContext ─────────
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

  const activeWorkspaceUnread = useMemo(() => {
    if (!activeWs) return {} as Record<string, number>;
    const wsSet = new Set(workspaceAgentIds);
    return Object.fromEntries(
      Object.entries(unreadByFrom).filter(([from]) => wsSet.has(from)),
    );
  }, [unreadByFrom, activeWs, workspaceAgentIds]);
  const totalUnread = Object.values(activeWorkspaceUnread).reduce(
    (a, b) => a + b,
    0,
  );

  // ── Soft-delete a workspace ────────────────────────────────────────
  const handleDeleteWorkspace = useCallback(
    async (workspaceId: string) => {
      // Kill any live agents belonging to this workspace before deleting
      // the row, otherwise their PTYs survive and keep burning tokens
      // with no UI handle to address them. Per-agent failure is logged
      // but doesn't abort the batch (a half-dead PTY shouldn't block
      // the user from removing the workspace).
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
        return;
      }
      // Optimistically drop it from local state — next listWorkspaces
      // refresh would catch it anyway but UI shouldn't lag a roundtrip.
      const remaining = workspaceRows.filter((w) => w.id !== workspaceId);
      setWorkspaceRows(remaining);
      // If we just deleted the active workspace, navigate to the first
      // remaining one or back to /chat splash.
      if (activeWs?.workspaceId === workspaceId) {
        const next = remaining[0];
        navigate(next ? `/chat/${next.slug}` : "/chat", { replace: true });
      }
    },
    [workspaceRows, activeWs, navigate],
  );

  // ── ⌘1-4 global shortcut ───────────────────────────────────────────
  useEffect(() => {
    if (!activeWs) return;
    const tabs = buildTabs(activeWs.id);
    const onKey = (e: KeyboardEvent) => {
      if (!(e.metaKey || e.ctrlKey)) return;
      if (e.target instanceof HTMLElement) {
        const tag = e.target.tagName;
        // 别和 IME / 表单组合键冲突 — 输入框里 ⌘1 仍走原生 (浏览器切 tab)。
        if (tag === "INPUT" || tag === "TEXTAREA" || e.target.isContentEditable) {
          return;
        }
      }
      const n = Number.parseInt(e.key, 10);
      if (!Number.isInteger(n) || n < 1 || n > tabs.length) return;
      e.preventDefault();
      navigate(tabs[n - 1].to);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [activeWs, navigate]);

  // ── Redirect a stale / unknown wsId to the first workspace ──────────
  // MUST be an effect, not render-phase. Calling navigate() while rendering
  // triggers React's "Cannot update a component (BrowserRouter) while
  // rendering a different component (WorkspaceShell)" warning and is unsafe
  // under React 18 concurrent rendering. This fires when a bookmark / refresh
  // points at a workspace that was since deleted while others still exist.
  useEffect(() => {
    if (
      !activeWs &&
      workspaces.length > 0 &&
      wsId &&
      !workspaces.some((w) => w.id === wsId)
    ) {
      navigate(`/chat/${workspaces[0].id}`, { replace: true });
    }
  }, [activeWs, workspaces, wsId, navigate]);

  // ── Render ─────────────────────────────────────────────────────────
  if (!activeWs) {
    // wsId 在 URL 但 listAgents 还没回 / 已经 evicted。渲染 sidebar +
    // "找不到工作空间" 提示；真正的跳转由上面的 useEffect 负责（render
    // 阶段不能 navigate）。
    return (
      <TooltipProvider delayDuration={300}>
        <div className="flex h-full min-h-0">
          <WorkspaceList
            workspaces={workspaces}
            activeId={wsId ?? null}
            onOpenWizard={() => setWizardOpen(true)}
            onDelete={handleDeleteWorkspace}
            onRootsChanged={refreshWorkspaces}
          />
          {workspaces.length === 0 ? (
            // 完全空：展示 Welcome 屏，跟 /chat 主入口体验一致。
            <Welcome onCreateWorkspace={() => setWizardOpen(true)} />
          ) : (
            // 有别的 ws 但 URL 指的这个不存在 — 给个安静提示就行，重定向
            // 已经在 useEffect 里发车。
            <section className="flex min-w-0 flex-1 flex-col items-center justify-center gap-3 bg-surface-primary text-foreground-tertiary">
              <FolderOpen className="size-10 opacity-40" />
              <p className="font-caption text-sm">
                {t("chat.selectWorkspace")}
              </p>
            </section>
          )}
          <CreateWizard
            open={wizardOpen}
            onClose={() => setWizardOpen(false)}
            onCreated={(ws) => {
              refreshAgents();
              // Await the workspace refetch BEFORE navigating — otherwise the
              // new slug isn't in `workspaces` yet and the not-found redirect
              // effect bounces us straight back to the previous workspace.
              void refreshWorkspaces().then(() => {
                if (ws) navigate(`/chat/${ws.slug}`);
              });
            }}
          />
        </div>
      </TooltipProvider>
    );
  }

  const ctx: ShellOutletContext = {
    workspace: activeWs,
    activeMembers: activeWs.members,
    allAliveAgents,
    workspaceAgentIds,
    liveMessage,
    liveRead,
    unreadByFrom: activeWorkspaceUnread,
    jumpUnreadTick,
    openAgent,
    refreshAgents,
  };

  return (
    <TooltipProvider delayDuration={300}>
      <div className="flex h-full min-h-0">
        <WorkspaceList
          workspaces={workspaces}
          activeId={activeWs.id}
          onOpenWizard={() => setWizardOpen(true)}
          onDelete={handleDeleteWorkspace}
          onRootsChanged={refreshWorkspaces}
        />
        <section className="flex min-w-0 flex-1 flex-col bg-surface-primary">
          <WorkspaceToolbar
            workspace={activeWs}
            agentCount={activeWs.members.length}
            totalUnread={totalUnread}
            onJumpUnread={() => setJumpUnreadTick((v) => v + 1)}
          />
          <ViewTransition>
            {/* View-level boundary: a crash in one tab (malformed ledger
                markdown, ReactFlow state, …) shows a contained fallback while
                the sidebar + tab bar stay intact. Keyed by wsId+view so a
                tab switch clears a held error. */}
            <ErrorBoundary resetKey={`${activeWs.id}:${location.pathname}`}>
              <Outlet context={ctx} />
            </ErrorBoundary>
          </ViewTransition>
        </section>

        {drawerAgentId && (
          <AgentDrawer agentId={drawerAgentId} onClose={closeAgent} />
        )}
        <CreateWizard
          open={wizardOpen}
          onClose={() => setWizardOpen(false)}
          onCreated={(ws) => {
            refreshAgents();
            // Await the workspace refetch BEFORE navigating — otherwise the
            // new slug isn't in `workspaces` yet and the not-found redirect
            // effect bounces us straight back to the previous workspace.
            void refreshWorkspaces().then(() => {
              if (ws) navigate(`/chat/${ws.slug}`);
            });
          }}
        />
      </div>
    </TooltipProvider>
  );
}
