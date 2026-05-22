/**
 * SpellsLauncher — small header control that lists `/api/spells` in a
 * dropdown and lets the user fire one off with a task description.
 *
 * Why this component is intentionally small:
 *   - Spells are spawn-orchestrators; once launched the user interacts
 *     with the individual agents through the existing pane UI. So this
 *     component owns no per-agent state — it just kicks the API and
 *     forgets.
 *   - We list spells on mount only; new spells require editing files on
 *     disk and restarting the server (no hot-reload), so the dropdown is
 *     stable for the session's lifetime.
 *   - Failure modes are user-facing: a /api/spells 500 is logged inline
 *     so people don't think the dropdown is just empty.
 *
 * Two run paths:
 *   - "✨ Auto" — primary CTA. Hardcodes `name: "auto-dispatch"`; the
 *     planner agent reads the task, picks a downstream spell, and
 *     launches it. Workspace input is ignored (planner picks one).
 *     Hidden if the auto-dispatch spell isn't loaded server-side.
 *   - "run" — manual. Uses whatever the user picked in the dropdown
 *     + the explicit workspace dir. Always available; useful when
 *     the user already knows which spell they want or wants to feed
 *     a specific workspace path.
 */

import { useEffect, useState } from "react";
import { api } from "../api/http";
import type { SpellInfo } from "../api/types";

interface Props {
  /** Notify the parent that new agents popped into existence so it can
   *  refresh its agent list. The parent already polls /ws/swarm for
   *  `agent_state=spawning`, so this is belt-and-braces. */
  onSpellLaunched?: () => void;
}

export function SpellsLauncher({ onSpellLaunched }: Props) {
  const [spells, setSpells] = useState<SpellInfo[]>([]);
  const [selected, setSelected] = useState<string>("");
  const [task, setTask] = useState("");
  // Optional shared workspace directory. Only meaningful for spells
  // whose manifest sets `shared_workspace = true` (M6a fullstack-feature
  // is the only one that does today). For per-agent spells like
  // critic-loop the server ignores this field. Showing it for every
  // spell keeps the launcher uniform — populating it for a spell that
  // doesn't need it has no side effects.
  const [workspaceDir, setWorkspaceDir] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [lastRun, setLastRun] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    api
      .listSpells()
      .then((rows) => {
        if (cancelled) return;
        setSpells(rows);
        if (rows.length > 0 && !selected) setSelected(rows[0].name);
      })
      .catch((e) => {
        if (cancelled) return;
        setError(`spell list failed: ${(e as Error).message}`);
      });
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  if (spells.length === 0 && !error) {
    // Nothing to show — keep the header tidy.
    return null;
  }

  // Shared launch path: PUT-and-forget the spell run, render the spawn
  // summary, clear the task input on success. `name` is the only thing
  // that differs between the manual and auto buttons — auto hardcodes
  // "auto-dispatch", manual uses whatever the dropdown selected.
  const launch = async (name: string, includeWorkspace: boolean) => {
    if (!name || !task.trim()) return;
    setBusy(true);
    setError(null);
    setLastRun(null);
    try {
      const wd = includeWorkspace ? workspaceDir.trim() : "";
      const resp = await api.runSpell({
        name,
        task: task.trim(),
        ...(wd ? { workspace_dir: wd } : {}),
      });
      setLastRun(
        `spawned ${resp.agents.length} agent(s) via ${name}: ${resp.agents
          .map((a) => `${a.role}=${a.agent_id}`)
          .join(", ")}`,
      );
      setTask("");
      onSpellLaunched?.();
    } catch (e) {
      setError(`run failed: ${(e as Error).message}`);
    } finally {
      setBusy(false);
    }
  };

  const run = () => launch(selected, true);
  const runAuto = () => launch("auto-dispatch", false);

  const hasAutoDispatch = spells.some((s) => s.name === "auto-dispatch");
  const current = spells.find((s) => s.name === selected);

  return (
    <div style={wrap}>
      <input
        type="text"
        value={task}
        onChange={(e) => setTask(e.target.value)}
        onKeyDown={(e) => {
          // Enter on the task input triggers the PRIMARY action: Auto if
          // available, otherwise manual. Power users who want manual with
          // Enter can press it while focused in the dropdown or workspace
          // input (those forward to `run()` below).
          if (e.key === "Enter") {
            if (hasAutoDispatch) runAuto();
            else run();
          }
        }}
        placeholder="任务描述 / what do you want to build?"
        style={input}
        disabled={busy}
      />
      {hasAutoDispatch && (
        <button
          onClick={runAuto}
          disabled={busy || !task.trim()}
          style={autoButton}
          title="Auto: planner picks the right spell for your task. Workspace dir below is ignored — the planner picks one."
        >
          {busy ? "thinking…" : "✨ Auto"}
        </button>
      )}
      <span style={divider}>or</span>
      <select
        value={selected}
        onChange={(e) => setSelected(e.target.value)}
        style={select}
        disabled={busy}
        title={current?.description}
      >
        {spells.map((s) => (
          <option key={s.name} value={s.name}>
            {s.name} ({s.agents.map((a) => `${a.role}:${a.cli}`).join(" → ")})
          </option>
        ))}
      </select>
      <input
        type="text"
        value={workspaceDir}
        onChange={(e) => setWorkspaceDir(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") run();
        }}
        placeholder="workspace dir (shared spells)"
        style={workspaceInput}
        disabled={busy}
        title="Absolute path. Only used by spells with shared_workspace=true (e.g. fullstack-feature). Leave blank to let the server pick one."
      />
      <button onClick={run} disabled={busy || !task.trim()} title="run the spell selected in the dropdown">
        run
      </button>
      {error && <span style={errStyle}>{error}</span>}
      {lastRun && <span style={okStyle}>{lastRun}</span>}
    </div>
  );
}

const wrap: React.CSSProperties = {
  display: "flex",
  gap: 4,
  alignItems: "center",
};

const select: React.CSSProperties = {
  background: "#0b1220",
  color: "#e2e8f0",
  border: "1px solid #374151",
  borderRadius: 4,
  padding: "2px 6px",
  fontSize: 12,
  fontFamily: "inherit",
};

const input: React.CSSProperties = {
  background: "#0b1220",
  color: "#e2e8f0",
  border: "1px solid #374151",
  borderRadius: 4,
  padding: "2px 6px",
  fontSize: 12,
  fontFamily: "inherit",
  minWidth: 200,
};

const workspaceInput: React.CSSProperties = {
  background: "#0b1220",
  color: "#cbd5e1",
  border: "1px solid #374151",
  borderRadius: 4,
  padding: "2px 6px",
  fontSize: 11,
  fontFamily: "inherit",
  minWidth: 160,
};

// Primary CTA — visually louder than the manual "run" button so a new
// user's eye lands on it first. Purple gradient picks up the ✨ vibe.
const autoButton: React.CSSProperties = {
  background: "linear-gradient(135deg, #7c3aed 0%, #c026d3 100%)",
  color: "#fff",
  border: "1px solid #9333ea",
  borderRadius: 4,
  padding: "3px 12px",
  fontSize: 12,
  fontWeight: 600,
  cursor: "pointer",
  fontFamily: "inherit",
};

const divider: React.CSSProperties = {
  color: "#6b7280",
  fontSize: 11,
  padding: "0 4px",
  fontStyle: "italic",
};

const errStyle: React.CSSProperties = {
  color: "#ef4444",
  fontSize: 11,
};

const okStyle: React.CSSProperties = {
  color: "#86efac",
  fontSize: 11,
};
