import {
  useEffect,
  useRef,
  useState,
  type Dispatch,
  type SetStateAction,
} from "react";
import { loadDraft, saveDraft } from "./draftStore";

/**
 * Composer body text, persisted per `draftKey` across direction/workspace
 * switches, reloads, and tab close. Extracted verbatim from MessagesPanel:
 * same two effects, same ref-captured-old-key cleanup, same beforeunload
 * fallback. Returns the [body, setBody] pair the composer drives directly.
 *
 * `draftKey` is computed by the caller (which also clears it on send), so it is
 * passed in rather than assembled here.
 */
export function useComposerDraft(
  draftKey: string,
): [string, Dispatch<SetStateAction<string>>] {
  const [body, setBody] = useState("");
  const bodyRef = useRef(body);
  bodyRef.current = body;
  const draftKeyRef = useRef(draftKey);

  useEffect(() => {
    // Load the incoming draft. The cleanup (runs on key change + unmount) saves
    // the OUTGOING draft under the key it belonged to (refs still hold the old
    // values at cleanup time — the new effect body updates them afterwards).
    const v = loadDraft(window.localStorage, draftKey);
    draftKeyRef.current = draftKey;
    setBody(v);
    return () => {
      saveDraft(window.localStorage, draftKeyRef.current, bodyRef.current);
    };
  }, [draftKey]);

  useEffect(() => {
    // Hard refresh / tab close doesn't run React cleanup — persist there too.
    // setItem-only on purpose: removal is owned by cleanup + send, not unload.
    const save = () => {
      const val = bodyRef.current;
      if (val && val.trim()) {
        try {
          window.localStorage.setItem(draftKey, val);
        } catch {
          /* ignore */
        }
      }
    };
    window.addEventListener("beforeunload", save);
    return () => window.removeEventListener("beforeunload", save);
  }, [draftKey]);

  return [body, setBody];
}
