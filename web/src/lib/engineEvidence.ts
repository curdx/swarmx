/**
 * Evidence strength behind a "usable" engine verdict.
 *
 * The probe records HOW it reached "usable" in `method`. A green "可用" can mean
 * very different things, and conflating them is the kind of dishonest UI this
 * project forbids:
 *   - "turn-ok"    → a real one-turn check passed (sent a prompt, the model
 *                    answered). The strongest proof the engine can actually work.
 *   - "live-ready" → a live agent of this engine came up healthy in real use
 *                    (write-back). Strong, but launch-level (no turn confirmed).
 *   - "ready" /    → only the process launched. Weakest: a logged-out / bad-key
 *     "exit-ok"      engine can still reach this (it only fails on a turn).
 *
 * This maps `method` to an evidence kind the UI renders as a small marker, so
 * "usable" is never ambiguous about how strongly it was verified. Only meaningful
 * for the "usable" state; everything else returns "none" (the state badge +
 * reason tooltip already carry the story).
 */
import type { EngineReadiness } from "@/api/types";

export type Evidence = "verified" | "live" | "launch" | "none";

export function evidenceOf(
  r: Pick<EngineReadiness, "state" | "method">,
): Evidence {
  if (r.state !== "usable") return "none";
  switch (r.method) {
    case "turn-ok":
      return "verified";
    case "live-ready":
      return "live";
    default:
      // "ready", "exit-ok", or anything else that still classified usable.
      return "launch";
  }
}

/** i18n keys for an evidence kind: a short chip label + a one-line detail
 *  (shown as a tooltip). Defined here so the chat + settings surfaces stay in
 *  sync. Components supply their own icons/tones. */
export const EVIDENCE_I18N: Record<
  Exclude<Evidence, "none">,
  { label: string; detail: string }
> = {
  verified: {
    label: "engine.evidence.verified",
    detail: "engine.evidence.verifiedDetail",
  },
  live: {
    label: "engine.evidence.live",
    detail: "engine.evidence.liveDetail",
  },
  launch: {
    label: "engine.evidence.launch",
    detail: "engine.evidence.launchDetail",
  },
};
