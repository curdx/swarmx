/**
 * RecordingsPanel — lists asciicast v2 files captured by flockmux-recorder.
 *
 * Each row exposes a "▶ play" toggle that expands an inline asciinema-player
 * (WASM-backed terminal session player from the official npm package). The
 * "raw .cast" / "download" links remain as escape hatches for offline
 * inspection. Only one player is mounted per row at a time; closing the row
 * disposes the player's WASM/canvas resources via the wrapper's cleanup.
 */

import { useEffect, useState } from "react";
import { api } from "../api/http";
import { downloadRecordingCast } from "@/lib/download";
import type { RecordingInfo } from "../api/types";
import { AsciicastPlayer } from "./AsciicastPlayer";

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
  // M6e: 录像现在弹全屏 modal 播放（而不是在 360px 侧栏里展开成
  // 蚂蚁字大小）。playingId = 当前要全屏播放的录像 id；null = 关闭。
  // 单实例：每个 player 拿一个 WASM context，开多个浪费资源。
  const [playingId, setPlayingId] = useState<string | null>(null);

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

  // M6e: Esc 关闭全屏播放器
  useEffect(() => {
    if (!playingId) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setPlayingId(null);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [playingId]);

  const playing = playingId ? items.find((r) => r.id === playingId) : null;

  return (
    <div style={{ display: "flex", flexDirection: "column", height: "100%" }}>
      <div style={headerRow}>
        <input
          name="recording-agent-filter"
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          placeholder="按 agent_id 过滤"
          style={{ ...input, flex: 1 }}
          onKeyDown={(e) => {
            if (e.key === "Enter") refresh();
          }}
        />
        <button onClick={refresh} title="刷新">
          ↻
        </button>
      </div>
      {error && <div style={errorRow}>{error}</div>}
      <div style={listStyle}>
        {items.length === 0 && <div style={emptyHint}>暂无录像</div>}
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
                  {live ? "● 实时" : "○ 已完结"}
                </span>
              </div>
              <div style={{ fontSize: 10, color: "#94a3b8" }}>
                开始于 {formatTime(r.started_at)} · {r.cols}×{r.rows}
                {r.duration_ms != null && (
                  <> · {formatDuration(r.duration_ms)}</>
                )}
                {r.last_seq != null && <> · {r.last_seq} B</>}
              </div>
              <div style={{ display: "flex", gap: 4, marginTop: 4 }}>
                <button
                  onClick={() => setPlayingId(r.id)}
                  style={linkButton}
                  title={
                    live
                      ? "实时录像可以回放，但只能播到已写入的字节为止"
                      : "回放这条录像（全屏）"
                  }
                >
                  ▶ 播放
                </button>
                <a
                  href={api.recordingCastUrl(r.id)}
                  target="_blank"
                  rel="noreferrer"
                  style={linkButton}
                >
                  原始 .cast
                </a>
                <button
                  type="button"
                  onClick={() => downloadRecordingCast(r.id)}
                  style={linkButton}
                >
                  下载
                </button>
              </div>
            </div>
          );
        })}
      </div>
      {/* M6e: 全屏 modal 播放器。塞在 360px 侧栏里 120 列文字会变成蚂蚁，
          点 ▶ 播放后改成铺满主区域的 overlay；按 Esc 或点右上 × 关掉。 */}
      {playing && (
        <div style={modalBackdrop} onClick={() => setPlayingId(null)}>
          <div style={modalCard} onClick={(e) => e.stopPropagation()}>
            <div style={modalHeader}>
              <span style={{ flex: 1, color: "#cbd5f5", fontSize: 13 }}>
                <strong>{playing.id}</strong>
                <span style={{ color: "#64748b", marginLeft: 8 }}>
                  · {playing.agent_id} · {playing.cols}×{playing.rows}
                </span>
                {playing.duration_ms != null && (
                  <span style={{ color: "#64748b", marginLeft: 8 }}>
                    · {formatDuration(playing.duration_ms)}
                  </span>
                )}
              </span>
              <button
                onClick={() => setPlayingId(null)}
                style={modalCloseBtn}
                title="关闭（Esc）"
              >
                × 关闭
              </button>
            </div>
            <div style={modalPlayerHost}>
              <AsciicastPlayer
                src={api.recordingCastUrl(playing.id)}
                cols={playing.cols}
                rows={playing.rows}
                autoPlay
              />
            </div>
            <div style={modalFooterHint}>
              快捷键：<kbd>空格</kbd> 播放/暂停 · <kbd>f</kbd> 全屏 · <kbd>Esc</kbd> 关闭
            </div>
          </div>
        </div>
      )}
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

// M6e: 全屏播放 modal 的 backdrop —— position: fixed 覆盖整个 viewport，
// z-index 提到最高（pane 终端 + 各种侧栏都在下面），点击空白处关闭。
const modalBackdrop: React.CSSProperties = {
  position: "fixed",
  inset: 0,
  background: "rgba(0, 0, 0, 0.7)",
  zIndex: 9999,
  display: "flex",
  alignItems: "center",
  justifyContent: "center",
  padding: 24,
};

const modalCard: React.CSSProperties = {
  background: "#0b1220",
  border: "1px solid #374151",
  borderRadius: 8,
  width: "min(1200px, 95vw)",
  maxHeight: "90vh",
  display: "flex",
  flexDirection: "column",
  overflow: "hidden",
};

const modalHeader: React.CSSProperties = {
  display: "flex",
  alignItems: "center",
  gap: 8,
  padding: "10px 14px",
  borderBottom: "1px solid #374151",
  background: "#111827",
};

const modalCloseBtn: React.CSSProperties = {
  background: "transparent",
  color: "#cbd5f5",
  border: "1px solid #374151",
  borderRadius: 4,
  padding: "4px 10px",
  fontSize: 12,
  cursor: "pointer",
};

const modalPlayerHost: React.CSSProperties = {
  flex: 1,
  minHeight: 0,
  padding: 16,
  background: "#0d0d0d",
  display: "flex",
  alignItems: "center",
  justifyContent: "center",
  overflow: "auto",
};

const modalFooterHint: React.CSSProperties = {
  fontSize: 11,
  color: "#64748b",
  padding: "8px 14px",
  borderTop: "1px solid #374151",
  background: "#111827",
};
