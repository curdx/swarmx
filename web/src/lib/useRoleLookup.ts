import { useEffect, useState } from "react";
import { api } from "../api/http";
import type { AgentInfo } from "../api/types";

/**
 * agent_id → role lookup covering exited agents too, so historical messages
 * render with the right avatar colour even after agents die. Seeded from
 * /api/agent (best-effort) and kept merged with the live `activeMembers`.
 *
 * Extracted verbatim from MessagesPanel — same two effects, same merge logic.
 */
export function useRoleLookup(activeMembers: AgentInfo[]): Map<string, string> {
  const [roleLookup, setRoleLookup] = useState<Map<string, string>>(() => new Map());

  useEffect(() => {
    api
      .listAgents()
      .then((all) => {
        setRoleLookup((prev) => {
          const next = new Map(prev);
          for (const a of all) next.set(a.agent_id, a.role);
          return next;
        });
      })
      .catch(() => {
        /* best-effort; resolveRole falls back to id-prefix heuristic */
      });
  }, []);

  useEffect(() => {
    setRoleLookup((prev) => {
      const next = new Map(prev);
      for (const a of activeMembers) next.set(a.agent_id, a.role);
      return next;
    });
  }, [activeMembers]);

  return roleLookup;
}
