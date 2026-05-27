/**
 * XtermPane — one xterm.js instance + WebSocket bridge to /ws/pty/:agent_id.
 *
 *   onData (keystrokes) ──┐
 *                          ├─→ WebSocket /ws/pty/:agent_id
 *   ResizeObserver        │      ├─ binary: raw keystroke bytes
 *      └→ fitAddon.fit() ─┘      └─ text: ClientControl JSON
 *                          ←──── binary: [4B seq][PTY bytes]
 *                                text:   ServerControl JSON
 *
 * M2 renderer policy: WebGL is gated through `lib/webglPool` so the page
 * stays under the browser's ~16-context limit. Panes that don't get a slot
 * (cap reached, in cooldown after a context-loss event, or `visible=false`)
 * fall back to xterm's DOM renderer — slower but never lost. Slots are
 * released the instant a pane goes hidden and re-acquired on show.
 *
 * Other notes:
 *   - Unicode 11 + FitAddon + WebLinks load once at mount.
 *   - seq monotonicity + resume: lastSeq is persisted in sessionStorage so a
 *     remount (StrictMode dev, tab switch via portal, page refresh) reconnects
 *     with `?last_seq=N` and replays the gap from the server's per-agent ring
 *     buffer. If the server reports a gap (Hello.seq_start > lastSeq+1) we
 *     clear local scrollback — the missing range is unrecoverable.
 *   - ACK is batched every 5KB or 50ms (server-side currently ignores
 *     them; harmless to send).
 */

import { useEffect, useRef, useState } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { Unicode11Addon } from "@xterm/addon-unicode11";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { WebglAddon } from "@xterm/addon-webgl";
import type { ServerControl, ClientControl } from "../api/types";
import { acquireSlot, releaseSlot, reportContextLoss } from "../lib/webglPool";
import { inputPolicyFor } from "../lib/cliInputPolicy";
import { WS_HOST, WS_PROTO } from "../lib/apiBase";

interface Props {
  agentId: string;
  /**
   * When false the pane is hidden (display:none or behind another maximised
   * pane). We dispose the WebGL addon and release its pool slot so other
   * panes can take it; on next `true` we try to reacquire.
   */
  visible?: boolean;
  onShimReady?: () => void;
  onShimExit?: (code: number) => void;
}

const ACK_BYTES = 5 * 1024;
const ACK_INTERVAL_MS = 50;
const RESIZE_DEBOUNCE_MS = 150;

const lastSeqKey = (agentId: string) => `flockmux:lastSeq:${agentId}`;
const readLastSeq = (agentId: string): number => {
  try {
    const v = window.sessionStorage.getItem(lastSeqKey(agentId));
    if (!v) return 0;
    const n = parseInt(v, 10);
    return Number.isFinite(n) && n >= 0 ? n : 0;
  } catch {
    return 0;
  }
};
const writeLastSeq = (agentId: string, seq: number) => {
  try {
    window.sessionStorage.setItem(lastSeqKey(agentId), String(seq));
  } catch {
    /* sessionStorage may be unavailable */
  }
};
const clearLastSeq = (agentId: string) => {
  try {
    window.sessionStorage.removeItem(lastSeqKey(agentId));
  } catch {
    /* noop */
  }
};

