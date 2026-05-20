/**
 * RecordingsPanel — lists asciicast v2 files captured by flockmux-recorder.
 *
 * No in-browser playback yet (M5 lands `asciinema-player`); for now each row
 * exposes a "raw .cast" link so users can download / inspect the file. The
 * server serves it as `application/x-asciicast`, which browsers typically
 * treat as a download.
 */

import { useEffect, useState } from "react";
import { api } from "../api/http";
import type { RecordingInfo } from "../api/types";

interface Props {
  /** Bump this whenever the parent sees a swarm event that may affect the
   *  recording list (e.g. `agent_state=exited` ⇒ a recording may have just
   *  been finalized). */
  refreshTick: number;
}

export function RecordingsPanel({ refreshTick }: Props) {
  const [items, setItems] = useState<RecordingInfo[]>([]);
  const [filter, setFilter] = useState("");
  const [error, setError] = useState<string | null>(null);

  const refresh = async () => {
    try {
      const rows = await api.listRecordings(
        filter.trim() ? filter.trim() : undefined,
      );
      setItems(rows);
      setError(null);
    } catch (e) {
      setError((e as Error).message);
    }
  };

  useEffect(() => {
    refresh();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [refreshTick]);

  return (
    <div style={{ display: "flex", flexDirection: "column", height: "100%" }}>
      <div style={headerRow}>
        <input
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          placeholder="filter by agent_id"
          style={{ ...input, flex: 1 }}
          onKeyDown={(e) => {
            if (e.key === "Enter") refresh();
          }}
        />
        <button onClick={refresh} title="refresh">
          ↻
        </button>
      </div>
      {error && <div style={errorRow}>{error}</div>}
      <div style={listStyle}>
        {items.length === 0 && <div style={emptyHint}>No recordings yet.</div>}
        {items.map((r) => {
          const live = r.finalized_at == null;
          return (
            <div key={r.id} style={row}>
              <div style={{ display: "flex", justifyContent: "space-between" }}>
                <span style={{ color: "#cbd5f5", fontSize: 12 }}>
                  <strong>{r.id}</strong>{" "}
                  <span style={{ color: "#64748b" }}>· {r.agent_id}</span>
                </span>
                <span
                  style={{
                    fontSize: 10,
                    color: live ? "#fbbf24" : "#86efac",
                  }}
                >
                  {live ? "● live" : "○ finalized"}
                </span>
              </div>
              <div style={{ fontSize: 10, color: "#94a3b8" }}>
                started {formatTime(r.started_at)} · {r.cols}×{r.rows}
                {r.duration_ms != null && (
                  <> · {formatDuration(r.duration_ms)}</>
                )}
                {r.last_seq != null && <> · {r.last_seq}B</>}
              </div>
              <div style={{ display: "flex", gap: 4, marginTop: 4 }}>
                <a
                  href={api.recordingCastUrl(r.id)}
                  target="_blank"
                  rel="noreferrer"
                  style={linkButton}
                >
                  raw .cast
                </a>
                <a
                  href={api.recordingCastUrl(r.id)}
                  download={`${r.id}.cast`}
                  style={linkButton}
                >
                  download
                </a>
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}

function formatTime(ms: number): string {
  return new Date(ms).toLocaleString();
}

function formatDuration(ms: number): string {
  if (ms < 1000) return `${ms}ms`;
  const s = ms / 1000;
  if (s < 60) return `${s.toFixed(1)}s`;
  const m = Math.floor(s / 60);
  const rem = (s - m * 60).toFixed(0);
  return `${m}m${rem}s`;
}

const headerRow: React.CSSProperties = {
  display: "flex",
  gap: 4,
  padding: "6px 8px",
  borderBottom: "1px solid #374151",
};

const listStyle: React.CSSProperties = {
  flex: 1,
  overflowY: "auto",
  padding: "6px 8px",
  display: "flex",
  flexDirection: "column",
  gap: 6,
  minHeight: 0,
};

const row: React.CSSProperties = {
  borderLeft: "2px solid #374151",
  paddingLeft: 6,
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

const linkButton: React.CSSProperties = {
  fontSize: 11,
  padding: "2px 6px",
  background: "#1f2937",
  color: "#cbd5f5",
  border: "1px solid #374151",
  borderRadius: 4,
  textDecoration: "none",
};

const errorRow: React.CSSProperties = {
  color: "#fca5a5",
  fontSize: 11,
  padding: "4px 8px",
  background: "#1f2937",
};

const emptyHint: React.CSSProperties = {
  color: "#64748b",
  fontSize: 12,
  textAlign: "center",
  marginTop: 16,
};
