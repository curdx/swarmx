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
import type { BlackboardEntry, BlackboardHistoryEntry } from "../api/types";

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
  const [historyOpen, setHistoryOpen] = useState(false);
  const [history, setHistory] = useState<BlackboardHistoryEntry[]>([]);
  const [historyLoading, setHistoryLoading] = useState(false);
  const [versionPreview, setVersionPreview] = useState<BlackboardHistoryEntry | null>(null);

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

  const refreshHistory = async (path: string) => {
    setHistoryLoading(true);
    try {
      const rows = await api.listBlackboardHistory(path, 50, false);
      setHistory(rows);
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setHistoryLoading(false);
    }
  };

  const openPath = async (path: string) => {
    if (isDirty && selected && selected !== path) {
      const ok = confirm(`Discard unsaved changes to "${selected}"?`);
      if (!ok) return;
    }
    setSelected(path);
    setInfo(null);
    setVersionPreview(null);
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
    // Best-effort: history count drives the toggle label; failures are
    // logged into `error` but don't block the editor.
    refreshHistory(path);
  };

  const openVersion = async (entry: BlackboardHistoryEntry) => {
    // The list call strips content by default; fetch the full row for the
    // selected version. We re-query the same endpoint with include_content
    // so each click is at most one byte-heavy request.
    setVersionPreview(entry);
    try {
      const rows = await api.listBlackboardHistory(entry.path, 200, true);
      const full = rows.find((r) => r.id === entry.id);
      if (full) setVersionPreview(full);
    } catch (e) {
      setError((e as Error).message);
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
      refreshHistory(selected);
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
              <button
                onClick={() => setHistoryOpen((v) => !v)}
                title="show write history"
                style={historyToggle}
              >
                history ({history.length}
                {historyLoading ? "…" : ""})
              </button>
              <button onClick={save} disabled={saving || !isDirty}>
                save
              </button>
            </div>
            {historyOpen && (
              <div style={historyDrawer}>
                {history.length === 0 && (
                  <div style={{ ...emptyHint, marginTop: 4 }}>No history.</div>
                )}
                {history.map((h) => {
                  const isPreview = versionPreview?.id === h.id;
                  return (
                    <button
                      key={h.id}
                      onClick={() => openVersion(h)}
                      style={{
                        ...historyRow,
                        background: isPreview ? "#1e3a8a" : "transparent",
                      }}
                      title={new Date(h.at).toLocaleString()}
                    >
                      <span style={{ color: "#fbbf24" }}>
                        {h.sha256.slice(0, 12)}
                      </span>
                      <span style={{ color: "#94a3b8", marginLeft: 6 }}>
                        {h.agent_id ?? "external"}
                      </span>
                      <span style={{ color: "#64748b", marginLeft: 6 }}>
                        {h.op}
                      </span>
                      <span style={{ color: "#64748b", marginLeft: 6 }}>
                        {formatRelative(h.at)}
                      </span>
                    </button>
                  );
                })}
              </div>
            )}
            {info && <div style={infoRow}>{info}</div>}
            {versionPreview ? (
              <>
                <div style={versionHeader}>
                  <span style={{ flex: 1 }}>
                    viewing version {versionPreview.sha256.slice(0, 12)}…
                    ({versionPreview.agent_id ?? "external"},{" "}
                    {new Date(versionPreview.at).toLocaleString()})
                  </span>
                  <button onClick={() => setVersionPreview(null)}>
                    close
                  </button>
                </div>
                <textarea
                  value={versionPreview.content ?? "(content not fetched)"}
                  readOnly
                  style={{ ...editor, background: "#101a2f" }}
                  spellCheck={false}
                />
              </>
            ) : (
              <textarea
                value={content}
                onChange={(e) => setContent(e.target.value)}
                style={editor}
                spellCheck={false}
              />
            )}
          </>
        ) : (
          <div style={emptyHint}>Select a path on the left.</div>
        )}
        {error && <div style={errorRow}>{error}</div>}
      </div>
    </div>
  );
}

function formatRelative(ms: number): string {
  const diff = Date.now() - ms;
  if (diff < 60_000) return "just now";
  if (diff < 3600_000) return `${Math.floor(diff / 60_000)}m ago`;
  if (diff < 86400_000) return `${Math.floor(diff / 3600_000)}h ago`;
  return `${Math.floor(diff / 86400_000)}d ago`;
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

const historyToggle: React.CSSProperties = {
  background: "transparent",
  color: "#94a3b8",
  border: "1px solid #374151",
  borderRadius: 4,
  fontSize: 11,
  padding: "2px 6px",
  cursor: "pointer",
};

const historyDrawer: React.CSSProperties = {
  borderBottom: "1px solid #374151",
  maxHeight: 180,
  overflowY: "auto",
  background: "#0b1220",
  display: "flex",
  flexDirection: "column",
};

const historyRow: React.CSSProperties = {
  textAlign: "left",
  border: "none",
  borderBottom: "1px solid #1f2937",
  padding: "4px 8px",
  fontSize: 11,
  color: "#cbd5f5",
  cursor: "pointer",
  fontFamily:
    "ui-monospace, SFMono-Regular, Menlo, Consolas, 'Liberation Mono', monospace",
};

const versionHeader: React.CSSProperties = {
  display: "flex",
  alignItems: "center",
  gap: 4,
  padding: "4px 8px",
  background: "#1f2937",
  fontSize: 11,
  color: "#cbd5f5",
  borderBottom: "1px solid #374151",
};