export function XtermPane({
  agentId,
  visible = true,
  onShimReady,
  onShimExit,
}: Props) {
  const hostRef = useRef<HTMLDivElement | null>(null);
  const termRef = useRef<Terminal | null>(null);
  const webglRef = useRef<WebglAddon | null>(null);
  const fitAddonRef = useRef<FitAddon | null>(null);
  const [status, setStatus] = useState<
    "connecting" | "spawning" | "ready" | "exited" | "error"
  >("connecting");
  const [statusDetail, setStatusDetail] = useState<string>("");

  useEffect(() => {
    if (!hostRef.current) return;
    const host = hostRef.current;

    // Disposed flag — guards against React 18 StrictMode (mount-unmount-remount
    // in dev). Async WS callbacks from the first mount must NOT mutate state
    // belonging to the second mount.
    let disposed = false;

    const term = new Terminal({
      allowProposedApi: true,
      cursorBlink: true,
      fontFamily:
        '"JetBrainsMono Nerd Font Mono", Menlo, Monaco, "Courier New", monospace',
      fontSize: 13,
      theme: { background: "#0d0d0d", foreground: "#f0f0f0" },
      scrollback: 5000,
    });

    const fitAddon = new FitAddon();
    term.loadAddon(fitAddon);
    term.loadAddon(new Unicode11Addon());
    term.unicode.activeVersion = "11";
    term.loadAddon(new WebLinksAddon());

    term.open(host);

    termRef.current = term;
    fitAddonRef.current = fitAddon;
    // WebGL is attached/detached by the `visible`-driven effect below — DOM
    // is the default renderer until that effect grants a pool slot.

    // Defer first fit() until host actually has layout dimensions — calling
    // it synchronously after term.open() blows up with "dimensions undefined"
    // when the host is still being laid out by the parent flex/grid.
    const safeFit = () => {
      if (disposed) return;
      if (host.clientWidth === 0 || host.clientHeight === 0) return;
      try {
        fitAddon.fit();
      } catch (err) {
        // eslint-disable-next-line no-console
        console.warn(`[XtermPane ${agentId}] fit() failed`, err);
      }
    };
    requestAnimationFrame(safeFit);

    // ---- WebSocket bridge ------------------------------------------------
    const resumeFrom = readLastSeq(agentId);
    const qs = resumeFrom > 0 ? `?last_seq=${resumeFrom}` : "";
    const wsUrl = `${WS_PROTO}//${WS_HOST}/ws/pty/${encodeURIComponent(agentId)}${qs}`;
    const ws = new WebSocket(wsUrl);
    ws.binaryType = "arraybuffer";

    let lastSeq = resumeFrom;
    let unackedBytes = 0;
    let ackTimer: number | null = null;
    let shimExited = false;

    // ---- per-CLI input gating ------------------------------------------
    // Keystrokes are dropped on the floor until the CLI is "really" ready:
    // shim_ready ⇒ true AND the per-CLI settle delay has elapsed. Codex's
    // TUI emits OSC_READY before its input poll is attached; bytes sent in
    // that window get swallowed by the startup banner, manifesting as
    // "first Enter is a newline, not a submit". Anything typed before that
    // moment is buffered (not dropped) so the user doesn't lose work.
    const inputPolicy = inputPolicyFor(agentId);
    let inputUnlocked = false;
    let settleTimer: number | null = null;
    const pendingChunks: Uint8Array[] = [];
    let pendingBytes = 0;

    const sendBytes = (bytes: Uint8Array) => {
      if (ws.readyState !== WebSocket.OPEN) return;
      ws.send(bytes);
    };

    const flushPendingInput = () => {
      if (pendingChunks.length === 0) return;
      let total = 0;
      for (const c of pendingChunks) total += c.byteLength;
      const combined = new Uint8Array(total);
      let off = 0;
      for (const c of pendingChunks) {
        combined.set(c, off);
        off += c.byteLength;
      }
      pendingChunks.length = 0;
      pendingBytes = 0;
      sendBytes(combined);
    };

    const unlockInput = () => {
      if (inputUnlocked) return;
      inputUnlocked = true;
      flushPendingInput();
    };

    const onShimReadySignal = () => {
      if (disposed || inputUnlocked) return;
      // Already had ready event (Hello/event); arm the settle timer once.
      if (settleTimer !== null) return;
      if (inputPolicy.postReadyDelayMs <= 0) {
        unlockInput();
        return;
      }
      settleTimer = window.setTimeout(() => {
        settleTimer = null;
        if (disposed) return;
        unlockInput();
      }, inputPolicy.postReadyDelayMs);
    };

    const sendCtrl = (msg: ClientControl) => {
      if (ws.readyState === WebSocket.OPEN) ws.send(JSON.stringify(msg));
    };

    const sendAck = () => {
      if (lastSeq === 0) return;
      sendCtrl({ type: "ack", seq: lastSeq });
      unackedBytes = 0;
      if (ackTimer !== null) {
        window.clearTimeout(ackTimer);
        ackTimer = null;
      }
    };

    ws.onopen = () => {
      if (disposed) return;
      setStatus("spawning");
    };
    ws.onerror = () => {
      if (disposed) return;
      setStatus("error");
      setStatusDetail("WebSocket error");
    };
    ws.onclose = (ev) => {
      if (disposed) return;
      // shim_exit / EOF (graceful) takes precedence over WS close codes.
      if (shimExited) return;
      setStatus("error");
      setStatusDetail(
        ev.reason ? `WS closed: ${ev.reason}` : `WS closed (code ${ev.code})`,
      );
    };

    ws.onmessage = (event) => {
      if (typeof event.data === "string") {
        try {
          const msg = JSON.parse(event.data) as ServerControl;
          handleServerControl(msg);
        } catch {
          // Non-JSON text: write as-is for hand-testing.
          term.write(event.data);
        }
        return;
      }
      // Binary frame: [4B BE seq][bytes...]
      const buf = event.data as ArrayBuffer;
      if (buf.byteLength < 4) return;
      const dv = new DataView(buf);
      const seq = dv.getUint32(0, false);
      const bytes = new Uint8Array(buf, 4);
      if (seq <= lastSeq) {
        // Duplicate / replay; skip but still update seq.
        // (Shouldn't happen in M1 since we don't replay.)
        // eslint-disable-next-line no-console
        console.warn(`[XtermPane ${agentId}] non-monotonic seq ${seq} ≤ ${lastSeq}`);
      } else {
        if (seq !== lastSeq + 1 && lastSeq !== 0) {
          // Gap detected mid-stream. Hello-driven gaps are already handled
          // (server inlines an Error frame + we reset scrollback), so this
          // path only fires if the ring buffer evicted bytes between
          // Hello and the first chunk — pathological but log it.
          // eslint-disable-next-line no-console
          console.warn(
            `[XtermPane ${agentId}] seq gap mid-stream: expected ${lastSeq + 1}, got ${seq}`,
          );
        }
        lastSeq = seq;
        writeLastSeq(agentId, seq);
      }
      term.write(bytes);

      // ACK throttle: every ACK_BYTES or ACK_INTERVAL_MS.
      unackedBytes += bytes.byteLength;
      if (unackedBytes >= ACK_BYTES) {
        sendAck();
      } else if (ackTimer === null) {
        ackTimer = window.setTimeout(sendAck, ACK_INTERVAL_MS);
      }
    };

    const handleServerControl = (msg: ServerControl) => {
      if (disposed) return;
      switch (msg.type) {
        case "hello": {
          const expected = lastSeq + 1;
          if (lastSeq > 0 && msg.seq_start > expected) {
            // Server couldn't replay everything we asked for — the missing
            // range is gone. Drop local scrollback so what the user sees
            // matches what's actually buffered server-side.
            term.reset();
            // eslint-disable-next-line no-console
            console.warn(
              `[XtermPane ${agentId}] resume gap: expected seq ${expected}, server starts at ${msg.seq_start}`,
            );
          }
          lastSeq = msg.seq_start - 1;
          writeLastSeq(agentId, lastSeq);
          // Hello carries the current lifecycle snapshot so a reconnect
          // doesn't sit in "spawning" forever waiting for an event that
          // already fired before we attached.
          if (msg.shim_ready) {
            setStatus("ready");
            setStatusDetail("");
            onShimReadySignal();
            onShimReady?.();
          }
          if (typeof msg.shim_exit === "number") {
            shimExited = true;
            setStatus("exited");
            setStatusDetail(`exit ${msg.shim_exit}`);
            // Unlock input on exit so any final bytes (e.g. user Ctrl-C
            // after seeing the exit banner) aren't permanently buffered.
            unlockInput();
            onShimExit?.(msg.shim_exit);
          }
          break;
        }
        case "shim_ready":
          setStatus("ready");
          setStatusDetail("");
          onShimReadySignal();
          onShimReady?.();
          break;
        case "shim_exit":
          shimExited = true;
          setStatus("exited");
          setStatusDetail(`exit ${msg.code}`);
          unlockInput();
          onShimExit?.(msg.code);
          clearLastSeq(agentId);
          break;
        case "eof":
          shimExited = true;
          setStatus("exited");
          setStatusDetail("EOF");
          clearLastSeq(agentId);
          break;
        case "error":
          // Resume-gap errors come through here. We've already reset the
          // terminal in the Hello branch; surface the message for UX.
          setStatus("error");
          setStatusDetail(msg.message);
          break;
      }
    };

    // ---- keystroke input ------------------------------------------------
    // Per-CLI gating: until `inputUnlocked === true`, keystrokes are buffered
    // (capped at preReadyBufferMax) and flushed in a single send the moment
    // the gate opens. Anything beyond the cap is dropped with a warn — that
    // path indicates an abnormal volume of pre-ready typing/pasting.
    const dataDisp = term.onData((data: string) => {
      if (ws.readyState !== WebSocket.OPEN) return;
      const bytes = new TextEncoder().encode(data);
      if (!inputUnlocked) {
        if (pendingBytes + bytes.byteLength > inputPolicy.preReadyBufferMax) {
          // eslint-disable-next-line no-console
          console.warn(
            `[XtermPane ${agentId}] pre-ready buffer full (${pendingBytes}B); dropping ${bytes.byteLength}B`,
          );
          return;
        }
        pendingChunks.push(bytes);
        pendingBytes += bytes.byteLength;
        return;
      }
      // Send as binary (UTF-8 bytes) so multi-byte sequences land intact.
      sendBytes(bytes);
    });

    // ---- resize handling ------------------------------------------------
    let resizeTimer: number | null = null;
    const ro = new ResizeObserver(() => {
      if (disposed) return;
      if (resizeTimer !== null) window.clearTimeout(resizeTimer);
      resizeTimer = window.setTimeout(() => {
        if (disposed) return;
        if (host.clientWidth === 0 || host.clientHeight === 0) return;
        try {
          fitAddon.fit();
          sendCtrl({ type: "resize", cols: term.cols, rows: term.rows });
        } catch {
          /* host detached */
        }
      }, RESIZE_DEBOUNCE_MS);
    });
    ro.observe(host);

    // ---- cleanup --------------------------------------------------------
    return () => {
      disposed = true;
      ro.disconnect();
      if (resizeTimer !== null) window.clearTimeout(resizeTimer);
      if (ackTimer !== null) window.clearTimeout(ackTimer);
      if (settleTimer !== null) window.clearTimeout(settleTimer);
      pendingChunks.length = 0;
      pendingBytes = 0;
      dataDisp.dispose();
      try {
        ws.close();
      } catch {
        /* noop */
      }
      try {
        webglRef.current?.dispose();
      } catch {
        /* noop */
      }
      webglRef.current = null;
      releaseSlot(agentId);
      termRef.current = null;
      fitAddonRef.current = null;
      term.dispose();
      // Clear the persisted lastSeq. The original design persisted it to
      // resume from where we left off across remounts (StrictMode dev,
      // page refresh). But xterm's scrollback dies with the Terminal
      // instance, so a remount always needs the FULL ring buffer replay
      // — not just the [lastSeq+1..] delta. Server-side cursor logic
      // (pty_ws.rs) handles the no-last_seq case as "replay entire
      // buffer", so dropping our memory of lastSeq triggers that path.
      clearLastSeq(agentId);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [agentId]);

  // Visibility-driven WebGL acquire/release. Separate effect so it can run
  // after the terminal is mounted *and* whenever the parent toggles `visible`
  // (maximize/minimize). DOM is the silent fallback when we can't get a slot.
  useEffect(() => {
    const term = termRef.current;
    const fit = fitAddonRef.current;
    if (!term) return;

    if (visible) {
      if (webglRef.current) return;
      if (!acquireSlot(agentId)) {
        // Cap reached or in cooldown — stay on DOM renderer.
        return;
      }
      try {
        const webgl = new WebglAddon();
        webgl.onContextLoss(() => {
          // eslint-disable-next-line no-console
          console.warn(`[XtermPane ${agentId}] WebGL context lost`);
          reportContextLoss(agentId);
          try {
            webgl.dispose();
          } catch {
            /* noop */
          }
          if (webglRef.current === webgl) webglRef.current = null;
        });
        term.loadAddon(webgl);
        webglRef.current = webgl;
        // Re-fit after attaching — atlas resets glyph cache and the new
        // renderer needs the current cols/rows to match the host.
        requestAnimationFrame(() => {
          try {
            fit?.fit();
          } catch {
            /* noop */
          }
        });
      } catch (err) {
        // eslint-disable-next-line no-console
        console.warn(`[XtermPane ${agentId}] WebGL acquire failed`, err);
        releaseSlot(agentId);
      }
    } else {
      // Hidden: free the slot so visible panes can have it.
      if (webglRef.current) {
        try {
          webglRef.current.dispose();
        } catch {
          /* noop */
        }
        webglRef.current = null;
      }
      releaseSlot(agentId);
    }
  }, [visible, agentId]);

  return (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        height: "100%",
        minHeight: 0,
      }}
    >
      <div
        style={{
          padding: "4px 8px",
          fontSize: 12,
          background: "#1f2937",
          color: status === "ready" ? "#10b981" : "#94a3b8",
          borderBottom: "1px solid #374151",
        }}
      >
        <strong>{agentId}</strong> — {status}
        {statusDetail && <span style={{ marginLeft: 8 }}>({statusDetail})</span>}
      </div>
      <div
        ref={hostRef}
        style={{
          flex: 1,
          minHeight: 0,
          padding: 4,
          background: "#0d0d0d",
        }}
      />
    </div>
  );
}
