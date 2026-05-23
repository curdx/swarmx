/**
 * M2 dashboard:
 *   - top bar lists installed CLI plugins; each button spawns one agent.
 *   - main area is an adaptive grid: cols = ceil(sqrt(visible)), capped at 6.
 *   - per-pane minimize / maximize / kill controls.
 *
 * Layout invariants (intentional):
 *   - panes that are minimized or hidden behind a maximize use `display:none`
 *     and stay mounted, so the WS+PTY stays alive without reconnect cost.
 *     XtermPane's ResizeObserver no-ops while host is 0x0, then refits when
 *     visibility returns. Reconnecting would lose terminal scrollback and
 *     re-trigger the CLI's "Welcome" sequence — both jarring.
 *   - cols cap at 6: ~Math.ceil(sqrt(40))=7 already gives <250px panes on a
 *     1500px-wide window; beyond 6 the terminal grid becomes unreadable.
 */

import { useEffect, useMemo, useState } from "react";
import { api } from "./api/http";
import type { CliPluginInfo, SpawnAgentResponse } from "./api/types";
import { XtermPane } from "./components/XtermPane";
import { SwarmPanel } from "./components/SwarmPanel";
import { SpellsLauncher } from "./components/SpellsLauncher";

const MAX_COLS = 6;
const SWARM_PANEL_KEY = "flockmux:swarmPanelOpen";

