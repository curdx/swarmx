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

/** Split a filesystem path into its last segment (`name`) and everything
 *  before it (`parent`), for the sidebar's name + mono-caption display.
 *  Trailing slashes are trimmed; a bare name (no separator) has empty parent.
 *  Shared by the Shell layout and the workspace sidebar tree. */
export function splitWorkspacePath(path: string): { name: string; parent: string } {
  if (!path || path === "(no workspace)") return { name: path || "", parent: "" };
  const trimmed = path.replace(/[\\/]+$/, "");
  const idx = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  if (idx < 0) return { name: trimmed, parent: "" };
  return { name: trimmed.slice(idx + 1) || trimmed, parent: trimmed.slice(0, idx) };
}

// workspaceNameKey / workspaceAccentKey / WORKSPACE_ACCENT_KEY_PREFIX /
// WORKSPACE_NAME_KEY_PREFIX_VALUE 在 workspace-as-first-class refactor
// 中被删了 — name 和 accent 现在直接是 workspaces 表的列，CreateWizard
// 通过 POST /api/workspaces 写入。前端不再用 blackboard 存这两个值。

/** Wizard 给用户挑的 5 个 accent — id 现在持久化到 workspaces 表，cssVar 用于
 *  渲染时给 `style={{ background: cssVar }}` 之类的。Single source of
 *  truth: wizard / chat sidebar / channel header 都从这里读，加新 accent
 *  只改这里。*/
// `id` is the value persisted to workspaces.accent — DO NOT rename it (would
// orphan existing rows' colors). `nameKey` is what the picker shows the user:
// the ids are legacy role names (frontend/backend/test/critic) and even
// "peach" actually renders blue, so labelling swatches by id confused users
// ("why is my workspace color called backend?"). nameKey points at the real
// hue instead. cssVar resolves the actual color.
export const ACCENT_OPTIONS = [
  { id: "peach", cssVar: "var(--color-accent-primary)", nameKey: "wizard.accentColors.blue" },
  { id: "frontend", cssVar: "var(--color-agent-frontend)", nameKey: "wizard.accentColors.cyan" },
  { id: "backend", cssVar: "var(--color-agent-backend)", nameKey: "wizard.accentColors.violet" },
  { id: "test", cssVar: "var(--color-agent-test)", nameKey: "wizard.accentColors.green" },
  { id: "critic", cssVar: "var(--color-agent-critic)", nameKey: "wizard.accentColors.orange" },
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
