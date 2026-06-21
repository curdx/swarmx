/**
 * The "active workspace" the global tool pages (files / terminal / tasks /
 * usage) default to. Those pages live outside the `/chat/:ws` shell, so they
 * have no route param to scope by — instead the chat shell records the
 * workspace you're viewing here, and the tool pages read it as their default
 * selection. Persisted in localStorage so it survives reloads and is shared
 * across tabs. The value is a workspace UUID (`Workspace.id`), not the slug.
 */
import type { Workspace } from "@/api/types";

const KEY = "swarmx:activeWorkspace";

export function getActiveWorkspaceId(): string | null {
  try {
    return localStorage.getItem(KEY);
  } catch {
    return null;
  }
}

export function setActiveWorkspaceId(id: string): void {
  try {
    if (id) localStorage.setItem(KEY, id);
  } catch {
    /* ignore (private mode / disabled storage) */
  }
}

/**
 * Pick the default workspace for a tool page: the last-active one if it's still
 * in the list, else the first workspace, else "" (none yet). Pure/testable.
 */
export function pickDefaultWorkspace(list: Workspace[], stored: string | null): string {
  if (stored && list.some((w) => w.id === stored)) return stored;
  return list[0]?.id ?? "";
}
