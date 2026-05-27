/**
 * Workspace-scoped blackboard key helpers.
 *
 * Multiple workspaces share one global blackboard namespace, so generic
 * keys like `project.summary` would silently overwrite each other when
 * scouts from different workspaces run in parallel. We append a
 * filesystem-derived slug per workspace so each room owns its own key.
 *
 * IMPORTANT: the slug algorithm here MUST match the shell expression
 * embedded in `roles/scout.md` (the scout writes the key from its own
 * cwd via `pwd | sed ...`). Keep both in sync — see scout.md step 2.
 */

/** Turn an absolute filesystem path into a blackboard-safe slug.
 *  Examples:
 *    /Users/me/code/web      → Users_me_code_web
 *    /private/tmp/foo-bar    → private_tmp_foo-bar
 *  Allowed chars: A-Z a-z 0-9 . _ -  (everything else collapses to `_`). */
export function workspaceSlug(path: string): string {
  return path.replace(/^\/+/, "").replace(/[^a-zA-Z0-9._-]/g, "_");
}

/** Per-workspace key that scout writes its project digest to. */
export function projectSummaryKey(path: string): string {
  return `project.summary.${workspaceSlug(path)}`;
}

/** Per-workspace key holding the human-friendly name the user typed
 *  into the create-workspace wizard (e.g. "我的待办 App"). When absent
 *  the chat sidebar falls back to the path's basename. */
export function workspaceNameKey(path: string): string {
  return `workspace.name.${workspaceSlug(path)}`;
}

/** Prefix every per-workspace key shares — used by the wizard to listen
 *  for "scout finished writing the summary" via the WS feed without
 *  having to know which slug the scout ended up using (matters when
 *  client-side path doesn't quite match server-side canonicalized cwd,
 *  e.g. /tmp vs /private/tmp on macOS). */
export const PROJECT_SUMMARY_KEY_PREFIX = "project.summary.";
