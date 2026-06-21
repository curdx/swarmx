#!/usr/bin/env node
// Directions (threads) live-refresh smoke / regression for 6cf02d5.
//
// Root cause it guards: a direction's state can change *server-side* (the
// orchestrator's `swarm_name_thread` → background worktree isolation, a delete,
// etc.) with no UI action behind it. The sidebar only refetches on a push, so
// every such transition MUST emit `SwarmEvent::ThreadChanged` over /ws/swarm or
// the sidebar silently goes stale (the original bug). This drives the three
// spawn-free transitions and asserts each one broadcasts:
//   1. POST   …/threads            → ThreadChanged{op:"created"}
//   2. PATCH  …/threads/<main>     → ThreadChanged{op:"updated"}   (rename main:
//        slug=="main" ⇒ no git isolation ⇒ no real-CLI spawn — see
//        update_thread_handler `should_isolate`)
//   3. DELETE …/threads/<new>      → ThreadChanged{op:"deleted"}
//
// The 4th op, "isolated", rides the SAME publish_thread_changed → broadcast →
// WS-frame path, but reaching it spawns a real orchestrator CLI
// (reroot_thread_orchestrator → run_spell "init"), so it is deliberately out of
// scope for a zero-token smoke; the path itself is proven by the three above.
//
// Assumptions: swarmx-server is running at 127.0.0.1:${SWARMX_PORT:-7777}.
// Run it against a THROWAWAY data dir (separate $HOME + SWARMX_PORT) so it
// never touches the user's real ~/.swarmx.

import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";

const PORT = process.env.SWARMX_PORT || "7777";
const BASE = `http://127.0.0.1:${PORT}`;
const WS_SWARM = `ws://127.0.0.1:${PORT}/ws/swarm`;

const failures = [];
function check(cond, msg) {
  if (!cond) {
    failures.push(msg);
    console.error(`FAIL: ${msg}`);
  } else {
    console.log(`  ok: ${msg}`);
  }
}

async function req(method, url, body) {
  const res = await fetch(url, {
    method,
    headers: body ? { "content-type": "application/json" } : undefined,
    body: body ? JSON.stringify(body) : undefined,
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

const pause = (ms) => new Promise((r) => setTimeout(r, ms));

class SwarmEventCollector {
  constructor() {
    this.events = [];
    this.ws = null;
    this.ready = new Promise((resolve) => (this._resolveReady = resolve));
  }
  async start() {
    this.ws = new WebSocket(WS_SWARM);
    this.ws.onmessage = (ev) => {
      if (typeof ev.data !== "string") return;
      try {
        this.events.push(JSON.parse(ev.data));
      } catch (e) {
        console.warn("non-JSON ws frame", ev.data, e);
      }
    };
    this.ws.onopen = () => this._resolveReady();
    this.ws.onerror = (e) => console.warn("ws swarm error", e?.message ?? e);
    await this.ready;
    await pause(50); // let the subscribe fully wire server-side
  }
  close() {
    if (this.ws) this.ws.close();
  }
  async waitFor(pred, timeoutMs = 3000) {
    const deadline = Date.now() + timeoutMs;
    while (Date.now() < deadline) {
      const hit = this.events.find(pred);
      if (hit) return hit;
      await pause(50);
    }
    return null;
  }
}

const threadChanged = (wsId, threadId, op) => (e) =>
  e.type === "thread_changed" &&
  e.workspace_id === wsId &&
  e.thread_id === threadId &&
  e.op === op;

async function main() {
  const cwd = await mkdtemp(path.join(tmpdir(), "swarmx-dir-smoke-"));
  console.log(`workspace cwd: ${cwd}  (server ${BASE})`);

  const collector = new SwarmEventCollector();
  await collector.start();
  console.log("ws/swarm subscribed");

  // 0. Workspace (auto-creates a `main` thread).
  const ws = await req("POST", `${BASE}/api/workspaces`, {
    name: `dir-smoke-${Date.now().toString(36)}`,
    cwd,
  });
  check(ws.status === 200, `POST /api/workspaces → 200 (got ${ws.status})`);
  const wsId = ws.json?.id;
  check(!!wsId, "workspace has an id");

  const threads = await req("GET", `${BASE}/api/workspaces/${wsId}/threads`);
  const mainThread = Array.isArray(threads.json)
    ? threads.json.find((t) => t.slug === "main")
    : null;
  check(!!mainThread, "workspace auto-created a `main` direction");

  // 1. Create a direction → ThreadChanged{created}.
  const created = await req("POST", `${BASE}/api/workspaces/${wsId}/threads`, {});
  check(created.status === 200, `POST …/threads → 200 (got ${created.status})`);
  const newId = created.json?.id;
  check(!!newId, "new direction has an id");
  const createdEv = await collector.waitFor(threadChanged(wsId, newId, "created"));
  check(!!createdEv, "ws/swarm observed ThreadChanged{op:'created'} for the new direction");

  // 2. Rename `main` → ThreadChanged{updated}. slug=="main" ⇒ pure rename, no
  //    isolation, no real-CLI spawn.
  const renamed = await req(
    "PATCH",
    `${BASE}/api/workspaces/${wsId}/threads/${mainThread.id}`,
    { name: "renamed main" },
  );
  check(renamed.status === 200, `PATCH …/threads/main → 200 (got ${renamed.status})`);
  const updatedEv = await collector.waitFor(threadChanged(wsId, mainThread.id, "updated"));
  check(!!updatedEv, "ws/swarm observed ThreadChanged{op:'updated'} for the renamed main");

  // 3. Delete the created direction → ThreadChanged{deleted}.
  const deleted = await req(
    "DELETE",
    `${BASE}/api/workspaces/${wsId}/threads/${newId}`,
  );
  check(
    deleted.status === 204 || deleted.status === 200,
    `DELETE …/threads/<new> → 2xx (got ${deleted.status})`,
  );
  const deletedEv = await collector.waitFor(threadChanged(wsId, newId, "deleted"));
  check(!!deletedEv, "ws/swarm observed ThreadChanged{op:'deleted'} for the deleted direction");

  // Cleanup (best-effort): drop the workspace + temp cwd.
  await req("DELETE", `${BASE}/api/workspaces/${wsId}`);
  collector.close();
  await rm(cwd, { recursive: true, force: true });

  if (failures.length === 0) {
    console.log("\nDIRECTIONS SMOKE: PASS");
    process.exit(0);
  } else {
    console.error(`\nDIRECTIONS SMOKE: FAIL (${failures.length} check(s) failed)`);
    for (const f of failures) console.error(`  - ${f}`);
    process.exit(1);
  }
}

main().catch((e) => {
  console.error("crash", e);
  process.exit(2);
});
