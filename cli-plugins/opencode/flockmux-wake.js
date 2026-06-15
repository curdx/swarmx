// flockmux wake plugin for OpenCode (Model A — PTY transport).
//
// OpenCode has NO blocking Stop hook (its only turn-end signal is a read-only
// `session.idle` event whose handler returns void — see opencode
// packages/plugin/src/index.ts `Hooks.event`). claude/codex get re-woken by a
// Stop hook that returns `{"decision":"block"}`; opencode can't. So flockmux
// runs the wake loop AS A PLUGIN: on every `session.idle`, ask flockmux whether
// this agent has pending wakes; if so, start a new turn by re-prompting the
// session with the same steering text the Rust Stop-hook helper
// (`flockmux-mcp wake-check`) feeds claude/codex.
//
// This is the opencode equivalent of crates/flockmux-mcp/src/wake_check.rs.
// It hits the SAME endpoint (`POST /api/message/consume_wakes`), which
// ATOMICALLY claims + marks-read all pending kind="wake" messages and returns
// their count. Because consume is atomic, repeated calls are safe: once the
// wakes are claimed the count is 0, so the agent is NOT re-prompted in a loop
// (the turn we start here ends in another `session.idle`, we ask again, get 0,
// and stop).
//
// Identity + server come from the env flockmux's spawn.rs injects into the
// opencode process: FLOCKMUX_AGENT_ID, FLOCKMUX_SERVER_URL.
//
// STATUS: written from opencode v1.17.x source (plugin Hooks API + @opencode-ai
// SDK `client.session.prompt`), NOT yet validated against a live opencode. Pin
// the event shape / SDK call against a running opencode before relying on it.

const AGENT_ID = process.env.FLOCKMUX_AGENT_ID
const SERVER_URL = (process.env.FLOCKMUX_SERVER_URL || "http://127.0.0.1:7777").replace(/\/+$/, "")

// Mirrors the reason string built in wake_check.rs::emit_block so a woken
// opencode worker follows the exact same recovery path as claude/codex.
function wakeReason(count) {
  return [
    `You were woken up: ${count} new wake event(s) just arrived.`,
    `A blackboard key you depend_on was likely written.`,
    `Steps:`,
    `1. Call swarm_list_blackboard, then swarm_read_blackboard on any key you depend on.`,
    `2. If you also have pending non-wake messages, call swarm_list_messages.`,
    `3. Continue with your role's workflow. If you decide to reply, use`,
    `   swarm_send_message with kind:"reply" AND in_reply_to:<id>.`,
    `Do not produce any user-facing output about these wakes outside the swarm tool calls.`,
  ].join("\n")
}

export const FlockmuxWake = async ({ client }) => {
  return {
    event: async ({ event }) => {
      // Only act at a turn boundary, and only if we know who we are.
      if (!event || event.type !== "session.idle") return
      if (!AGENT_ID) return
      const sessionID = event.properties && event.properties.sessionID
      if (!sessionID) return

      let count = 0
      try {
        const res = await fetch(
          `${SERVER_URL}/api/message/consume_wakes?to=${encodeURIComponent(AGENT_ID)}`,
          { method: "POST" },
        )
        if (!res.ok) return
        const body = await res.json()
        count = (body && typeof body.count === "number") ? body.count : 0
      } catch {
        // Server unreachable → never block the agent; just skip this wake.
        return
      }
      if (count <= 0) return

      try {
        await client.session.prompt({
          path: { id: sessionID },
          body: { parts: [{ type: "text", text: wakeReason(count) }] },
        })
      } catch {
        // Re-prompt failed → the wakes were already consumed; they will be
        // re-delivered as fresh wakes by flockmux's WakeCoordinator if the
        // dependency is still unsatisfied. Never throw from a hook.
      }
    },
  }
}