export default function App() {
  const [plugins, setPlugins] = useState<CliPluginInfo[]>([]);
  const [pluginsError, setPluginsError] = useState<string | null>(null);
  const [agents, setAgents] = useState<SpawnAgentResponse[]>([]);
  const [spawning, setSpawning] = useState(false);
  const [maximized, setMaximized] = useState<string | null>(null);
  const [minimized, setMinimized] = useState<Set<string>>(new Set());
  const [swarmOpen, setSwarmOpen] = useState<boolean>(() => {
    try {
      return window.localStorage.getItem(SWARM_PANEL_KEY) === "1";
    } catch {
      return false;
    }
  });

  useEffect(() => {
    try {
      window.localStorage.setItem(SWARM_PANEL_KEY, swarmOpen ? "1" : "0");
    } catch {
      // ignore
    }
  }, [swarmOpen]);

  // Pull /api/agent and replace the in-memory pane list with everything
  // the server still considers live. Used on mount (reattach after
  // refresh) AND after a spell run (multiple new agents need to appear
  // without an extra ws/swarm round-trip).
  const refreshAgents = async () => {
    try {
      const items = await api.listAgents();
      const live = items.filter(
        (a) => a.killed_at == null && a.shim_exit == null,
      );
      // Replace, don't merge: SQLite IS the source of truth for who's
      // alive. The previous in-memory list might hold a row that the
      // server just killed.
      setAgents(
        live.map((a) => ({
          agent_id: a.agent_id,
          cli: a.cli,
          role: a.role,
          workspace: a.workspace,
        })),
      );
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn("listAgents failed", err);
    }
  };

  useEffect(() => {
    api
      .listPlugins()
      .then(setPlugins)
      .catch((err: Error) => setPluginsError(err.message));
    // Reattach to agents that survived a page reload. The server-side
    // registry outlives the WS, so a refresh / new tab should pick up
    // existing PTYs instead of stranding them.
    refreshAgents();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const spawn = async (cli: string) => {
    setSpawning(true);
    try {
      const agent = await api.spawnAgent({ cli });
      setAgents((prev) => [...prev, agent]);
    } catch (err) {
      // eslint-disable-next-line no-alert
      alert(`Spawn failed: ${(err as Error).message}`);
    } finally {
      setSpawning(false);
    }
  };

  const kill = async (agentId: string) => {
    try {
      await api.killAgent(agentId);
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn("kill failed", err);
    }
    setAgents((prev) => prev.filter((a) => a.agent_id !== agentId));
    setMinimized((prev) => {
      if (!prev.has(agentId)) return prev;
      const next = new Set(prev);
      next.delete(agentId);
      return next;
    });
    setMaximized((cur) => (cur === agentId ? null : cur));
  };

  const toggleMinimize = (agentId: string) => {
    setMinimized((prev) => {
      const next = new Set(prev);
      if (next.has(agentId)) next.delete(agentId);
      else next.add(agentId);
      return next;
    });
    setMaximized((cur) => (cur === agentId ? null : cur));
  };

  const toggleMaximize = (agentId: string) => {
    setMaximized((cur) => (cur === agentId ? null : agentId));
    setMinimized((prev) => {
      if (!prev.has(agentId)) return prev;
      const next = new Set(prev);
      next.delete(agentId);
      return next;
    });
  };

  const visibleAgents = useMemo(() => {
    if (maximized) return agents.filter((a) => a.agent_id === maximized);
    return agents.filter((a) => !minimized.has(a.agent_id));
  }, [agents, maximized, minimized]);

  const cols = useMemo(() => {
    if (maximized) return 1;
    if (visibleAgents.length === 0) return 1;
    return Math.min(MAX_COLS, Math.ceil(Math.sqrt(visibleAgents.length)));
  }, [maximized, visibleAgents.length]);

  const dockAgents = agents.filter((a) => minimized.has(a.agent_id));

  return (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        height: "100%",
      }}
    >
      <header
        style={{
          background: "#111827",
          borderBottom: "1px solid #374151",
          display: "flex",
          flexDirection: "column",
          gap: 6,
          padding: "8px 12px",
        }}
      >
        {/* 第一行：品牌 + 简要状态 + spawn 按钮 + 面板开关 */}
        <div
          style={{
            display: "flex",
            alignItems: "center",
            gap: 12,
            flexWrap: "wrap",
          }}
        >
          <strong style={{ fontSize: 14 }}>flockmux M2</strong>
          <span style={{ color: "#64748b", fontSize: 12 }}>
            本地单用户 · 仅回环
          </span>
          <span style={{ color: "#64748b", fontSize: 12 }}>
            {agents.length} 个 agent · 显示 {visibleAgents.length}
          </span>
          <div style={{ flex: 1 }} />
          {pluginsError && (
            <span style={{ color: "#ef4444", fontSize: 12 }}>
              插件加载失败：{pluginsError}
            </span>
          )}
          {plugins.map((p) => (
            <button
              key={p.id}
              onClick={() => spawn(p.id)}
              disabled={spawning}
              title={`启动 ${p.binary}`}
            >
              + {p.display_name}
            </button>
          ))}
          <button
            onClick={() => setSwarmOpen((v) => !v)}
            title="切换协作面板"
            style={{
              background: swarmOpen ? "#1e3a8a" : "#1f2937",
              color: "#e2e8f0",
              border: "1px solid #374151",
              borderRadius: 4,
              padding: "2px 8px",
              fontSize: 12,
            }}
          >
            {swarmOpen ? "隐藏面板" : "显示面板"}
          </button>
        </div>
        {/* 第二行：法术启动器单独一行，输入框可自适应宽度 */}
        <SpellsLauncher onSpellLaunched={refreshAgents} />
      </header>

      <div
        style={{
          flex: 1,
          minHeight: 0,
          display: "flex",
          flexDirection: "row",
        }}
      >
      <main
        style={{
          flex: 1,
          minHeight: 0,
          padding: 8,
          display: "grid",
          gap: 8,
          gridTemplateColumns: `repeat(${cols}, minmax(0, 1fr))`,
          gridAutoRows: "minmax(0, 1fr)",
        }}
      >
        {agents.length === 0 && (
          <div
            style={{
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              color: "#64748b",
              border: "1px dashed #374151",
              borderRadius: 6,
            }}
          >
            还没有 agent — 在上方输入任务点 ✨ Auto，或单独启动一个 CLI
          </div>
        )}
        {agents.map((agent) => {
          const isMinimized = minimized.has(agent.agent_id);
          const isMaximized = maximized === agent.agent_id;
          const hidden =
            (maximized !== null && !isMaximized) || (!maximized && isMinimized);
          return (
            <div
              key={agent.agent_id}
              style={{
                display: hidden ? "none" : "flex",
                flexDirection: "column",
                border: "1px solid #374151",
                borderRadius: 6,
                minHeight: 0,
                overflow: "hidden",
              }}
            >
              <div
                style={{
                  display: "flex",
                  justifyContent: "space-between",
                  alignItems: "center",
                  padding: "4px 8px",
                  background: "#1f2937",
                  fontSize: 12,
                  gap: 8,
                }}
              >
                <span style={{ overflow: "hidden", textOverflow: "ellipsis" }}>
                  <strong>{agent.role}</strong>
                  <span style={{ color: "#94a3b8", marginLeft: 8 }}>
                    {agent.cli}
                  </span>
                </span>
                <span style={{ display: "flex", gap: 4 }}>
                  <button
                    onClick={() => toggleMinimize(agent.agent_id)}
                    title="最小化"
                  >
                    _
                  </button>
                  <button
                    onClick={() => toggleMaximize(agent.agent_id)}
                    title={isMaximized ? "还原" : "最大化"}
                  >
                    {isMaximized ? "❐" : "□"}
                  </button>
                  <button onClick={() => kill(agent.agent_id)} title="终止">
                    ×
                  </button>
                </span>
              </div>
              <div style={{ flex: 1, minHeight: 0 }}>
                <XtermPane agentId={agent.agent_id} visible={!hidden} />
              </div>
            </div>
          );
        })}
      </main>
        {swarmOpen && <SwarmPanel />}
      </div>

      {dockAgents.length > 0 && (
        <footer
          style={{
            display: "flex",
            gap: 6,
            flexWrap: "wrap",
            padding: "6px 8px",
            background: "#111827",
            borderTop: "1px solid #374151",
            fontSize: 12,
          }}
        >
          <span style={{ color: "#64748b", alignSelf: "center" }}>
            已最小化：
          </span>
          {dockAgents.map((a) => (
            <button
              key={a.agent_id}
              onClick={() => toggleMinimize(a.agent_id)}
              title={`还原 ${a.agent_id}`}
              style={{
                background: "#1f2937",
                color: "#cbd5f5",
                border: "1px solid #374151",
                borderRadius: 4,
                padding: "2px 8px",
              }}
            >
              {a.role}{" "}
              <span style={{ color: "#64748b" }}>({a.agent_id.slice(-8)})</span>
            </button>
          ))}
        </footer>
      )}
    </div>
  );
}
