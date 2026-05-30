/**
 * Per-CLI keystroke forwarding policy — DATA-DRIVEN from the backend manifest.
 *
 * Why this exists: claude and codex both emit the same OSC ready marker, but
 * their TUIs become *usable* at different points relative to that marker.
 *
 *   - claude: OSC_READY fires after its REPL has fully attached stdin. Any
 *     keystroke we forward after ready is accepted normally (settle = 0).
 *   - codex:  OSC_READY fires while the ratatui input loop is still wiring up
 *     its crossterm event poll. Keystrokes that race the loop are swallowed by
 *     the startup banner; the first Enter is observed as "newline not submit".
 *     A small post-ready settle period lets the loop finish initializing.
 *
 * The per-CLI settle value now lives in `cli-plugins/<id>.toml`
 * (`input_settle_ms`) and is served via `GET /api/plugins` → `CliPluginInfo`.
 * The frontend primes a cli→settle map once at startup (`primeInputPolicies`,
 * called from AppShell). Adding a new CLI is therefore a config change — no
 * `startsWith('codex-')` branch to edit here. Per the data-driven-UI /
 * VS Code "contributes" model: BEHAVIOR (input timing) lives in the backend
 * manifest; only the uniform memory guard (`preReadyBufferMax`) stays here.
 *
 * Pre-ready keystrokes are buffered (not dropped) so a user who clicks the
 * pane and starts typing before the CLI is up doesn't lose work.
 */

import type { CliPluginInfo } from "../api/types";

export interface CliInputPolicy {
  /** Plugin id this policy targets. Useful for logs. */
  readonly cli: string;
  /**
   * Delay after `shim_ready` before forwarding the first keystroke. Some TUIs
   * emit OSC_READY before their input poll is attached and eat the first byte;
   * >0 gives them breathing room. Sourced from the plugin manifest.
   */
  readonly postReadyDelayMs: number;
  /**
   * Cap on bytes buffered while waiting for ready + settle. Uniform across
   * CLIs (a memory guard against an accidental paste into a not-yet-attached
   * pane), so it is NOT a per-CLI manifest knob.
   */
  readonly preReadyBufferMax: number;
}

const PRE_READY_BUFFER_MAX = 4096;

/**
 * cli id → post-ready settle delay (ms), primed from the backend plugin list.
 * Empty until `primeInputPolicies` runs; an unknown / not-yet-primed cli falls
 * back to 0 (the permissive default — most CLIs need no delay).
 */
const settleByCli = new Map<string, number>();

/**
 * Populate the per-CLI settle map from `GET /api/plugins`. Called once at app
 * startup (AppShell). Idempotent — re-priming overwrites.
 */
export function primeInputPolicies(plugins: CliPluginInfo[]): void {
  for (const p of plugins) {
    settleByCli.set(p.id, p.input_settle_ms ?? 0);
  }
}

/** cli id is the prefix of the server-allocated agent id: `<cli>-<8 hex>`. */
function cliOf(agentId: string): string {
  const dash = agentId.indexOf("-");
  return dash > 0 ? agentId.slice(0, dash) : agentId;
}

export function inputPolicyFor(agentId: string): CliInputPolicy {
  const cli = cliOf(agentId);
  return {
    cli,
    postReadyDelayMs: settleByCli.get(cli) ?? 0,
    preReadyBufferMax: PRE_READY_BUFFER_MAX,
  };
}
