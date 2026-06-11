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
import {
  ConfirmActionDialog,
  type ConfirmActionState,
} from "@/components/ConfirmActionDialog";

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
  // M6d-4: HITL gate quick-actions. Only meaningful when the user has
  // `design.md` selected (the architect's draft from a `fullstack-
  // feature-gated` run). Approve writes `design.approved` with a
  // sentinel body; Reject opens an inline reason input and writes
  // `design.rejected` with `{"reason": ...}` (which architect is
  // subscribed to — it wakes and revises).
  const [rejectOpen, setRejectOpen] = useState(false);
  const [rejectReason, setRejectReason] = useState("");
  const [gateBusy, setGateBusy] = useState(false);
  const [discardConfirm, setDiscardConfirm] = useState<ConfirmActionState | null>(null);

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

  const loadPath = async (path: string) => {
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

  const openPath = async (path: string) => {
    if (isDirty && selected && selected !== path) {
      setDiscardConfirm({
        title: "放弃未保存的修改？",
        description: `“${selected}”还有未保存内容，切换到“${path}”会丢弃这些修改。`,
        confirmLabel: "放弃修改",
        variant: "destructive",
        onConfirm: () => loadPath(path),
      });
      return;
    }
    await loadPath(path);
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
      setInfo(`已保存 ${selected}`);
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

  // M6d-4: write the architect-approval companion to whatever the
  // operator just read. Body is intentionally minimal — the gate
  // mechanism only cares that the key exists with non-empty content;
  // anything more is for human eyes.
  const approveDesign = async () => {
    setGateBusy(true);
    try {
      await api.writeBlackboard("design.approved", {
        content: "approved via UI",
      });
      setError(null);
      await refreshList();
      // setInfo only after refreshList confirms the write is visible (was:
      // claimed before the round-trip). Wake is the coordinator's real
      // design.approved subscription, so the phrasing is honest.
      setInfo("已写入 design.approved · 前后端 agent 会据此醒来开工");
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setGateBusy(false);
    }
  };

  // M6d-4: companion to approveDesign for the M6d-2 revision loop.
  // The body MUST be the JSON shape the architect's prompt expects
  // (`{"reason": "..."}`) — architect's wake handler reads it and
  // rewrites design.md addressing the feedback.
  const rejectDesign = async () => {
    const reason = rejectReason.trim();
    if (!reason) return;
    setGateBusy(true);
    try {
      await api.writeBlackboard("design.rejected", {
        content: JSON.stringify({ reason }, null, 2),
      });
      setError(null);
      setRejectOpen(false);
      setRejectReason("");
      await refreshList();
      // Claim success only AFTER refreshList confirms the write actually
      // landed (was: setInfo fired before the round-trip, asserting a
      // persistence that hadn't been verified). The architect picks this up via
      // the wake-coordinator's design.rejected subscription — a real mechanism,
      // so the phrasing states what's now certain rather than a bare promise.
      setInfo("已记录拒绝意见 · architect 会据此修订方案");
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setGateBusy(false);
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
          `⚠ ${liveChange.path} 在磁盘上发生变化 (op=${liveChange.op}) — 你有未保存的修改`,
        );
      } else {
        // Silently refresh the buffer.
        api
          .readBlackboard(selected)
          .then((snap) => {
            setContent(snap.content);
            setOriginalContent(snap.content);
            setInfo(`已根据 ${liveChange.op} 刷新`);
          })
          .catch(() => {
            /* ignore */
          });
      }
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [liveChange]);

  return (
    <>
    <div style={{ display: "flex", height: "100%", minHeight: 0 }}>
      <div style={leftCol}>
        <div style={headerRow}>
          <input
            name="blackboard-new-path"
            value={newPath}
            onChange={(e) => setNewPath(e.target.value)}
            placeholder="新文件路径（如 tasks.md）"
            style={{ ...input, flex: 1 }}
            onKeyDown={(e) => {
              if (e.key === "Enter") createNew();
            }}
          />
          <button onClick={createNew} disabled={!newPath.trim()} title="创建">
            +
          </button>
          <button onClick={refreshList} title="刷新">
            ↻
          </button>
        </div>
        <div style={pathList}>
          {entries.length === 0 && <div style={emptyHint}>暂无文件</div>}
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
                {selected} {isDirty && <em style={{ color: "#fbbf24" }}>(未保存)</em>}
              </span>
              {selected === "design.md" && (
                <>
                  <button
                    onClick={approveDesign}
                    disabled={gateBusy}
                    title="批准该设计 — 写入 design.approved，唤醒前后端"
                    style={approveBtn}
                  >
                    ✓ 通过
                  </button>
                  <button
                    onClick={() => setRejectOpen((v) => !v)}
                    disabled={gateBusy}
                    title="要求重做 — 写入 design.rejected 并附原因；architect 会重新起稿"
                    style={rejectBtn}
                  >
                    ✗ 驳回
                  </button>
                </>
              )}
              <button
                onClick={() => setHistoryOpen((v) => !v)}
                title="查看写入历史"
                style={historyToggle}
              >
                历史 ({history.length}
                {historyLoading ? "…" : ""})
              </button>
              <button onClick={save} disabled={saving || !isDirty}>
                保存
              </button>
            </div>
            {selected === "design.md" && rejectOpen && (
              <div style={rejectInlineRow}>
                <input
                  name="blackboard-reject-reason"
                  type="text"
                  value={rejectReason}
                  onChange={(e) => setRejectReason(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" && rejectReason.trim()) {
                      rejectDesign();
                    } else if (e.key === "Escape") {
                      setRejectOpen(false);
                      setRejectReason("");
                    }
                  }}
                  placeholder="驳回原因（一句话）— architect 会读这条并改稿"
                  style={rejectInput}
                  autoFocus
                />
                <button
                  onClick={rejectDesign}
                  disabled={gateBusy || !rejectReason.trim()}
                  style={rejectConfirmBtn}
                >
                  发送驳回
                </button>
                <button
                  onClick={() => {
                    setRejectOpen(false);
                    setRejectReason("");
                  }}
                  style={historyToggle}
                >
                  取消
                </button>
              </div>
            )}
            {historyOpen && (
              <div style={historyDrawer}>
                {history.length === 0 && (
                  <div style={{ ...emptyHint, marginTop: 4 }}>暂无历史</div>
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
                        {h.agent_id ?? "外部"}
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
                    正在查看版本 {versionPreview.sha256.slice(0, 12)}…
                    （{versionPreview.agent_id ?? "外部"} · {" "}
                    {new Date(versionPreview.at).toLocaleString()}）
                  </span>
                  <button onClick={() => setVersionPreview(null)}>
                    关闭
                  </button>
                </div>
                <textarea
                  value={versionPreview.content ?? "(尚未加载内容)"}
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
          <div style={emptyHint}>请在左侧选择一个文件</div>
        )}
        {error && <div style={errorRow}>{error}</div>}
      </div>
    </div>
    <ConfirmActionDialog
      action={discardConfirm}
      onOpenChange={(open) => {
        if (!open) setDiscardConfirm(null);
      }}
    />
    </>
  );
}

function formatRelative(ms: number): string {
  const diff = Date.now() - ms;
  if (diff < 60_000) return "刚刚";
  if (diff < 3600_000) return `${Math.floor(diff / 60_000)} 分钟前`;
  if (diff < 86400_000) return `${Math.floor(diff / 3600_000)} 小时前`;
  return `${Math.floor(diff / 86400_000)} 天前`;
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

// M6d-4: HITL gate quick-action buttons. Approve is green (the
// loud-and-positive default action when the operator's read the
// design and is happy). Reject is a quieter outline-only red — the
// user is asking for a revision, not erroring out, so it shouldn't
// look like a destructive action.
const approveBtn: React.CSSProperties = {
  background: "#16a34a",
  color: "#fff",
  border: "1px solid #15803d",
  borderRadius: 4,
  fontSize: 11,
  fontWeight: 600,
  padding: "2px 8px",
  cursor: "pointer",
};

const rejectBtn: React.CSSProperties = {
  background: "transparent",
  color: "#f87171",
  border: "1px solid #b91c1c",
  borderRadius: 4,
  fontSize: 11,
  fontWeight: 600,
  padding: "2px 8px",
  cursor: "pointer",
};

const rejectInlineRow: React.CSSProperties = {
  display: "flex",
  gap: 6,
  padding: "6px 8px",
  borderBottom: "1px solid #374151",
  background: "#1a0f0f",
  alignItems: "center",
};

const rejectInput: React.CSSProperties = {
  flex: 1,
  background: "#0b1220",
  color: "#e2e8f0",
  border: "1px solid #b91c1c",
  borderRadius: 4,
  padding: "3px 6px",
  fontSize: 12,
  fontFamily: "inherit",
};

const rejectConfirmBtn: React.CSSProperties = {
  background: "#b91c1c",
  color: "#fff",
  border: "1px solid #7f1d1d",
  borderRadius: 4,
  fontSize: 11,
  fontWeight: 600,
  padding: "2px 8px",
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
