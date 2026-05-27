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

/** Per-workspace accent color id chosen in the wizard. Used by chat
 *  sidebar / channel header to render a small color chip so the user
 *  can tell multiple workspaces apart at a glance (like Discord server
 *  icons or GitHub repo project labels). Value is one of ACCENT_OPTIONS'
 *  `id` string; falls back to "peach" (default accent) when missing. */
export function workspaceAccentKey(path: string): string {
  return `workspace.accent.${workspaceSlug(path)}`;
}

export const WORKSPACE_ACCENT_KEY_PREFIX = "workspace.accent.";

/** Wizard 给用户挑的 5 个 accent — id 持久化到 blackboard，cssVar 用于
 *  渲染时给 `style={{ background: cssVar }}` 之类的。Single source of
 *  truth: wizard / chat sidebar / channel header 都从这里读，加新 accent
 *  只改这里。*/
export const ACCENT_OPTIONS = [
  { id: "peach", cssVar: "var(--color-accent-primary)" },
  { id: "frontend", cssVar: "var(--color-agent-frontend)" },
  { id: "backend", cssVar: "var(--color-agent-backend)" },
  { id: "test", cssVar: "var(--color-agent-test)" },
  { id: "critic", cssVar: "var(--color-agent-critic)" },
] as const;

export type AccentId = (typeof ACCENT_OPTIONS)[number]["id"];

export function accentToCssVar(id: string | null | undefined): string {
  const found = ACCENT_OPTIONS.find((o) => o.id === id);
  return found?.cssVar ?? ACCENT_OPTIONS[0].cssVar;
}

/** Prefix every per-workspace key shares — used by the wizard to listen
 *  for "scout finished writing the summary" via the WS feed without
 *  having to know which slug the scout ended up using (matters when
 *  client-side path doesn't quite match server-side canonicalized cwd,
 *  e.g. /tmp vs /private/tmp on macOS). */
export const PROJECT_SUMMARY_KEY_PREFIX = "project.summary.";

/** Blackboard keys that fullstack-feature* spells write internally and
 *  read across roles. They live in the GLOBAL blackboard namespace (not
 *  per-workspace), so a previous spell run's leftover values bleed into
 *  the next run — `test` sees a stale `backend.done` and idles waiting
 *  for `codex-<new>` to overwrite it, adding minutes of delay.
 *
 *  Chat composer clears these keys right before firing auto-dispatch so
 *  the new spell starts from an empty slate. Cleared = write empty
 *  content (server doesn't expose a delete-key endpoint); downstream
 *  prompts already treat empty content as "not yet written".
 *
 *  KNOWN LIMITATION: clearing is global, so two workspaces firing
 *  auto-dispatch simultaneously would step on each other. Real fix is
 *  per-spell-run namespace in the blackboard schema; tracked as future
 *  work. Single-user single-active-spell is the 80% case where this
 *  workaround is fine. */
export const FULLSTACK_INTERNAL_KEYS = [
  "api.spec",
  "frontend.done",
  "frontend.review",
  "backend.done",
  "backend.review",
  "review.completed",
  "test.passed",
  "fixer.done",
  "fixer.skipped",
  "architect.done",
] as const;
