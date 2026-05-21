/**
 * Per-CLI keystroke forwarding policy.
 *
 * Why this exists: claude and codex both emit the same OSC ready marker, but
 * their TUIs become *usable* at different points relative to that marker.
 *
 *   - claude: OSC_READY fires after its REPL has fully attached stdin. Any
 *     keystroke we forward after ready is accepted normally.
 *   - codex:  OSC_READY fires while the ratatui input loop is still wiring
 *     up its crossterm event poll. Keystrokes that race the loop are
 *     swallowed by the startup banner; the first Enter is observed by the
 *     user as "newline instead of submit". A small post-ready settle period
 *     lets the input loop finish initializing before we deliver bytes.
 *
 * Pre-ready keystrokes are buffered (not dropped) so a user who clicks the
 * pane and starts typing before the CLI is up doesn't lose work — the
 * buffered bytes flush in a single send after the settle period.
 *
 * Adding a new CLI: prefix-match `agentId` (server allocates IDs as
 * `<plugin_id>-<8 hex>`), return its policy. Default falls through to the
 * permissive claude profile because most modern CLIs don't need any delay.
 */

export interface CliInputPolicy {
  /** Plugin id this policy targets. Useful for logs. */
  readonly cli: string;
  /**
   * Delay after `shim_ready` before forwarding the first keystroke. Some
   * TUIs emit OSC_READY before their input poll is attached and eat the
   * first byte; setting this >0 gives them breathing room.
   */
  readonly postReadyDelayMs: number;
  /**
   * Cap on bytes buffered while waiting for ready + settle. Anything beyond
   * this is dropped silently — a guard against an accidental paste-into-
   * not-yet-attached-pane filling memory.
   */
  readonly preReadyBufferMax: number;
}

export const CLAUDE_INPUT: CliInputPolicy = {
  cli: "claude",
  postReadyDelayMs: 0,
  preReadyBufferMax: 4096,
};

export const CODEX_INPUT: CliInputPolicy = {
  cli: "codex",
  // 300 ms is enough on local machines (codex's ratatui init takes ~120–180 ms
  // empirically) while still feeling instantaneous to the user. Bump if a
  // remote/laggy host shows the same first-Enter-lost symptom.
  postReadyDelayMs: 300,
  preReadyBufferMax: 4096,
};

/** Default for unknown plugins — match the permissive claude profile. */
const DEFAULT_INPUT: CliInputPolicy = CLAUDE_INPUT;

export function inputPolicyFor(agentId: string): CliInputPolicy {
  if (agentId.startsWith("codex-")) return CODEX_INPUT;
  if (agentId.startsWith("claude-")) return CLAUDE_INPUT;
  return DEFAULT_INPUT;
}
