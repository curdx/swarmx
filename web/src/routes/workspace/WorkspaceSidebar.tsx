/**
 * Workspace sidebar: the left-rail workspace list + its logical source-root
 * tree + the "manage attached roots" CRUD dialog. Extracted from Shell.tsx
 * (which had grown into a 1400-line god-file) so the layout route is a data
 * orchestrator and the presentational sidebar lives on its own. Re-used by the
 * `/chat` Home page too.
 *
 * Everything here is props-driven — it reads no Shell state directly; mutations
 * go out through `onDelete` / `onRootsChanged` callbacks so the owner refetches.
 */

import { useEffect, useMemo, useState, type ReactNode } from "react";
import { NavLink } from "react-router-dom";
import { useTranslation } from "react-i18next";
import {
  AlertTriangle,
  ChevronDown,
  ChevronRight,
  Folder,
  FolderOpen,
  FolderPlus,
  GitBranch,
  Hash,
  Home,
  Loader2,
  Plus,
  Trash2,
  Unlink,
  X,
} from "lucide-react";
import { api, ApiError } from "../../api/http";
import type { ThreadInfo, WorkspaceRoot } from "../../api/types";
import { splitWorkspacePath } from "../../lib/workspace";
import { directionBase } from "../../lib/thread";
import type { WorkspaceSummary } from "./types";
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
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { cn } from "@/lib/cn";

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

/** Live git-branch caption for a work repo (the cwd + peer projects) and the
 *  main direction. Front-loads branch identity the way GitButler / worktree
 *  tools do, and — by showing each peer repo's own branch — lets the multi-repo
 *  roots read as equals instead of a "primary vs the rest" hierarchy. Renders
 *  nothing when there's no branch (non-git dir / detached HEAD). */
