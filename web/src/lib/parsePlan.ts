/**
 * parsePlan — defensively turn the orchestrator's structured plan blackboard
 * value (`<ws>/<thread>/plan.json`) into a checklist the PlanStickyCard renders.
 *
 * P2 "稳妥版": the orchestrator writes a STRUCTURED JSON plan (not free prose),
 * so the frontend parses JSON instead of guessing structure from markdown. JSON
 * is still produced by an LLM, so this parser is forgiving — it accepts a bare
 * array or a `{steps:[…]}` wrapper, tolerates field-name variants
 * (task|text|label, owner_role|owner), normalizes status synonyms, and returns
 * `null` for anything it can't trust (malformed JSON, no usable steps). A wrong
 * card would betray the "不撒谎" principle, so when unsure it shows nothing.
 */

export type PlanStatus = "done" | "doing" | "blocked" | "todo";

export interface PlanStep {
  /** 1-based order if the orchestrator provided it; else undefined. */
  seq?: number;
  /** The step description (required — steps without it are dropped). */
  task: string;
  /** Role slug of the owner ("self"/orchestrator → the captain). */
  owner?: string;
  status: PlanStatus;
}

export interface ParsedPlan {
  steps: PlanStep[];
  /** unix-ms of the orchestrator's last plan update, if it included one. */
  updatedAt?: number;
}

const DONE = new Set(["done", "complete", "completed", "finished", "ok", "✓", "x"]);
const DOING = new Set([
  "doing",
  "in_progress",
  "in-progress",
  "inprogress",
  "running",
  "active",
  "wip",
  "current",
]);
const BLOCKED = new Set(["blocked", "waiting", "stuck", "wait", "paused"]);

export function normalizeStatus(raw: unknown): PlanStatus {
  if (typeof raw !== "string") return "todo";
  const s = raw.trim().toLowerCase();
  if (DONE.has(s)) return "done";
  if (DOING.has(s)) return "doing";
  if (BLOCKED.has(s)) return "blocked";
  return "todo";
}

function str(v: unknown): string | undefined {
  return typeof v === "string" && v.trim() ? v.trim() : undefined;
}

export function parsePlan(content: string | null | undefined): ParsedPlan | null {
  if (!content || !content.trim()) return null;
  let obj: unknown;
  try {
    obj = JSON.parse(content);
  } catch {
    return null; // not JSON → don't guess
  }
  const rawSteps: unknown = Array.isArray(obj)
    ? obj
    : obj && typeof obj === "object" && Array.isArray((obj as Record<string, unknown>).steps)
      ? (obj as Record<string, unknown>).steps
      : null;
  if (!Array.isArray(rawSteps)) return null;

  const steps: PlanStep[] = [];
  for (const raw of rawSteps) {
    if (!raw || typeof raw !== "object") continue;
    const r = raw as Record<string, unknown>;
    const task = str(r.task) ?? str(r.text) ?? str(r.label) ?? str(r.description);
    if (!task) continue;
    steps.push({
      seq: typeof r.seq === "number" ? r.seq : undefined,
      task,
      owner: str(r.owner_role) ?? str(r.owner) ?? str(r.role),
      status: normalizeStatus(r.status),
    });
  }
  if (steps.length === 0) return null;

  const updatedAt =
    obj && typeof obj === "object" && typeof (obj as Record<string, unknown>).updated_at === "number"
      ? ((obj as Record<string, unknown>).updated_at as number)
      : undefined;
  return { steps, updatedAt };
}
