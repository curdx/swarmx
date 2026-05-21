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

  const run = async () => {
    if (!selected || !task.trim()) return;
    setBusy(true);
    setError(null);
    setLastRun(null);
    try {
      const resp = await api.runSpell({ name: selected, task: task.trim() });
      setLastRun(
        `spawned ${resp.agents.length} agent(s): ${resp.agents
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

  const current = spells.find((s) => s.name === selected);

  return (
    <div style={wrap}>
      <select
        value={selected}
        onChange={(e) => setSelected(e.target.value)}
        style={select}
        disabled={busy}
        title={current?.description}
      >
        {spells.map((s) => (
          <option key={s.name} value={s.name}>
            ✨ {s.name} ({s.agents.map((a) => `${a.role}:${a.cli}`).join(" → ")})
          </option>
        ))}
      </select>
      <input
        type="text"
        value={task}
        onChange={(e) => setTask(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") run();
        }}
        placeholder="task description…"
        style={input}
        disabled={busy}
      />
      <button onClick={run} disabled={busy || !task.trim()} title="run spell">
        {busy ? "running…" : "run"}
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

const errStyle: React.CSSProperties = {
  color: "#ef4444",
  fontSize: 11,
};

const okStyle: React.CSSProperties = {
  color: "#86efac",
  fontSize: 11,
};
