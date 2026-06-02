#!/usr/bin/env node
// REAL-TOKEN acceptance test for the isolation → re-root flow (fd45c14 territory).
//
// Spawns real orchestrator CLIs, so it is NOT a CI test — run by hand against a
// server started with a THROWAWAY flockmux data dir but the REAL $HOME (so the
// spawned claude/codex find their login), e.g.:
//
//   D=$(mktemp -d)
//   FLOCKMUX_PORT=7799 FLOCKMUX_SERVER_URL=http://127.0.0.1:7799 \
//     FLOCKMUX_DB_PATH=$D/d.db FLOCKMUX_WORKSPACES_DIR=$D/ws \
//     FLOCKMUX_BLACKBOARD_DIR=$D/bb FLOCKMUX_RECORDINGS_DIR=$D/rec \
//     RUST_LOG=warn ./target/debug/flockmux-server &
//   FLOCKMUX_PORT=7799 node scripts/reroot-acceptance.mjs
//
// Flow: git workspace → non-main direction → init spell spawns orchestrator #1
// → post an UNREAD user request to it → PATCH a name (== swarm_name_thread)
// which triggers real worktree isolation + re-root (kills orch1, spawns orch2,
// reassigns the unread request). We HARD-assert the deterministic plumbing and
// OBSERVE the (timing-dependent) request-survival outcome.

import { execSync } from "node:child_process";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";

const PORT = process.env.FLOCKMUX_PORT || "7799";
const BASE = `http://127.0.0.1:${PORT}`;
const WS_SWARM = `ws://127.0.0.1:${PORT}/ws/swarm`;
const REQUEST = "Please add a dark mode toggle to the settings page.";

const fails = [];
const hard = (cond, msg) => {
  console.log(`${cond ? "  ok" : "FAIL"}: ${msg}`);
  if (!cond) fails.push(msg);
};
const note = (msg) => console.log(`  ·· ${msg}`);
const pause = (ms) => new Promise((r) => setTimeout(r, ms));

async function req(method, url, body) {
  const res = await fetch(url, {
    method,
    headers: body ? { "content-type": "application/json" } : undefined,
    body: body ? JSON.stringify(body) : undefined,
  });
  const text = await res.text();
  let json;
  try { json = text ? JSON.parse(text) : null; } catch { json = text; }
  return { status: res.status, json };
}

class Collector {
  constructor() { this.events = []; this.ready = new Promise((r) => (this._r = r)); }
  async start() {
    this.ws = new WebSocket(WS_SWARM);
    this.ws.onmessage = (ev) => {
      if (typeof ev.data === "string") {
        try { this.events.push(JSON.parse(ev.data)); } catch {}
      }
    };
    this.ws.onopen = () => this._r();
    await this.ready; await pause(50);
  }
  has(pred) { return this.events.some(pred); }
  close() { this.ws?.close(); }
}

const agentsOnThread = async (tid) => {
  const { json } = await req("GET", `${BASE}/api/agent`);
  return Array.isArray(json) ? json.filter((a) => a.thread_id === tid) : [];
};

