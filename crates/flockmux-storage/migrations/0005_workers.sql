-- 0005_workers: ad-hoc workers spawned by an orchestrator agent.
--
-- Magentic-One 重构后,业务 agent 不再通过 spell + role 静态拉起,而是由
-- orchestrator 在 runtime 通过 swarm_spawn_worker MCP 工具按需 spawn,每个
-- worker 带自定义 prompt + handoff_signal + depends_on。spell_runs 表只剩
-- init spell 一种用途(workspace 创建时拉 orchestrator),所有 ad-hoc worker
-- 走这张新表登记。
--
-- parent_agent_id 必填(orchestrator 的 agent_id),用来支撑 DAG 视图的
-- "雇佣关系"实线 — 取代之前从 spell_runs.caller_agent_id 派生的路径。
-- handoff_signal / depends_on_json 直接喂给 WakeCoordinator(原来从 role
-- manifest 读,现在从这里读)。
--
-- system_prompt 留档便于:1) 用户事后回看 orchestrator 派了啥活,2) 录像
-- 回放完整还原现场,3) 失败 re-spawn 时直接复用同样的 prompt。
--
-- agent_id 是真 PTY agent_id(workers.agent_id ↔ agents.id 1:1)。worker
-- 行 always 跟 agents 行成对出现,不允许悬空。

INSERT INTO schema_version VALUES (5);

CREATE TABLE workers (
    agent_id          TEXT PRIMARY KEY REFERENCES agents(id),
    parent_agent_id   TEXT NOT NULL,
    role_label        TEXT NOT NULL,
    system_prompt     TEXT NOT NULL,
    handoff_signal    TEXT,
    depends_on_json   TEXT,
    spawned_at        INTEGER NOT NULL
);
CREATE INDEX idx_workers_parent ON workers(parent_agent_id);
