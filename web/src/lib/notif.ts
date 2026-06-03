/**
 * Shared notification rendering helpers.
 *
 * `humanizeBlackboard` lives here (not in a component) because BOTH the bell
 * popover and the full /notifications page render blackboard events — and they
 * MUST agree. They drifted once: the popover humanized the key while the page
 * printed the raw `write {32-hex-uuid}/{thread_slug}/task.ledger.md` storage
 * key, which is meaningless to a user. One source of truth prevents that.
 */

import type { MessageMeta, Workspace } from "@/api/types";

type Tr = (k: string, opts?: Record<string, unknown>) => string;

/** Auto blackboard wakes (and legacy untyped wakes, meta absent) are internal
 *  agent-coordination plumbing — redundant with the BlackboardChanged event the
 *  feed already shows. Both the bell popover and the full /notifications page
 *  hide them; only operator-initiated manual wakes (meta.reason === "manual")
 *  stay, since they record a real intervention. Shared so the two surfaces
 *  filter identically. */
export function isHiddenWake(m: {
  kind: string;
  meta?: MessageMeta | null;
}): boolean {
  return m.kind === "wake" && m.meta?.reason !== "manual";
}

/** A blackboard key is `{workspace_id}/{thread_slug}/{file}`. Render it as
 *  human text — a friendly ledger label + the workspace/direction names —
 *  instead of the raw 32-char UUID + slug + content hash the user can't read. */
export function humanizeBlackboard(
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
