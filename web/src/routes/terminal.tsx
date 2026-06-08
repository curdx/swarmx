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
  const { workspaces, wsId, setWsId, ready } = useToolWorkspaces();
  const hostRef = useRef<HTMLDivElement>(null);
  const [armed, setArmed] = useState(false);
  const activeWs = workspaces.find((w) => w.id === wsId) ?? null;

  useEffect(() => {
    setArmed(false);
  }, [wsId]);

  useEffect(() => {
    // Wait until the workspace list resolved so we attach to the right shell.
    if (!ready || !armed) return;
    const host = hostRef.current;
    if (!host) return;

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
  }, [wsId, ready, armed]);

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
        <div ref={hostRef} className="min-h-0 flex-1 overflow-hidden bg-[#0d0d0d] p-2" />
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
                <p className="mt-1 font-caption text-xs leading-relaxed text-foreground-secondary">
                  {t("terminal.confirmDesc", {
                    workspace: activeWs?.name ?? t("common.all"),
                  })}
                </p>
              </div>
            </div>
            <Button
              className="self-start gap-1.5"
              onClick={() => ready && setArmed(true)}
              disabled={!ready}
            >
              <TerminalIcon className="size-3.5" />
              {ready ? t("terminal.connect") : t("common.loading")}
            </Button>
          </section>
        </div>
      )}
    </div>
  );
}
