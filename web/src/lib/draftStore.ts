/**
 * Composer-draft persistence — the localStorage read/write contract pulled out
 * of MessagesPanel so it can be unit-tested without a DOM.
 *
 * The hook (useComposerDraft) owns the React state and the effect timing; these
 * two functions own the storage rules: a draft is kept only while it still has
 * non-blank text, and every storage access is guarded so a disabled/full store
 * (Safari private mode, quota) can never crash the composer over a draft.
 */

/** Read the draft saved under `key`; "" when absent or storage is unavailable. */
export function loadDraft(storage: Pick<Storage, "getItem">, key: string): string {
  try {
    return storage.getItem(key) ?? "";
  } catch {
    return "";
  }
}

/**
 * Persist `value` under `key`, or remove the key when the draft is blank
 * (whitespace-only counts as blank). Storage errors are swallowed — a lost
 * draft is never worth crashing the composer.
 */
export function saveDraft(
  storage: Pick<Storage, "setItem" | "removeItem">,
  key: string,
  value: string,
): void {
  try {
    if (value && value.trim()) storage.setItem(key, value);
    else storage.removeItem(key);
  } catch {
    /* ignore */
  }
}