function BranchCaption({ branch }: { branch?: string | null }) {
  if (!branch) return null;
  return (
    <span
      className="flex items-center gap-1 truncate font-mono text-[9px] leading-tight text-foreground-tertiary"
      title={branch}
    >
      <GitBranch className="size-2.5 shrink-0" />
      <span className="truncate">{branch}</span>
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
    // Carry the cwd's live branch so the branch caption renders through the same
    // path as peers — the cwd reads as just another repo (+ an "AI home" mark).
    root: { path: ws.path, role: "project", parent_id: null, branch: ws.cwdBranch },
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
                <span className="flex min-w-0 items-center gap-1">
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
                  {/* cwd is where the AI's terminal opens — a functional marker,
                      not a "primary" rank, so the multi-repo roots stay peers. */}
                  {node.isMain && (
                    <span
                      className="flex shrink-0 items-center text-accent-primary"
                      title={t("chat.agentHome")}
                      aria-label={t("chat.agentHome")}
                    >
                      <Home className="size-2.5" />
                    </span>
                  )}
                </span>
                {/* Work repos (cwd + peers) front-load their live branch; mounts
                    keep their path caption. */}
                {isProject && node.root.branch ? (
                  <BranchCaption branch={node.root.branch} />
                ) : (
                  node.parent && (
                    <span className="truncate font-mono text-[9px] leading-tight text-foreground-tertiary">
                      {node.parent}
                    </span>
                  )
                )}
              </span>
              {/* Role chip only on source mounts (deps/tools) — they're
                  read-only references, not branches you work on. */}
              {!isProject && (
                <RoleChip
                  role={node.root.role}
                  label={
                    node.root.role === "tool"
                      ? t("chat.roleTool")
                      : t("chat.roleDependency")
                  }
                />
              )}
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
  activeThreadSlug,
  onOpenWizard,
  onDelete,
  onRootsChanged,
  onNewDirection,
  onDeleteThread,
}: {
  workspaces: WorkspaceSummary[];
  activeId: string | null;
  /** Slug of the active direction in the active workspace (`main` default).
   *  Highlights the current direction row in the active workspace's subtree. */
  activeThreadSlug?: string;
  onOpenWizard: () => void;
  /** Soft-delete handler. Receives the full workspace UUID (NOT the slug)
   *  so the parent can call `DELETE /api/workspaces/:id` directly. */
  onDelete?: (workspaceId: string) => void;
  /** Called after attached roots are added/removed so the parent refetches
   *  workspaces (keeps the sidebar tree in sync). */
  onRootsChanged?: () => void;
  /** Open a new direction in this workspace (create + navigate + launch the
   *  orchestrator). Owner (Shell) does the create/nav/spawn. `name` is the
   *  optional user-chosen direction name (blank → orchestrator auto-names it). */
  onNewDirection?: (ws: WorkspaceSummary, name?: string) => void;
  /** Delete a direction (server kills its live agents first). */
  onDeleteThread?: (ws: WorkspaceSummary, threadId: string) => void;
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
  // Direction (thread) pending deletion → confirm dialog (server kills its
  // live agents first, so this is a real action worth confirming).
  const [pendingDeleteThread, setPendingDeleteThread] = useState<{
    ws: WorkspaceSummary;
    thread: ThreadInfo;
  } | null>(null);
  // New-direction dialog target (null = closed) + the in-progress name. Opening
  // a direction spawns a real orchestrator process, so it gets a name + confirm
  // step instead of firing on a single (mis)click. Also the place we warn when
  // the workspace cwd isn't a git repo (directions can't be isolated then).
  const [newDirFor, setNewDirFor] = useState<WorkspaceSummary | null>(null);
  const [newDirName, setNewDirName] = useState("");
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

              {/* ── directions (threads): the active workspace's parallel
               *  lines of work. Shown only for the active workspace to keep
               *  the rail calm; the list itself only when there's >1 (a lone
               *  "main" row would be noise — you're already in it). The
               *  "+ new direction" affordance is always offered. ───────── */}
              {active && (
                <div className="ml-[0.9rem] mt-0.5 flex flex-col gap-px border-l border-border-subtle pl-1.5">
                  {ws.threads.length > 1 &&
                    ws.threads.map((th) => {
                      const isMain = th.slug === "main";
                      const thActive = (activeThreadSlug ?? "main") === th.slug;
                      const isolated = th.isolation === "worktree";
                      // Isolation was attempted but failed — sharing the main
                      // cwd. Signal it so the user doesn't assume isolation.
                      const degraded = th.isolation === "degraded";
                      const preparing = th.state === "preparing";
                      // The main direction runs in the cwd, so its branch is the
                      // workspace cwd branch; isolated directions carry their own.
                      const branch = isMain ? ws.cwdBranch : th.branch;
                      // Don't surface the raw `t-xxxxxx` placeholder slug — until
                      // the orchestrator names it (swarm_name_thread), it's just
                      // an unnamed direction.
                      const label =
                        th.name?.trim() ||
                        (isMain
                          ? t("chat.mainDirection")
                          : th.slug.startsWith("t-")
                            ? t("chat.directionUnnamed")
                            : th.slug);
                      return (
                        <div
                          key={th.id}
                          className={cn(
                            "group/dir relative flex items-center rounded-md transition-colors",
                            thActive
                              ? "bg-accent-primary-soft"
                              : "hover:bg-surface-tertiary",
                          )}
                        >
                          <NavLink
                            to={directionBase(ws.id, th.slug)}
                            title={
                              isolated && th.cwd
                                ? th.cwd
                                : degraded
                                  ? t("chat.directionDegraded")
                                  : undefined
                            }
                            className={cn(
                              "flex flex-1 items-center gap-1.5 py-1 pl-1 pr-6 text-[12px]",
                              thActive
                                ? "text-foreground-primary"
                                : "text-foreground-secondary",
                            )}
                          >
                            {preparing ? (
                              <Loader2 className="size-3 shrink-0 animate-spin text-foreground-tertiary" />
                            ) : isolated ? (
                              <GitBranch className="size-3 shrink-0 text-accent-purple" />
                            ) : degraded ? (
                              <Unlink className="size-3 shrink-0 text-state-warning" />
                            ) : (
                              <Hash className="size-3 shrink-0 text-foreground-tertiary" />
                            )}
                            <span className="flex min-w-0 flex-1 flex-col">
                              <span className="truncate">{label}</span>
                              <BranchCaption branch={branch} />
                            </span>
                            {preparing && (
                              <span className="ml-auto shrink-0 font-caption text-[9px] text-foreground-tertiary">
                                {t("chat.directionPreparing")}
                              </span>
                            )}
                          </NavLink>
                          {!isMain && onDeleteThread && !preparing && (
                            <button
                              type="button"
                              onClick={(e) => {
                                e.preventDefault();
                                e.stopPropagation();
                                setPendingDeleteThread({ ws, thread: th });
                              }}
                              className="absolute right-1 top-1/2 size-5 -translate-y-1/2 text-foreground-tertiary opacity-0 transition-opacity group-hover/dir:opacity-100 hover:text-state-danger"
                              aria-label={t("chat.deleteDirection", { name: label })}
                            >
                              <X className="size-3" />
                            </button>
                          )}
                        </div>
                      );
                    })}
                  {onNewDirection && (
                    <Tooltip>
                      <TooltipTrigger asChild>
                        <button
                          type="button"
                          onClick={() => {
                            setNewDirName("");
                            setNewDirFor(ws);
                          }}
                          className="flex items-center gap-1.5 rounded-md py-1 pl-1 pr-2 text-[12px] text-foreground-tertiary hover:bg-surface-tertiary hover:text-foreground-primary"
                        >
                          <Plus className="size-3 shrink-0" />
                          <span>{t("chat.newDirection")}</span>
                        </button>
                      </TooltipTrigger>
                      <TooltipContent side="right">
                        {t("chat.newDirectionTooltip")}
                      </TooltipContent>
                    </Tooltip>
                  )}
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

      {/* Delete-direction confirm — this kills the direction's live agents
       *  server-side, so it gets the same app-native confirm as workspace
       *  delete. */}
      <Dialog
        open={pendingDeleteThread != null}
        onOpenChange={(next) => {
          if (!next) setPendingDeleteThread(null);
        }}
      >
        <DialogContent showCloseButton={false} className="sm:max-w-md">
          <DialogHeader>
            <DialogTitle>
              {t("chat.directionDeleteConfirmTitle", {
                name:
                  pendingDeleteThread?.thread.name?.trim() ||
                  pendingDeleteThread?.thread.slug ||
                  "",
              })}
            </DialogTitle>
            <DialogDescription>
              {t("chat.directionDeleteConfirmBody")}
            </DialogDescription>
          </DialogHeader>
          <div className="flex justify-end gap-2 pt-2">
            <Button
              variant="outline"
              onClick={() => setPendingDeleteThread(null)}
            >
              {t("common.cancel")}
            </Button>
            <Button
              variant="destructive"
              onClick={() => {
                const target = pendingDeleteThread;
                setPendingDeleteThread(null);
                if (target)
                  onDeleteThread?.(target.ws, target.thread.id);
              }}
            >
              <Trash2 className="size-3.5" />
              {t("common.delete")}
            </Button>
          </div>
        </DialogContent>
      </Dialog>

      {/* New-direction dialog — opening a direction spawns a real orchestrator
       *  process, so it gets a name + confirm step instead of firing on one
       *  (mis)click. Also warns when the cwd isn't a git repo: directions then
       *  share the same files (no worktree isolation) and clobber each other. */}
      <Dialog
        open={newDirFor != null}
        onOpenChange={(next) => {
          if (!next) setNewDirFor(null);
        }}
      >
        <DialogContent showCloseButton={false} className="sm:max-w-md">
          <DialogHeader>
            <DialogTitle>
              {t("chat.newDirectionDialogTitle", { name: newDirFor?.name ?? "" })}
            </DialogTitle>
            <DialogDescription>
              {t("chat.newDirectionSpawnNote")}
            </DialogDescription>
          </DialogHeader>
          <Input
            autoFocus
            value={newDirName}
            onChange={(e) => setNewDirName(e.target.value)}
            placeholder={t("chat.newDirectionNamePlaceholder")}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                const target = newDirFor;
                setNewDirFor(null);
                if (target)
                  onNewDirection?.(target, newDirName.trim() || undefined);
              }
            }}
          />
          {newDirFor?.cwdBranch == null && (
            <div className="flex items-start gap-2 rounded-lg border border-status-warning/40 bg-status-warning-soft px-3 py-2">
              <AlertTriangle className="size-4 shrink-0 text-status-warning" />
              <span className="font-caption text-xs text-foreground-secondary">
                {t("chat.newDirectionNonGit")}
              </span>
            </div>
          )}
          <div className="flex justify-end gap-2 pt-1">
            <Button variant="outline" onClick={() => setNewDirFor(null)}>
              {t("common.cancel")}
            </Button>
            <Button
              onClick={() => {
                const target = newDirFor;
                setNewDirFor(null);
                if (target)
                  onNewDirection?.(target, newDirName.trim() || undefined);
              }}
            >
              <Plus className="size-3.5" />
              {t("chat.newDirectionConfirm")}
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
