/**
 * Global WebGL context budget for xterm panes.
 *
 * Why:
 *   - Browsers cap concurrent WebGL contexts at ~16 per page (Chrome/Edge);
 *     overshooting silently fires `contextlost` on the *oldest* context,
 *     which then knocks down whichever pane was rendering there. That's
 *     racy and user-visible.
 *   - Hidden panes (display:none under a maximize / minimize toggle) still
 *     hold a context unless we release it — so by the time the user opens
 *     N≈12 agents and then maximises one, the maximised pane is on its way
 *     to losing its renderer.
 *
 * Strategy:
 *   - Cap ourselves at MAX_CTXS = 12 (4-slot buffer below the browser limit
 *     for devtools, charts, etc.).
 *   - Panes that can't acquire fall back to xterm's DOM renderer. DOM is
 *     slower but won't lose its context.
 *   - On a `webglcontextlost` event we go into a global COOLDOWN_MS window
 *     where *no* pane can acquire — prevents thundering-herd reacquires
 *     while the GPU process is settling. (Mirrors golutra's
 *     setWebglCooldown.)
 *   - Visibility-driven: panes release on hide and reacquire on show, so
 *     budget tracks what's actually on-screen.
 */

const MAX_CTXS = 12;
const COOLDOWN_MS = 30_000;

const activeIds = new Set<string>();
let cooldownUntil = 0;

export function acquireSlot(agentId: string): boolean {
  if (activeIds.has(agentId)) return true;
  if (Date.now() < cooldownUntil) return false;
  if (activeIds.size >= MAX_CTXS) return false;
  activeIds.add(agentId);
  return true;
}

export function releaseSlot(agentId: string): void {
  activeIds.delete(agentId);
}

/**
 * Called from `webglAddon.onContextLoss`. Drops the caller's slot and
 * prevents any new acquires for COOLDOWN_MS — gives the GPU process room
 * to stabilise before we hammer it again.
 */
export function reportContextLoss(agentId: string): void {
  activeIds.delete(agentId);
  cooldownUntil = Date.now() + COOLDOWN_MS;
}

export function isInCooldown(): boolean {
  return Date.now() < cooldownUntil;
}

export function poolStats(): {
  active: number;
  max: number;
  cooldownMsRemaining: number;
} {
  return {
    active: activeIds.size,
    max: MAX_CTXS,
    cooldownMsRemaining: Math.max(0, cooldownUntil - Date.now()),
  };
}
