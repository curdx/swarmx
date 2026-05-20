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
 * M1 scope:
 *   - WebGL renderer with Canvas fallback on context loss.
 *   - Unicode 11 + FitAddon + WebLinks.
 *   - seq number is read but only validated for monotonicity (no resume
 *     replay yet — M3).
 *   - ACK is batched every 5KB or 50ms (server-side currently ignores
 *     them; harmless to send).
 *
 * References:
 *   - hermes-agent web/src/pages/ChatPage.tsx (xterm WebGL + ResizeObserver)
 *   - golutra src/features/terminal/TerminalPane.vue (WebGL cooldown, ACK)
 */

import { useEffect, useRef, useState } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { Unicode11Addon } from "@xterm/addon-unicode11";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { WebglAddon } from "@xterm/addon-webgl";
import type { ServerControl, ClientControl } from "../api/types";

interface Props {
  agentId: string;
  onShimReady?: () => void;
  onShimExit?: (code: number) => void;
}

const ACK_BYTES = 5 * 1024;
const ACK_INTERVAL_MS = 50;
const RESIZE_DEBOUNCE_MS = 150;

export function XtermPane({ agentId, onShimReady, onShimExit }: Props) {
  const hostRef = useRef<HTMLDivElement | null>(null);
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

    let webgl: WebglAddon | null = null;
    try {
      webgl = new WebglAddon();
      webgl.onContextLoss(() => {
        webgl?.dispose();
        webgl = null;
        // eslint-disable-next-line no-console
        console.warn(
          `[XtermPane ${agentId}] WebGL context lost; falling back to canvas`,
        );
      });
      term.loadAddon(webgl);
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn(`[XtermPane ${agentId}] WebGL not available`, err);
    }

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
    const proto = window.location.protocol === "https:" ? "wss:" : "ws:";
    const wsUrl = `${proto}//${window.location.host}/ws/pty/${encodeURIComponent(agentId)}`;
    const ws = new WebSocket(wsUrl);
    ws.binaryType = "arraybuffer";

    let lastSeq = 0;
    let unackedBytes = 0;
    let ackTimer: number | null = null;
    let shimExited = false;

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
          // Gap detected — in M3 we'll request resume; for M1 just warn.
          // eslint-disable-next-line no-console
          console.warn(
            `[XtermPane ${agentId}] seq gap: expected ${lastSeq + 1}, got ${seq}`,
          );
        }
        lastSeq = seq;
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
        case "hello":
          lastSeq = msg.seq_start - 1;
          break;
        case "shim_ready":
          setStatus("ready");
          setStatusDetail("");
          onShimReady?.();
          break;
        case "shim_exit":
          shimExited = true;
          setStatus("exited");
          setStatusDetail(`exit ${msg.code}`);
          onShimExit?.(msg.code);
          break;
        case "eof":
          shimExited = true;
          setStatus("exited");
          setStatusDetail("EOF");
          break;
        case "error":
          setStatus("error");
          setStatusDetail(msg.message);
          break;
      }
    };

    // ---- keystroke input ------------------------------------------------
    const dataDisp = term.onData((data: string) => {
      if (ws.readyState !== WebSocket.OPEN) return;
      // Send as binary (UTF-8 bytes) so multi-byte sequences land intact.
      ws.send(new TextEncoder().encode(data));
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
      dataDisp.dispose();
      try {
        ws.close();
      } catch {
        /* noop */
      }
      try {
        webgl?.dispose();
      } catch {
        /* noop */
      }
      term.dispose();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [agentId]);

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
