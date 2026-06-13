import {
  useCallback,
  useState,
  type Dispatch,
  type SetStateAction,
} from "react";
import { api } from "../api/http";
import type { AgentInfo } from "../api/types";

/**
 * P0-9 stop controls — the 打断 menu (stopMenuOpen), the per-member interrupt
 * spinner (interruptingId), and the two stop actions. Lifted out of
 * MessagesPanel; consumes markInterrupted (from usePendingResponders, for the
 * optimistic clear) and runningMembers (the stop-all target). Extracted verbatim
 * — same setInterruptingId/markInterrupted/api.interruptAgent dance, same deps,
 * same finally-resets-interruptingId-only-in-stopMember asymmetry.
 *
 * queuedHint stays in the component on purpose: its writer lives inside send()
 * (a send-side "已排队" chip), not in these controls.
 */
export function useInterruptControls(opts: {
  markInterrupted: (agentId: string) => void;
  runningMembers: AgentInfo[];
}): {
  stopMenuOpen: boolean;
  setStopMenuOpen: Dispatch<SetStateAction<boolean>>;
  interruptingId: string | null;
  stopMember: (agentId: string) => Promise<void>;
  stopAllRunning: () => Promise<void>;
} {
  const { markInterrupted, runningMembers } = opts;
  const [stopMenuOpen, setStopMenuOpen] = useState(false);
  const [interruptingId, setInterruptingId] = useState<string | null>(null);

  const stopMember = useCallback(
    async (agentId: string) => {
      setInterruptingId(agentId);
      markInterrupted(agentId); // optimistic clear, before the round-trip
      try {
        await api.interruptAgent(agentId);
      } catch {
        /* best-effort — interrupt is idempotent; UI doesn't block on it */
      } finally {
        setInterruptingId(null);
      }
    },
    [markInterrupted],
  );

  const stopAllRunning = useCallback(async () => {
    const ids = runningMembers.map((m) => m.agent_id);
    ids.forEach(markInterrupted);
    setStopMenuOpen(false);
    await Promise.allSettled(ids.map((id) => api.interruptAgent(id)));
  }, [runningMembers, markInterrupted]);

  return {
    stopMenuOpen,
    setStopMenuOpen,
    interruptingId,
    stopMember,
    stopAllRunning,
  };
}
