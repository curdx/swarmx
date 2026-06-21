#!/usr/bin/env node
// REAL-TOKEN acceptance test for the isolation → re-root flow (fd45c14 territory).
//
// Spawns real orchestrator CLIs, so it is NOT a CI test — run by hand against a
// server started with a THROWAWAY swarmx data dir but the REAL $HOME (so the
// spawned claude/codex find their login), e.g.:
//
//   D=$(mktemp -d)
//   SWARMX_PORT=7799 SWARMX_SERVER_URL=http://127.0.0.1:7799 \
//     SWARMX_DB_PATH=$D/d.db SWARMX_WORKSPACES_DIR=$D/ws \
//     SWARMX_BLACKBOARD_DIR=$D/bb SWARMX_RECORDINGS_DIR=$D/rec \
//     RUST_LOG=warn ./target/debug/swarmx-server &
//   SWARMX_PORT=7799 node scripts/reroot-acceptance.mjs [reassign|orphan|both]
//
// Two scenarios for how the user's opening request survives the orchestrator
// swap when a direction is named (== isolation + re-root):
//   • reassign — the request is still UNREAD when re-root fires, so
//     reassign_unread_user_messages re-addresses it to the new orchestrator.
//   • orphan   — orch1 already READ the request before re-root (we force this
//     deterministically via POST /api/message/read), so reassign finds nothing
//     and recovery falls to the task-seed (latest_user_message_for_agents →
//     the new orchestrator's {task}). This is the actual fd45c14 orphan.
// Both HARD-assert the deterministic isolation/re-root plumbing with real
// agents in the loop.

import { execSync } from "node:child_process";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";

const PORT = process.env.SWARMX_PORT || "7799";
const BASE = `http://127.0.0.1:${PORT}`;
const WS_SWARM = `ws://127.0.0.1:${PORT}/ws/swarm`;
const REQUEST = "Please add a dark mode toggle to the settings page.";
const MODE = process.argv[2] || "both";

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
  return { status: res.status, json, text };
}

