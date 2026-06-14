/**
 * Terminal page (`/terminal`).
 *
 * An interactive `$SHELL` in the browser over /ws/terminal — for ad-hoc
 * commands, not a worker PTY. Minimal protocol: server sends binary PTY bytes,
 * we send keystrokes as binary + a `{type:"resize"}` JSON on fit.
 *
 * Per-workspace + persistent: the session id is keyed by the selected workspace
 * (`flockmux.terminal.session:<wsId>`, kept in sessionStorage), so each
 * workspace has its own shell that survives navigation/F5 (the server replays
 * scrollback on reattach). Switching the workspace picker tears down the view
 * and attaches that workspace's shell; a first spawn starts in its cwd.
 */
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Terminal as XtermTerminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import { ShieldAlert, Terminal as TerminalIcon } from "lucide-react";
import { WS_HOST, WS_PROTO } from "@/lib/apiBase";
import { useToolWorkspaces } from "@/lib/useToolWorkspaces";
import { WorkspacePicker } from "@/components/WorkspacePicker";
import { Button } from "@/components/ui/button";

/** Stable per-tab, per-workspace terminal session id, so navigating away and
 *  back reattaches to the same server-side shell instead of spawning a fresh
 *  one — and each workspace keeps its own shell. */
const SID_PREFIX = "flockmux.terminal.session:";
function terminalSessionId(wsId: string): string {
  const key = SID_PREFIX + wsId;
  try {
    let id = sessionStorage.getItem(key);
    if (!id) {
      id =
        typeof crypto !== "undefined" && crypto.randomUUID
          ? crypto.randomUUID()
          : `t-${Date.now()}-${Math.random().toString(36).slice(2)}`;
      sessionStorage.setItem(key, id);
    }
    return id;
  } catch {
    return `t-${Date.now()}`;
  }
}

export default function TerminalRoute() {
  const { t } = useTranslation();
  const { workspaces, wsId, setWsId, ready, error } = useToolWorkspaces();
  const hostRef = useRef<HTMLDivElement>(null);
  const [armed, setArmed] = useState(false);
  // Set true when the socket drops (onclose/onerror); drives the reconnect banner.
  const [wsClosed, setWsClosed] = useState(false);
  // Bumped by the reconnect button to re-run the connect effect even when
  // `armed` is already true.
  const [reconnectNonce, setReconnectNonce] = useState(0);
  const activeWs = workspaces.find((w) => w.id === wsId) ?? null;

  useEffect(() => {
    setArmed(false);
    setWsClosed(false);
  }, [wsId]);

  useEffect(() => {
    // Wait until the workspace list resolved so we attach to the right shell.
    if (!ready || !armed) return;
    const host = hostRef.current;
    if (!host) return;
    setWsClosed(false);

    const term = new XtermTerminal({
      fontSize: 13,
      fontFamily: "ui-monospace, SFMono-Regular, Menlo, monospace",
      cursorBlink: true,
      theme: { background: "#0d0d0d", foreground: "#f0f0f0" },
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(host);
    fit.fit();

    const wsParam = wsId ? `&workspace_id=${encodeURIComponent(wsId)}` : "";
    const ws = new WebSocket(
      `${WS_PROTO}//${WS_HOST}/ws/terminal?session=${encodeURIComponent(terminalSessionId(wsId))}${wsParam}`,
    );
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
    ws.onerror = () => {
      term.write("\r\n\x1b[31m[连接出错]\x1b[0m\r\n");
      setWsClosed(true);
    };
    ws.onclose = () => {
      term.write("\r\n\x1b[90m[session closed]\x1b[0m\r\n");
      setWsClosed(true);
    };

    const enc = new TextEncoder();
    const dataDisp = term.onData((d) => {
      if (ws.readyState === WebSocket.OPEN) {
        ws.send(enc.encode(d));
      } else {
        // Don't silently swallow keystrokes — tell the user nothing was sent.
        term.write("\r\n\x1b[33m[未连接，输入未发送]\x1b[0m\r\n");
        setWsClosed(true);
      }
    });

    const ro = new ResizeObserver(() => sendResize());
    ro.observe(host);

    return () => {
      ro.disconnect();
      dataDisp.dispose();
      ws.close();
      term.dispose();
    };
  }, [wsId, ready, armed, reconnectNonce]);

  return (
    <div className="flex h-full flex-col">
      <header className="flex items-center gap-2 border-b border-border-subtle px-4 py-2">
        <span className="font-display text-sm text-foreground-primary">{t("nav.terminal")}</span>
        {workspaces.length > 0 && (
          <WorkspacePicker
            className="ml-auto"
            workspaces={workspaces}
            value={wsId}
            onChange={setWsId}
          />
        )}
      </header>
      {armed ? (
        <div className="flex min-h-0 flex-1 flex-col">
          {wsClosed && (
            <div className="flex items-center gap-3 border-b border-border-subtle bg-status-warning-soft px-4 py-2">
              <span className="font-caption text-xs text-status-warning">
                {t("terminal.disconnected", { defaultValue: "连接已断开" })}
              </span>
              <Button
                size="sm"
                variant="outline"
                className="ml-auto gap-1.5"
                onClick={() => {
                  setWsClosed(false);
                  setReconnectNonce((n) => n + 1);
                }}
              >
                <TerminalIcon className="size-3.5" />
                {t("terminal.reconnect", { defaultValue: "重新连接" })}
              </Button>
            </div>
          )}
          <div ref={hostRef} className="min-h-0 flex-1 overflow-hidden bg-[#0d0d0d] p-2" />
        </div>
      ) : (
        <div className="flex min-h-0 flex-1 items-center justify-center bg-surface-primary px-6">
          <section className="flex w-full max-w-lg flex-col gap-4 rounded-lg border border-border-subtle bg-surface-elevated p-5">
            <div className="flex items-start gap-3">
              <span className="flex size-10 shrink-0 items-center justify-center rounded-md bg-status-warning-soft text-status-warning">
                <ShieldAlert className="size-5" />
              </span>
              <div className="min-w-0 flex-1">
                <h1 className="font-heading text-base font-semibold text-foreground-primary">
                  {t("terminal.confirmTitle")}
                </h1>
                <p
                  className={`mt-1 font-caption text-xs leading-relaxed ${
                    error ? "text-state-danger" : "text-foreground-secondary"
                  }`}
                >
                  {error
                    ? t("terminal.backendDown", {
                        defaultValue:
                          "连接不上后端 (127.0.0.1:7777)，无法打开终端。请确认 flockmux 服务在运行。",
                      })
                    : t("terminal.confirmDesc", {
                        workspace: activeWs?.name ?? t("common.all"),
                      })}
                </p>
              </div>
            </div>
            <Button
              className="self-start gap-1.5"
              onClick={() => ready && !error && setArmed(true)}
              disabled={!ready || !!error}
            >
              <TerminalIcon className="size-3.5" />
              {error
                ? t("terminal.unavailable", { defaultValue: "后端未连接" })
                : ready
                  ? t("terminal.connect")
                  : t("common.loading")}
            </Button>
          </section>
        </div>
      )}
    </div>
  );
}
