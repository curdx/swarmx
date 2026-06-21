#!/usr/bin/env node
// Swarm smoke test (M3 #18):
//   1. spawn two agents (A, B)
//   2. POST /api/message  A→B  → 200 + id
//   3. GET  /api/message?to=B  → contains the message
//   4. GET  /api/message?q=...  → FTS5 hit
//   5. PUT  /api/blackboard/tasks.md → 200
//   6. GET  /api/blackboard/tasks.md → echoes content + sha
//   7. Subscribe ws/swarm, then fs.writeFile <blackboard>/external.md
//      from outside swarmx → expect a BlackboardChanged{op:"external"} event
//   8. PUT /api/blackboard/..%2Fescape.md → 400 (path traversal)
//   9. DELETE both agents, exit 0 on PASS / 1 on FAIL
//
// Assumptions:
//   * swarmx-server is running at 127.0.0.1:7777.
//   * The blackboard root is either SWARMX_BLACKBOARD_DIR (if set when the
//     server started, also export it here) or ~/.swarmx/blackboard.

import { writeFile, mkdir, rm } from "node:fs/promises";
import { existsSync } from "node:fs";
import { homedir } from "node:os";
import path from "node:path";

const BASE = "http://127.0.0.1:7777";
const WS_SWARM = "ws://127.0.0.1:7777/ws/swarm";
const CLI = process.argv[2] || "codex";

const BLACKBOARD_ROOT =
  process.env.SWARMX_BLACKBOARD_DIR ||
  path.join(homedir(), ".swarmx", "blackboard");

const failures = [];
function check(cond, msg) {
  if (!cond) {
    failures.push(msg);
    console.error(`FAIL: ${msg}`);
  } else {
    console.log(`  ok: ${msg}`);
  }
}

async function postJson(url, body) {
  const res = await fetch(url, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  });
  const text = await res.text();
  let json;
  try {
    json = text ? JSON.parse(text) : null;
  } catch {
    json = text;
  }
  return { status: res.status, json };
}

async function putJson(url, body) {
  const res = await fetch(url, {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  });
  const text = await res.text();
  let json;
  try {
    json = text ? JSON.parse(text) : null;
  } catch {
    json = text;
  }
  return { status: res.status, json };
}

async function getJson(url) {
  const res = await fetch(url);
  const text = await res.text();
  let json;
  try {
    json = text ? JSON.parse(text) : null;
  } catch {
    json = text;
  }
  return { status: res.status, json };
}

async function spawnAgent() {
  const { status, json } = await postJson(`${BASE}/api/agent`, { cli: CLI });
  if (status !== 200) {
    throw new Error(`spawn failed: ${status} ${JSON.stringify(json)}`);
  }
  return json.agent_id;
}

async function killAgent(id) {
  await fetch(`${BASE}/api/agent/${encodeURIComponent(id)}`, {
    method: "DELETE",
  });
}

function pause(ms) {
  return new Promise((r) => setTimeout(r, ms));
}

class SwarmEventCollector {
  constructor() {
    this.events = [];
    this.ws = null;
    this.ready = new Promise((resolve) => {
      this._resolveReady = resolve;
    });
  }
  async start() {
    this.ws = new WebSocket(WS_SWARM);
    this.ws.onmessage = (ev) => {
      if (typeof ev.data !== "string") return;
      try {
        const msg = JSON.parse(ev.data);
        this.events.push(msg);
      } catch (e) {
        console.warn("non-JSON ws frame", ev.data, e);
      }
    };
    this.ws.onopen = () => this._resolveReady();
    this.ws.onerror = (e) => console.warn("ws swarm error", e?.message ?? e);
    await this.ready;
    // Tiny grace period so subscribe is fully wired on the server.
    await pause(50);
  }
  close() {
    if (this.ws) this.ws.close();
  }
  // Wait up to `timeoutMs` for a message matching `pred`.
  async waitFor(pred, timeoutMs = 2000) {
    const deadline = Date.now() + timeoutMs;
    while (Date.now() < deadline) {
      const hit = this.events.find(pred);
      if (hit) return hit;
      await pause(50);
    }
    return null;
  }
}