async function main() {
  // 0. A real git project as the workspace cwd (worktree add needs a base).
  const cwd = await mkdtemp(path.join(tmpdir(), "flockmux-reroot-"));
  await writeFile(path.join(cwd, "README.md"), "# reroot acceptance\n");
  execSync('git init -q && git add -A && git -c user.email=t@t -c user.name=t commit -qm init', { cwd });
  console.log(`git workspace: ${cwd}  (server ${BASE})`);

  const col = new Collector();
  await col.start();

  const ws = await req("POST", `${BASE}/api/workspaces`, { name: `reroot-${Date.now().toString(36)}`, cwd });
  const wsId = ws.json?.id;
  hard(!!wsId, "workspace created");

  // 1. A non-main direction (shared, not yet isolated).
  const thr = await req("POST", `${BASE}/api/workspaces/${wsId}/threads`, {});
  const tid = thr.json?.id;
  const slug = thr.json?.slug;
  hard(!!tid && slug && slug !== "main", `non-main direction created (slug=${slug})`);

  // 2. A real orchestrator ON that direction via the init spell.
  const spell = await req("POST", `${BASE}/api/spell/run`, {
    name: "init", task: REQUEST, workspace_dir: cwd, workspace_id: wsId, thread_id: tid,
  });
  const orch1 = spell.json?.agents?.find((a) => a.role === "orchestrator")?.agent_id;
  hard(!!orch1, `orchestrator #1 spawned (${orch1})`);

  // 3. An UNREAD user request addressed to orch1, then isolate FAST (before
  //    orch1's ~30s Phase-A turn reads it — that maximizes the chance reassign,
  //    not the task-seed fallback, carries it).
  await req("POST", `${BASE}/api/message`, { from: "user", to: orch1, kind: "note", body: REQUEST });
  await pause(1500);

  console.log("→ PATCH name (== swarm_name_thread): triggers worktree isolation + re-root");
  const patch = await req("PATCH", `${BASE}/api/workspaces/${wsId}/threads/${tid}`, { name: "dark mode" });
  hard(patch.status === 200, `PATCH …/threads → 200 (got ${patch.status})`);

  // 4. Wait for isolation to land: thread.isolation flips to "worktree" AND a
  //    fresh orchestrator (≠ orch1) appears. Real CLI boot ⇒ generous poll.
  let isolated = null, orch2 = null;
  for (let i = 0; i < 60; i++) {
    const threads = await req("GET", `${BASE}/api/workspaces/${wsId}/threads`);
    const t = Array.isArray(threads.json) ? threads.json.find((x) => x.id === tid) : null;
    const live = (await agentsOnThread(tid)).filter((a) => a.role === "orchestrator" && a.killed_at == null && a.agent_id !== orch1);
    if (t?.isolation === "worktree") isolated = t;
    if (live.length) orch2 = live[0].agent_id;
    if (isolated && orch2) break;
    await pause(2000);
  }

  // 5. HARD assertions — the deterministic plumbing, with real agents in the loop.
  hard(!!isolated, "thread isolation flipped to 'worktree'");
  const wtDir = `${cwd}-${slug}`;
  hard(existsSync(wtDir), `worktree dir exists on disk (${wtDir})`);
  hard(col.has((e) => e.type === "thread_changed" && e.thread_id === tid && e.op === "isolated"),
    "ws/swarm observed ThreadChanged{op:'isolated'}");
  const all = await agentsOnThread(tid);
  const o1 = all.find((a) => a.agent_id === orch1);
  hard(o1 && o1.killed_at != null, "orchestrator #1 was torn down (killed_at set)");
  hard(!!orch2, `a fresh orchestrator #2 was re-rooted in (${orch2})`);

  // 6. OBSERVE request survival (timing-dependent: reassign vs task-seed fallback).
  if (orch2) {
    const toNew = await req("GET", `${BASE}/api/message?to=${encodeURIComponent(orch2)}&limit=50`);
    const moved = Array.isArray(toNew.json) && toNew.json.some((m) => m.from_agent === "user" && m.body === REQUEST);
    const toOld = await req("GET", `${BASE}/api/message?to=${encodeURIComponent(orch1)}&limit=50`);
    const stranded = Array.isArray(toOld.json) && toOld.json.some((m) => m.from_agent === "user" && m.body === REQUEST && m.read_at == null);
    if (moved) note("request SURVIVED: unread user message reassigned to orchestrator #2 (the fd45c14 guarantee)");
    else if (stranded) note("⚠ request STRANDED on the dead orch1 inbox, unread — investigate");
    else note("request not on orch2 inbox — orch1 likely READ it first; should be recovered via task-seed (latest_user_message_for_agents). Not stranded.");
  }

  // cleanup
  for (const a of await agentsOnThread(tid)) await req("DELETE", `${BASE}/api/agent/${encodeURIComponent(a.agent_id)}`);
  await req("DELETE", `${BASE}/api/workspaces/${wsId}`);
  col.close();
  await rm(cwd, { recursive: true, force: true });
  await rm(`${cwd}-${slug}`, { recursive: true, force: true });

  if (fails.length === 0) { console.log("\nREROOT ACCEPTANCE: PASS"); process.exit(0); }
  console.error(`\nREROOT ACCEPTANCE: FAIL (${fails.length})`);
  for (const f of fails) console.error(`  - ${f}`);
  process.exit(1);
}

main().catch((e) => { console.error("crash", e); process.exit(2); });
