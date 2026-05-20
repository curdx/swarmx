/**
 * M1 minimal dashboard:
 *   - top bar lists installed CLI plugins (loaded from /api/plugins),
 *   - each "Spawn" button creates an agent and opens an XtermPane,
 *   - panes laid out as a simple CSS Grid (M2 will dress this up).
 */

import { useEffect, useState } from "react";
import { api } from "./api/http";
import type { CliPluginInfo, SpawnAgentResponse } from "./api/types";
import { XtermPane } from "./components/XtermPane";

export default function App() {
  const [plugins, setPlugins] = useState<CliPluginInfo[]>([]);
  const [pluginsError, setPluginsError] = useState<string | null>(null);
  const [agents, setAgents] = useState<SpawnAgentResponse[]>([]);
  const [spawning, setSpawning] = useState(false);

  useEffect(() => {
    api
      .listPlugins()
      .then(setPlugins)
      .catch((err: Error) => setPluginsError(err.message));
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
  };

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
          padding: "8px 12px",
          background: "#111827",
          borderBottom: "1px solid #374151",
          display: "flex",
          alignItems: "center",
          gap: 12,
          flexWrap: "wrap",
        }}
      >
        <strong style={{ fontSize: 14 }}>flockmux M1</strong>
        <span style={{ color: "#64748b", fontSize: 12 }}>
          local single-user — loopback only
        </span>
        <div style={{ flex: 1 }} />
        {pluginsError && (
          <span style={{ color: "#ef4444", fontSize: 12 }}>
            plugins error: {pluginsError}
          </span>
        )}
        {plugins.map((p) => (
          <button
            key={p.id}
            onClick={() => spawn(p.id)}
            disabled={spawning}
            title={`spawn ${p.binary}`}
          >
            + {p.display_name}
          </button>
        ))}
      </header>

      <main
        style={{
          flex: 1,
          minHeight: 0,
          padding: 8,
          display: "grid",
          gap: 8,
          gridTemplateColumns:
            agents.length <= 1 ? "1fr" : "repeat(2, minmax(0, 1fr))",
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
            No agents yet — pick a CLI above to spawn one.
          </div>
        )}
        {agents.map((agent) => (
          <div
            key={agent.agent_id}
            style={{
              display: "flex",
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
              }}
            >
              <span>
                <strong>{agent.role}</strong>
                <span style={{ color: "#94a3b8", marginLeft: 8 }}>
                  {agent.cli}
                </span>
              </span>
              <button onClick={() => kill(agent.agent_id)}>kill</button>
            </div>
            <div style={{ flex: 1, minHeight: 0 }}>
              <XtermPane agentId={agent.agent_id} />
            </div>
          </div>
        ))}
      </main>
    </div>
  );
}
