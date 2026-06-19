/**
 * useEngineReadiness — the honest "can I actually use this engine" hook.
 *
 * `installed` (binary on PATH) is NOT "usable": a logged-out claude, a
 * key-less reasonix, a quota-exhausted codex are all installed but can't run.
 * The backend's real-usability probe (`engine_probe.rs`) settles that by
 * actually STARTING each CLI over PTY; this hook merges the install info
 * (`GET /api/plugins`) with the probe verdict (`GET /api/plugins/probe`) into a
 * single per-engine readiness the UI renders without ever faking a green.
 *
 * Stale-while-revalidate: on mount it shows whatever the cache already holds
 * (instant, possibly empty/stale), and `probe()` kicks a fresh sweep then polls
 * the verdicts in as each engine completes. Probing is expensive (real cold
 * starts, serial, tens of seconds each) so it is NEVER auto-kicked — only on an
 * explicit user action. Until then an installed-but-unprobed engine reads
 * "unknown" (shown neutrally), not "ready".
 */

import { useCallback, useEffect, useRef, useState } from "react";
import { api } from "@/api/http";
import type { EngineProbe, EngineReadiness } from "@/api/types";

const POLL_MS = 2500;
/** Safety cap so a stuck `probing` flag can't poll forever. A full 4-engine
 *  sweep (opencode cold start ~90s) fits comfortably under this. */
const POLL_MAX_MS = 6 * 60 * 1000;

export interface EngineReadinessState {
  /** Initial plugins+probe fetch still in flight. */
  loading: boolean;
  /** A real-usability sweep is running right now. */
  probing: boolean;
  engines: EngineReadiness[];
  /** Newest verdict timestamp across engines (unix-ms), or null if never probed. */
  lastProbedAt: number | null;
  /** Kick a fresh sweep and poll verdicts in. No-op while already probing. */
  probe: () => void;
  error: string | null;
}

function mergeState(
  installed: boolean,
  probe: EngineProbe | undefined,
): EngineReadiness["state"] {
  if (!installed) return "not_installed";
  // A stale "not_installed" verdict for a now-present binary shouldn't override
  // reality — fall back to unknown and let the user re-probe.
  if (probe && probe.state !== "not_installed") return probe.state;
  return "unknown";
}

export function useEngineReadiness(): EngineReadinessState {
  const [loading, setLoading] = useState(true);
  const [probing, setProbing] = useState(false);
  const [engines, setEngines] = useState<EngineReadiness[]>([]);
  const [lastProbedAt, setLastProbedAt] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);

  const mounted = useRef(true);
  const pollTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const pollStartedAt = useRef<number>(0);

  // Merge the latest install list with the latest probe verdicts. Install
  // order (registry order) is preserved so engines render consistently.
  const applyMerge = useCallback(
    (probes: EngineProbe[]) => {
      const byId = new Map(probes.map((p) => [p.engine, p]));
      setEngines((prev) =>
        prev.map((e) => {
          const probe = byId.get(e.id);
          return {
            ...e,
            state: mergeState(e.installed, probe),
            reason: probe?.reason ?? null,
            kind: probe?.kind ?? null,
            probed_at: probe?.probed_at ?? null,
            method: probe?.method ?? null,
          };
        }),
      );
      const newest = probes.reduce<number | null>(
        (max, p) => (p.probed_at > (max ?? 0) ? p.probed_at : max),
        null,
      );
      if (newest !== null) setLastProbedAt(newest);
    },
    [],
  );

  const stopPolling = useCallback(() => {
    if (pollTimer.current) {
      clearTimeout(pollTimer.current);
      pollTimer.current = null;
    }
  }, []);

  // One poll tick: re-read verdicts, keep going while the backend reports a
  // sweep is in flight (and we're under the safety cap).
  const pollOnce = useCallback(() => {
    api
      .getEngineProbe()
      .then((resp) => {
        if (!mounted.current) return;
        applyMerge(resp.engines);
        setProbing(resp.probing);
        const overCap = Date.now() - pollStartedAt.current > POLL_MAX_MS;
        if (resp.probing && !overCap) {
          pollTimer.current = setTimeout(pollOnce, POLL_MS);
        } else {
          stopPolling();
          if (overCap) setProbing(false);
        }
      })
      .catch(() => {
        // Transient fetch error: keep polling (the sweep may still be running)
        // until the safety cap, then give up.
        if (!mounted.current) return;
        if (Date.now() - pollStartedAt.current > POLL_MAX_MS) {
          stopPolling();
          setProbing(false);
        } else {
          pollTimer.current = setTimeout(pollOnce, POLL_MS);
        }
      });
  }, [applyMerge, stopPolling]);

  const probe = useCallback(() => {
    if (probing) return;
    setProbing(true);
    setError(null);
    pollStartedAt.current = Date.now();
    api.probeEngines().catch(() => {
      /* the POST only kicks; verdicts come from polling regardless */
    });
    stopPolling();
    pollTimer.current = setTimeout(pollOnce, POLL_MS);
  }, [probing, pollOnce, stopPolling]);

  // Initial load: plugins (install info) + cached probe verdicts, in parallel.
  // If a sweep is already in flight (another tab kicked it), start polling.
  useEffect(() => {
    mounted.current = true;
    Promise.all([api.listPlugins(), api.getEngineProbe()])
      .then(([plugins, probeResp]) => {
        if (!mounted.current) return;
        const byId = new Map(probeResp.engines.map((p) => [p.engine, p]));
        const merged: EngineReadiness[] = plugins.map((p) => {
          const probe = byId.get(p.id);
          return {
            id: p.id,
            display_name: p.display_name,
            installed: p.installed === true,
            install: p.install ?? null,
            state: mergeState(p.installed === true, probe),
            reason: probe?.reason ?? null,
            kind: probe?.kind ?? null,
            probed_at: probe?.probed_at ?? null,
            method: probe?.method ?? null,
          };
        });
        setEngines(merged);
        const newest = probeResp.engines.reduce<number | null>(
          (max, p) => (p.probed_at > (max ?? 0) ? p.probed_at : max),
          null,
        );
        setLastProbedAt(newest);
        setLoading(false);
        if (probeResp.probing) {
          setProbing(true);
          pollStartedAt.current = Date.now();
          pollTimer.current = setTimeout(pollOnce, POLL_MS);
        }
      })
      .catch((e) => {
        if (!mounted.current) return;
        setError(e instanceof Error ? e.message : String(e));
        setLoading(false);
      });
    return () => {
      mounted.current = false;
      stopPolling();
    };
    // pollOnce/stopPolling are stable enough; intentionally run once on mount.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return { loading, probing, engines, lastProbedAt, probe, error };
}
