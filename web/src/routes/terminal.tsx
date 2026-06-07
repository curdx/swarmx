/**
 * Terminal page (`/terminal`).
 *
 * A throwaway interactive `$SHELL` in the browser over /ws/terminal — for
 * ad-hoc commands, not a worker PTY. Minimal protocol: server sends binary PTY
 * bytes, we send keystrokes as binary + a `{type:"resize"}` JSON on fit. A
 * fresh shell each mount; closing the page (WS close) kills it server-side.
 */
import { useEffect, useRef } from "react";
import { useTranslation } from "react-i18next";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import { WS_HOST, WS_PROTO } from "@/lib/apiBase";

export default function TerminalRoute() {
  const { t } = useTranslation();
  const hostRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;

    const term = new Terminal({
      fontSize: 13,
      fontFamily: "ui-monospace, SFMono-Regular, Menlo, monospace",
      cursorBlink: true,
      theme: { background: "#0d0d0d", foreground: "#f0f0f0" },
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(host);
    fit.fit();

    const ws = new WebSocket(`${WS_PROTO}//${WS_HOST}/ws/terminal`);
    ws.binaryType = "arraybuffer";

    const sendResize = () => {
      try {
        fit.fit();
        if (ws.readyState === WebSocket.OPEN) {
          ws.send(JSON.stringify({ type: "resize", cols: term.cols, rows: term.rows }));
        }
      } catch {
        /* ignore */
      }
    };

    ws.onopen = () => {
      term.focus();
      sendResize();
    };
    ws.onmessage = (e) => {
      if (typeof e.data === "string") term.write(e.data);
      else term.write(new Uint8Array(e.data as ArrayBuffer));
    };
    ws.onclose = () => term.write("\r\n\x1b[90m[session closed]\x1b[0m\r\n");

    const enc = new TextEncoder();
    const dataDisp = term.onData((d) => {
      if (ws.readyState === WebSocket.OPEN) ws.send(enc.encode(d));
    });

    const ro = new ResizeObserver(() => sendResize());
    ro.observe(host);

    return () => {
      ro.disconnect();
      dataDisp.dispose();
      ws.close();
      term.dispose();
    };
  }, []);

  return (
    <div className="flex h-full flex-col">
      <header className="border-b border-border-subtle px-4 py-2 font-display text-sm text-foreground-primary">
        {t("nav.terminal")}
      </header>
      <div ref={hostRef} className="min-h-0 flex-1 overflow-hidden bg-[#0d0d0d] p-2" />
    </div>
  );
}
