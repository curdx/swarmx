/**
 * useAppSettingsBehaviors — wires the three behavioural Settings toggles
 * (routes/settings.tsx → swarmx:settings:v1) to real runtime effects. Mounted
 * once, in AppShell, so it lives across every route.
 *
 * 1. openMainOnLaunch — on first mount under Tauri, if the user opted out, hide
 *    the window to the tray. No-op in a browser.
 * 2. desktopNotify — on a real agent→user reply, fire an OS notification (Tauri
 *    only) when the user isn't actively looking at the app. De-duped per message
 *    id so a re-delivery / reconnect can't double-fire.
 * 3. killOthersOnFail — when an agent newly fails, kill the OTHER live agents in
 *    the same workspace (preferring same spell run). Destructive, so guarded:
 *    diffs a previous-snapshot ref to detect the *transition* into failure, and
 *    a processed-id set ensures each failure is acted on at most once.
 *
 * The swarm WS feed carries no `shim_exit` / `last_error` / `workspace_id` — those
 * live only on the REST `AgentInfo` row. So failure detection re-fetches
 * `listAgents()` whenever an `agent_state` event lands (and on (re)connect) and
 * diffs that fresh snapshot, rather than trying to infer failure from the lossy
 * `state:"error"` event alone.
 *
 * All Tauri-only modules (window control, notification plugin) are dynamically
 * imported and try/catch-guarded so a browser dev build is a clean no-op.
 */

import { useCallback, useEffect, useRef } from "react";
import type { AgentInfo, SwarmEvent } from "@/api/types";
import { api } from "@/api/http";
import { useSwarmFeed } from "@/hooks/useSwarmFeed";
import { loadAppSettings } from "@/lib/appSettings";
import { isTauriOverlayWindow } from "@/lib/tauriWindowChrome";
import i18n from "@/i18n";

/** A REST agent row counts as "failed" if it crashed (non-zero shim_exit) or the
 *  server flagged it alive-but-stuck (last_error set). A clean exit (shim_exit
 *  === 0) and a deliberate kill (killed_at set) are NOT failures. */
function isFailed(a: AgentInfo): boolean {
  if (a.shim_exit != null && a.shim_exit !== 0) return true;
  if (a.last_error != null && a.last_error !== "") return true;
  return false;
}

/** A row is "live" (eligible to be killed) when it has neither been killed nor
 *  exited. */
function isLive(a: AgentInfo): boolean {
  return a.killed_at == null && a.shim_exit == null;
}

async function hideWindowIfOptedOut(): Promise<void> {
  if (!isTauriOverlayWindow()) return;
  if (loadAppSettings().openMainOnLaunch) return;
  try {
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    await getCurrentWindow().hide();
  } catch {
    /* not Tauri / window API unavailable — no-op */
  }
}

async function notifyAgentReply(from: string): Promise<void> {
  try {
    const { isPermissionGranted, requestPermission, sendNotification } =
      await import("@tauri-apps/plugin-notification");
    let granted = await isPermissionGranted();
    if (!granted) granted = (await requestPermission()) === "granted";
    if (granted)
      sendNotification({
        title: "swarmx",
        body: i18n.t("agent.notifyReply", { from, defaultValue: "{{from}} 回复了你" }),
      });
  } catch {
    /* not Tauri / plugin unavailable — no-op */
  }
}