async function main() {
  console.log(`blackboard root: ${BLACKBOARD_ROOT}`);
  if (!existsSync(BLACKBOARD_ROOT)) {
    await mkdir(BLACKBOARD_ROOT, { recursive: true });
  }

  // Subscribe early so we don't miss BlackboardChanged events triggered later.
  const collector = new SwarmEventCollector();
  await collector.start();
  console.log("ws/swarm subscribed");

  const a = await spawnAgent();
  const b = await spawnAgent();
  console.log(`spawned A=${a}  B=${b}`);

  // 1. POST /api/message
  //    Use an alphanumeric tag (no hyphen) because FTS5 treats `-` as the
  //    NOT operator; this lets the search endpoint stay a thin pass-through
  //    to FTS5 query syntax (the API contract).
  const tag = `helloswarm${Date.now().toString(36)}`;
  const send = await postJson(`${BASE}/api/message`, {
    from: a,
    to: b,
    kind: "note",
    body: `swarm-smoke ${tag}`,
  });
  check(send.status === 200, "POST /api/message returns 200");
  check(send.json && typeof send.json.id === "number", "send response has numeric id");
  const msgId = send.json?.id;

  // 2. GET /api/message?to=B
  const listed = await getJson(
    `${BASE}/api/message?to=${encodeURIComponent(b)}&limit=10`,
  );
  check(listed.status === 200, "GET /api/message?to=B returns 200");
  const found = Array.isArray(listed.json)
    ? listed.json.find((m) => m.id === msgId)
    : null;
  check(!!found, "listed messages include the just-sent message");

  // 3. FTS5 search
  const search = await getJson(
    `${BASE}/api/message?q=${encodeURIComponent(tag)}`,
  );
  check(search.status === 200, "GET /api/message?q=tag returns 200");
  check(
    Array.isArray(search.json) && search.json.some((m) => m.id === msgId),
    "FTS5 search hits the just-sent message",
  );

  // 4. PUT blackboard
  const tasksContent = `# tasks\n- [ ] foo ${tag}\n`;
  const putRes = await putJson(`${BASE}/api/blackboard/tasks.md`, {
    agent_id: a,
    content: tasksContent,
  });
  check(putRes.status === 200, "PUT /api/blackboard/tasks.md returns 200");
  check(
    putRes.json && typeof putRes.json.sha256 === "string",
    "PUT response carries sha256",
  );

  // 5. GET blackboard
  const getRes = await getJson(`${BASE}/api/blackboard/tasks.md`);
  check(getRes.status === 200, "GET /api/blackboard/tasks.md returns 200");
  check(
    getRes.json && getRes.json.content === tasksContent,
    "GET blackboard content matches what we wrote",
  );

  // 6. ws/swarm should have already seen the BlackboardChanged{op:"write"}
  const writeEv = await collector.waitFor(
    (e) =>
      e.type === "blackboard_changed" &&
      e.path === "tasks.md" &&
      e.op === "write",
    2000,
  );
  check(!!writeEv, "ws/swarm observed BlackboardChanged{op:'write'} for tasks.md");

  // 7. External write — write outside the API; watcher should pick it up.
  const externalRel = `external-${Date.now()}.md`;
  const externalAbs = path.join(BLACKBOARD_ROOT, externalRel);
  const externalBody = `external edit ${tag}\n`;
  await writeFile(externalAbs, externalBody, "utf8");
  const extEv = await collector.waitFor(
    (e) =>
      e.type === "blackboard_changed" &&
      e.path === externalRel &&
      e.op === "external",
    3000,
  );
  check(
    !!extEv,
    "ws/swarm observed BlackboardChanged{op:'external'} after external fs.writeFile",
  );

  // 8. Path traversal: server must reject without writing.
  //    Encode the slash and dots so fetch doesn't normalise them away.
  const traversal = await putJson(
    `${BASE}/api/blackboard/${encodeURIComponent("../escape.md")}`,
    { content: "should not exist" },
  );
  check(
    traversal.status === 400,
    `PUT /api/blackboard/<traversal> returns 400 (got ${traversal.status})`,
  );

  // 9. Also see Message event in the broadcast.
  const msgEv = await collector.waitFor(
    (e) => e.type === "message" && e.id === msgId,
    1500,
  );
  check(!!msgEv, "ws/swarm observed Message broadcast for the sent message");

  // Cleanup.
  await killAgent(a);
  await killAgent(b);
  await rm(externalAbs, { force: true });
  collector.close();

  // 10. Final assertion: GET /api/agent should now contain both agents
  //     with killed_at populated (history-backed view).
  await pause(200);
  const agents = await getJson(`${BASE}/api/agent`);
  check(agents.status === 200, "GET /api/agent (post-kill) returns 200");
  const hasA = Array.isArray(agents.json)
    ? agents.json.find((r) => r.agent_id === a)
    : null;
  const hasB = Array.isArray(agents.json)
    ? agents.json.find((r) => r.agent_id === b)
    : null;
  check(!!hasA && hasA.killed_at != null, "agent A is in /api/agent with killed_at");
  check(!!hasB && hasB.killed_at != null, "agent B is in /api/agent with killed_at");

  if (failures.length === 0) {
    console.log("\nSWARM SMOKE: PASS");
    process.exit(0);
  } else {
    console.error(`\nSWARM SMOKE: FAIL (${failures.length} check(s) failed)`);
    for (const f of failures) console.error(`  - ${f}`);
    process.exit(1);
  }
}

main().catch((e) => {
  console.error("crash", e);
  process.exit(2);
});
