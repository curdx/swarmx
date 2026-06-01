/** Shared workspace view-model types for the Shell layout and its sidebar.
 *  Kept in its own module so Shell.tsx, WorkspaceSidebar.tsx and the /chat
 *  Home page can all import `WorkspaceSummary` without a component-file
 *  cycle. */

import type { AgentInfo, ThreadInfo, WorkspaceRoot } from "../../api/types";

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
  /** The workspace's directions (always ≥1: an auto-created `main`).
   *  Oldest-first; the first entry is the main thread. Used for thread-aware
   *  routing (`/chat/:wsId/t/:threadSlug`) and per-direction key scoping. */
  threads: ThreadInfo[];
}