export function useAppSettingsBehaviors(): void {
  // ── 1) launch show/hide (Tauri only) ────────────────────────────────────
  // Run once on mount. loadAppSettings() reads the persisted value; a browser
  // build short-circuits inside hideWindowIfOptedOut.
  useEffect(() => {
    void hideWindowIfOptedOut();
  }, []);

  // ── 2) desktop notification de-dupe ──────────────────────────────────────
  // Ids we've already notified for. Bounded so a long session can't grow it
  // unboundedly (insertion order = oldest-first; we trim from the front).
  const notifiedIds = useRef<Set<number>>(new Set());

  // ── 3) kill-others-on-fail bookkeeping ───────────────────────────────────
  // Previous REST snapshot keyed by agent_id, to detect the *transition* into
  // failure (so we don't re-trigger every poll). Failures already acted on, so
  // a flapping snapshot can't kill twice.
  const prevAgentsRef = useRef<Map<string, AgentInfo>>(new Map());
  const handledFailures = useRef<Set<string>>(new Set());
  const refreshingRef = useRef(false);
  // Has the baseline snapshot been seeded yet? The FIRST reconcile only records
  // state (and pre-marks already-failed agents as handled) — it must NOT kill,
  // or a stale failure that predates this page load would trigger a sweep.
  const seededRef = useRef(false);

  const killOtherLiveAgents = useCallback(
    (failed: AgentInfo, all: AgentInfo[]) => {
      const wsId = failed.workspace_id;
      // No workspace to scope by ⇒ refuse to act (we won't kill the whole world).
      if (wsId == null) return;
      const runId = failed.spell_run_id ?? null;
      const victims = all.filter((a) => {
        if (a.agent_id === failed.agent_id) return false; // never the failer
        if (a.workspace_id !== wsId) return false; // same workspace only
        // Prefer same spell run when the failer belongs to one — keeps a single
        // run's blast radius contained instead of nuking unrelated agents that
        // merely share the workspace.
        if (runId != null && (a.spell_run_id ?? null) !== runId) return false;
        return isLive(a);
      });
      for (const v of victims) {
        api.killAgent(v.agent_id).catch(() => {
          /* soft-fail: one kill failing must not block the others */
        });
      }
    },
    [],
  );

  const refreshAndReconcile = useCallback(async () => {
    // Coalesce overlapping refreshes (events can burst): if one is in flight,
    // skip — the in-flight one will read the latest server state anyway.
    if (refreshingRef.current) return;
    refreshingRef.current = true;
    let rows: AgentInfo[];
    try {
      rows = await api.listAgents();
    } catch {
      return; // transient fetch failure — try again on the next event
    } finally {
      refreshingRef.current = false;
    }

    const prev = prevAgentsRef.current;
    const seeding = !seededRef.current;
    const killOn = loadAppSettings().killOthersOnFail;

    for (const row of rows) {
      const nowFailed = isFailed(row);
      if (seeding) {
        // Baseline pass: never kill. Pre-mark anything already failed as handled
        // so a later refresh doesn't treat its lingering failure as "new".
        if (nowFailed) handledFailures.current.add(row.agent_id);
        continue;
      }
      const before = prev.get(row.agent_id);
      const wasFailed = before ? isFailed(before) : false;
      // A *new* failure: this agent wasn't failed last snapshot (or we'd never
      // seen it) and is failed now. The handledFailures guard makes this
      // idempotent even if the diff somehow re-fires.
      const newlyFailed = nowFailed && !wasFailed;
      if (
        newlyFailed &&
        killOn &&
        !handledFailures.current.has(row.agent_id)
      ) {
        handledFailures.current.add(row.agent_id);
        killOtherLiveAgents(row, rows);
      }
    }

    // Rebuild the snapshot from the fresh rows.
    const nextMap = new Map<string, AgentInfo>();
    for (const row of rows) nextMap.set(row.agent_id, row);
    prevAgentsRef.current = nextMap;
    seededRef.current = true;
  }, [killOtherLiveAgents]);

  const onEvent = useCallback(
    (ev: SwarmEvent) => {
      // Desktop notification: a real agent→user reply.
      if (ev.type === "message") {
        const realReply =
          ev.to_agent === "user" &&
          ev.from_agent !== "user" &&
          ev.from_agent !== "system" &&
          ev.kind !== "wake" &&
          ev.meta?.subtype !== "completion";
        if (
          realReply &&
          isTauriOverlayWindow() &&
          loadAppSettings().desktopNotify &&
          document.visibilityState !== "visible" &&
          !notifiedIds.current.has(ev.id)
        ) {
          notifiedIds.current.add(ev.id);
          // Bound the de-dupe set so a marathon session can't leak memory.
          if (notifiedIds.current.size > 500) {
            const oldest = notifiedIds.current.values().next().value;
            if (oldest !== undefined) notifiedIds.current.delete(oldest);
          }
          void notifyAgentReply(ev.from_agent);
        }
      }

      // Kill-others-on-fail: an agent's lifecycle changed — re-fetch the REST
      // snapshot (which carries shim_exit/last_error/workspace_id) and diff. We
      // refresh on ANY agent_state event (cheap, coalesced) rather than trying
      // to read failure off the lossy `state:"error"` payload.
      if (ev.type === "agent_state") {
        void refreshAndReconcile();
      }
    },
    [refreshAndReconcile],
  );

  const onReconnect = useCallback(() => {
    // Seed / re-seed the snapshot on (re)connect so the first post-connect
    // failure transition is detected against a real baseline, not an empty map
    // (which would make every already-failed agent look "newly" failed).
    void refreshAndReconcile();
  }, [refreshAndReconcile]);

  useSwarmFeed({ onEvent, onReconnect });
}