class Collector {
  constructor() { this.events = []; this.ready = new Promise((r) => (this._r = r)); }
  async start() {
    this.ws = new WebSocket(WS_SWARM);
    this.ws.onmessage = (ev) => {
      if (typeof ev.data === "string") { try { this.events.push(JSON.parse(ev.data)); } catch {} }
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
const inbox = async (agent) => {
  const { json } = await req("GET", `${BASE}/api/message?to=${encodeURIComponent(agent)}&limit=50`);
  return Array.isArray(json) ? json : [];
};

async function runScenario(col, mode) {
  console.log(`\n========== scenario: ${mode} ==========`);
  const cwd = await mkdtemp(path.join(tmpdir(), `swarmx-reroot-${mode}-`));
  await writeFile(path.join(cwd, "README.md"), "# reroot acceptance\n");
  execSync('git init -q && git add -A && git -c user.email=t@t -c user.name=t commit -qm init', { cwd });

  const ws = await req("POST", `${BASE}/api/workspaces`, { name: `reroot-${mode}-${Date.now().toString(36)}`, cwd });
  const wsId = ws.json?.id;
  hard(!!wsId, `[${mode}] workspace created`);

  const thr = await req("POST", `${BASE}/api/workspaces/${wsId}/threads`, {});
  const tid = thr.json?.id, slug = thr.json?.slug;
  hard(!!tid && slug && slug !== "main", `[${mode}] non-main direction created (slug=${slug})`);

  const spell = await req("POST", `${BASE}/api/spell/run`, {
    name: "init", task: REQUEST, workspace_dir: cwd, workspace_id: wsId, thread_id: tid,
  });
  const orch1 = spell.json?.agents?.find((a) => a.role === "orchestrator")?.agent_id;
  hard(!!orch1, `[${mode}] orchestrator #1 spawned (${orch1})`);

  // Post the unread user request to orch1.
  const m = await req("POST", `${BASE}/api/message`, { from: "user", to: orch1, kind: "note", body: REQUEST });
  const msgId = m.json?.id;

  if (mode === "orphan") {
    // Force the orphan precondition deterministically: orch1 has "read" the
    // request before re-root, so reassign_unread will find nothing.
    await req("POST", `${BASE}/api/message/read`, { to: orch1, ids: [msgId] });
    const read = (await inbox(orch1)).find((x) => x.id === msgId);
    hard(read && read.read_at != null, `[orphan] precondition: orch1's request is READ before re-root`);
  } else {
    await pause(1500); // fast: PATCH before orch1's ~30s Phase-A reads the inbox
  }

  console.log(`→ [${mode}] PATCH name (== swarm_name_thread): worktree isolation + re-root`);
  const patch = await req("PATCH", `${BASE}/api/workspaces/${wsId}/threads/${tid}`, { name: `dark mode ${mode}` });
  hard(patch.status === 200, `[${mode}] PATCH …/threads → 200 (got ${patch.status})`);

  let isolated = null, orch2 = null;
  for (let i = 0; i < 60; i++) {
    const threads = await req("GET", `${BASE}/api/workspaces/${wsId}/threads`);
    const t = Array.isArray(threads.json) ? threads.json.find((x) => x.id === tid) : null;
    if (t?.isolation === "worktree") isolated = t;
    const live = (await agentsOnThread(tid)).filter((a) => a.role === "orchestrator" && a.killed_at == null && a.agent_id !== orch1);
    if (live.length) orch2 = live[0].agent_id;
    if (isolated && orch2) break;
    await pause(2000);
  }

  // Deterministic plumbing, real agents in the loop.
  hard(!!isolated, `[${mode}] thread isolation flipped to 'worktree'`);
  hard(existsSync(`${cwd}-${slug}`), `[${mode}] worktree dir exists (${cwd}-${slug})`);
  hard(col.has((e) => e.type === "thread_changed" && e.thread_id === tid && e.op === "isolated"),
    `[${mode}] ws/swarm observed ThreadChanged{op:'isolated'}`);
  const o1 = (await agentsOnThread(tid)).find((a) => a.agent_id === orch1);
  hard(o1 && o1.killed_at != null, `[${mode}] orchestrator #1 torn down (killed_at set)`);
  hard(!!orch2, `[${mode}] fresh orchestrator #2 re-rooted in (${orch2})`);

  // Request-survival, per mode.
  if (orch2) {
    const onNew = (await inbox(orch2)).some((x) => x.from_agent === "user" && x.body === REQUEST);
    if (mode === "reassign") {
      hard(onNew, `[reassign] unread request was reassigned to orchestrator #2`);
    } else {
      // Orphan: reassign must NOT move a read message; recovery is the task-seed.
      hard(!onNew, `[orphan] read request was NOT (wrongly) reassigned to orch2`);
      const o1read = (await inbox(orch1)).some((x) => x.id === msgId && x.read_at != null);
      hard(o1read, `[orphan] request still accounted for on orch1 (read, not stranded-unread)`);
      // Best-effort: confirm the request was seeded into orch2's bootstrap (the
      // recovery mechanism). PTY echo can mangle it, so this is observational —
      // the data path itself is unit-tested (store: latest_user_message_for_agents).
      await pause(2500);
      const recs = await req("GET", `${BASE}/api/recording?agent_id=${encodeURIComponent(orch2)}`);
      const recId = Array.isArray(recs.json) && recs.json[0]?.id;
      let seeded = false;
      if (recId) {
        const cast = await req("GET", `${BASE}/api/recording/${recId}`);
        seeded = typeof cast.text === "string" && cast.text.includes("dark mode toggle");
      }
      note(seeded
        ? `[orphan] task-seed CONFIRMED: the request reached orchestrator #2's bootstrap`
        : `[orphan] task-seed not visible in recording (PTY echo) — data path is store-tested separately`);
    }
  }

  for (const a of await agentsOnThread(tid)) await req("DELETE", `${BASE}/api/agent/${encodeURIComponent(a.agent_id)}`);
  await req("DELETE", `${BASE}/api/workspaces/${wsId}`);
  await rm(cwd, { recursive: true, force: true });
  await rm(`${cwd}-${slug}`, { recursive: true, force: true });
}

async function main() {
  console.log(`server ${BASE} · mode=${MODE}`);
  const col = new Collector();
  await col.start();
  const modes = MODE === "both" ? ["reassign", "orphan"] : [MODE];
  for (const mode of modes) await runScenario(col, mode);
  col.close();

  if (fails.length === 0) { console.log("\nREROOT ACCEPTANCE: PASS"); process.exit(0); }
  console.error(`\nREROOT ACCEPTANCE: FAIL (${fails.length})`);
  for (const f of fails) console.error(`  - ${f}`);
  process.exit(1);
}

main().catch((e) => { console.error("crash", e); process.exit(2); });
