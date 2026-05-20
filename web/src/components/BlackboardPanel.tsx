/**
 * BlackboardPanel — list paths, read + write markdown via /api/blackboard.
 *
 * Live updates: when the parent forwards a `blackboard_changed` event via
 * `liveChange`, we refresh the path list. If the changed path equals the
 * one we're currently editing AND the local buffer is clean, we silently
 * pull the new content; if dirty, we surface a warning instead of clobbering.
 */

import { useEffect, useMemo, useState } from "react";
import { api } from "../api/http";
import type { BlackboardEntry } from "../api/types";

interface Props {
  /** Latest swarm `blackboard_changed` event observed by the parent. */
  liveChange: { path: string; agent_id: string | null; op: string } | null;
}

export function BlackboardPanel({ liveChange }: Props) {
  const [entries, setEntries] = useState<BlackboardEntry[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [content, setContent] = useState("");
  const [originalContent, setOriginalContent] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [info, setInfo] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [newPath, setNewPath] = useState("");

  const isDirty = useMemo(() => content !== originalContent, [content, originalContent]);

  const refreshList = async () => {
    try {
      const rows = await api.listBlackboard();
      setEntries(rows);
      setError(null);
    } catch (e) {
      setError((e as Error).message);
    }
  };

  const openPath = async (path: string) => {
    if (isDirty && selected && selected !== path) {
      const ok = confirm(`Discard unsaved changes to "${selected}"?`);
      if (!ok) return;
    }
    setSelected(path);
    setInfo(null);
    try {
      const snap = await api.readBlackboard(path);
      setContent(snap.content);
      setOriginalContent(snap.content);
      setError(null);
    } catch (e) {
      setError((e as Error).message);
      setContent("");
      setOriginalContent("");
    }
  };

  const save = async () => {
    if (!selected) return;
    setSaving(true);
    try {
      await api.writeBlackboard(selected, { content });
      setOriginalContent(content);
      setInfo(`saved ${selected}`);
      await refreshList();
      setError(null);
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setSaving(false);
    }
  };

  const createNew = async () => {
    const path = newPath.trim();
    if (!path) return;
    try {
      await api.writeBlackboard(path, { content: "" });
      setNewPath("");
      await refreshList();
      await openPath(path);
      setError(null);
    } catch (e) {
      setError((e as Error).message);
    }
  };

  useEffect(() => {
    refreshList();
  }, []);

  useEffect(() => {
    if (!liveChange) return;
    refreshList();
    if (selected && liveChange.path === selected) {
      if (isDirty) {
        setInfo(
          `⚠ ${liveChange.path} changed on disk (op=${liveChange.op}) — local edits unsaved`,
        );
      } else {
        // Silently refresh the buffer.
        api
          .readBlackboard(selected)
          .then((snap) => {
            setContent(snap.content);
            setOriginalContent(snap.content);
            setInfo(`refreshed from ${liveChange.op}`);
          })
          .catch(() => {
            /* ignore */
          });
      }
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [liveChange]);

  return (
    <div style={{ display: "flex", height: "100%", minHeight: 0 }}>
      <div style={leftCol}>
        <div style={headerRow}>
          <input
            value={newPath}
            onChange={(e) => setNewPath(e.target.value)}
            placeholder="new path (e.g. tasks.md)"
            style={{ ...input, flex: 1 }}
            onKeyDown={(e) => {
              if (e.key === "Enter") createNew();
            }}
          />
          <button onClick={createNew} disabled={!newPath.trim()} title="create">
            +
          </button>
          <button onClick={refreshList} title="refresh">
            ↻
          </button>
        </div>
        <div style={pathList}>
          {entries.length === 0 && <div style={emptyHint}>No paths yet.</div>}
          {entries.map((e) => (
            <button
              key={e.path}
              onClick={() => openPath(e.path)}
              style={{
                ...pathRow,
                background: e.path === selected ? "#1e3a8a" : "transparent",
              }}
              title={`${e.op} · ${formatTime(e.at)} · ${e.sha256.slice(0, 8)}`}
            >
              {e.path}
            </button>
          ))}
        </div>
      </div>

      <div style={rightCol}>
        {selected ? (
          <>
            <div style={headerRow}>
              <span style={{ flex: 1, fontSize: 12, color: "#cbd5f5" }}>
                {selected} {isDirty && <em style={{ color: "#fbbf24" }}>(unsaved)</em>}
              </span>
              <button onClick={save} disabled={saving || !isDirty}>
                save
              </button>
            </div>
            {info && <div style={infoRow}>{info}</div>}
            <textarea
              value={content}
              onChange={(e) => setContent(e.target.value)}
              style={editor}
              spellCheck={false}
            />
          </>
        ) : (
          <div style={emptyHint}>Select a path on the left.</div>
        )}
        {error && <div style={errorRow}>{error}</div>}
      </div>
    </div>
  );
}

function formatTime(ms: number): string {
  return new Date(ms).toLocaleString();
}

const leftCol: React.CSSProperties = {
  width: 160,
  display: "flex",
  flexDirection: "column",
  borderRight: "1px solid #374151",
  minHeight: 0,
};

const rightCol: React.CSSProperties = {
  flex: 1,
  display: "flex",
  flexDirection: "column",
  minWidth: 0,
  minHeight: 0,
};

const headerRow: React.CSSProperties = {
  display: "flex",
  alignItems: "center",
  gap: 4,
  padding: "6px 8px",
  borderBottom: "1px solid #374151",
};

const pathList: React.CSSProperties = {
  flex: 1,
  overflowY: "auto",
  display: "flex",
  flexDirection: "column",
};

const pathRow: React.CSSProperties = {
  textAlign: "left",
  padding: "4px 8px",
  fontSize: 12,
  border: "none",
  borderBottom: "1px solid #1f2937",
  color: "#cbd5f5",
  cursor: "pointer",
  whiteSpace: "nowrap",
  overflow: "hidden",
  textOverflow: "ellipsis",
};

const input: React.CSSProperties = {
  background: "#0b1220",
  color: "#e2e8f0",
  border: "1px solid #374151",
  borderRadius: 4,
  padding: "4px 6px",
  fontSize: 12,
  fontFamily: "inherit",
};

const editor: React.CSSProperties = {
  flex: 1,
  background: "#0b1220",
  color: "#e2e8f0",
  border: "none",
  padding: "8px",
  fontSize: 12,
  fontFamily:
    "ui-monospace, SFMono-Regular, Menlo, Consolas, 'Liberation Mono', monospace",
  resize: "none",
  outline: "none",
  minHeight: 0,
};

const emptyHint: React.CSSProperties = {
  color: "#64748b",
  fontSize: 12,
  textAlign: "center",
  marginTop: 16,
  padding: "0 8px",
};

const errorRow: React.CSSProperties = {
  color: "#fca5a5",
  fontSize: 11,
  padding: "4px 8px",
  background: "#1f2937",
};

const infoRow: React.CSSProperties = {
  color: "#86efac",
  fontSize: 11,
  padding: "4px 8px",
  background: "#1f2937",
};
