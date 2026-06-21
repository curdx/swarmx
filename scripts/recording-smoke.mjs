#!/usr/bin/env node
// Recording smoke test (M3 #19):
//   1. spawn a codex agent
//   2. POST /api/message lets us learn its agent_id (already returned by spawn)
//   3. GET  /api/recording?agent_id=<id>  → exactly 1 entry, finalized_at=null
//      while alive
//   4. DELETE /api/agent/:id              → triggers PTY EOF + recorder finalize
//   5. Poll /api/recording?agent_id=…     → finalized_at + duration_ms populated
//      (best-effort; finalize is async, give it ~2s)
//   6. GET  /api/recording/:id            → returns the .cast file body
//   7. Parse the body as JSON-lines:
//        - line 0 is the asciicast v2 header {version: 2, width, height, ...}
//        - lines 1.. are [delta, "o", data] arrays
//   8. Assert ≥1 event line and at least one event contains the OSC ready
//      sequence \x1b]633;A\x07 (the shim emits it before exec) — proves the
//      recorder captured the OSC lifecycle markers.
//   9. Cleanup: best-effort delete the .cast file from disk (the recorder
//      lives under SWARMX_RECORDINGS_DIR if set; we don't poke the filesystem
//      directly here — the SQLite row stays around as history).
//
// Assumptions:
//   * swarmx-server is running at 127.0.0.1:7777.
//   * `codex` is on PATH and previously authenticated (or `claude` if argv[2]).

const BASE = "http://127.0.0.1:7777";
const CLI = process.argv[2] || "codex";

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

async function getText(url) {
  const res = await fetch(url);
  const text = await res.text();
  return { status: res.status, body: text };
}

async function killAgent(id) {
  await fetch(`${BASE}/api/agent/${encodeURIComponent(id)}`, { method: "DELETE" });
}

function pause(ms) {
  return new Promise((r) => setTimeout(r, ms));
}

async function pollFinalized(id, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const r = await getJson(`${BASE}/api/recording/${encodeURIComponent(id)}`);
    // get_recording returns the .cast bytes, not the row — use list instead.
    const list = await getJson(`${BASE}/api/recording`);
    const row = Array.isArray(list.json) ? list.json.find((x) => x.id === id) : null;
    if (row && row.finalized_at != null) return row;
    await pause(150);
  }
  return null;
}

async function main() {
  console.log(`spawning ${CLI} agent…`);
  const spawn = await postJson(`${BASE}/api/agent`, { cli: CLI });
  if (spawn.status !== 200) {
    console.error(`spawn failed: ${spawn.status} ${JSON.stringify(spawn.json)}`);
    process.exit(2);
  }
  const agentId = spawn.json.agent_id;
  console.log(`spawned ${agentId}`);

  // Give the shim a moment to emit OSC ready + the inner CLI to print its
  // banner / first prompt so the .cast file has content.
  await pause(1500);

  // 1. List recordings for this agent — should have exactly one, still live.
  const listed = await getJson(
    `${BASE}/api/recording?agent_id=${encodeURIComponent(agentId)}`,
  );
  check(listed.status === 200, "GET /api/recording?agent_id=… returns 200");
  check(
    Array.isArray(listed.json) && listed.json.length === 1,
    `exactly one recording for ${agentId} (got ${
      Array.isArray(listed.json) ? listed.json.length : "non-array"
    })`,
  );
  const rec = Array.isArray(listed.json) ? listed.json[0] : null;
  if (!rec) {
    console.error("no recording returned — aborting smoke");
    await killAgent(agentId);
    process.exit(1);
  }
  check(rec.agent_id === agentId, "recording.agent_id matches");
  check(rec.cols === 120 && rec.rows === 32, "recording cols/rows are 120x32");
  check(rec.finalized_at == null, "recording is live (finalized_at == null)");

  // 2. Kill the agent → recorder sees EOF, writer task finalizes.
  await killAgent(agentId);

  // 3. Wait for finalize_at to land in SQLite.
  const finalized = await pollFinalized(rec.id, 3000);
  check(!!finalized, "recording finalized within 3s of kill");
  if (finalized) {
    check(
      typeof finalized.duration_ms === "number" && finalized.duration_ms >= 0,
      `duration_ms populated (${finalized.duration_ms})`,
    );
    check(
      typeof finalized.last_seq === "number" && finalized.last_seq > 0,
      `last_seq > 0 bytes recorded (${finalized.last_seq})`,
    );
  }

  // 4. Fetch the cast bytes.
  const cast = await getText(`${BASE}/api/recording/${encodeURIComponent(rec.id)}`);
  check(cast.status === 200, "GET /api/recording/:id returns 200");
  const lines = cast.body.split("\n").filter((l) => l.length > 0);
  check(lines.length >= 2, `cast has ≥1 header + ≥1 event (got ${lines.length} lines)`);

  // 5. Parse header.
  let header;
  try {
    header = JSON.parse(lines[0]);
  } catch (e) {
    check(false, `header is JSON: ${e?.message ?? e}`);
    header = {};
  }
  check(header.version === 2, "asciicast header.version === 2");
  check(header.width === 120, "header.width === 120");
  check(header.height === 32, "header.height === 32");
  check(typeof header.timestamp === "number", "header.timestamp is a number");

  // 6. Parse events.
  let events = [];
  for (let i = 1; i < lines.length; i++) {
    try {
      const ev = JSON.parse(lines[i]);
      if (Array.isArray(ev) && ev.length === 3 && ev[1] === "o") {
        events.push(ev);
      }
    } catch (e) {
      console.warn(`  warn: line ${i} not JSON: ${e?.message ?? e}`);
    }
  }
  check(events.length > 0, `≥1 output event recorded (got ${events.length})`);

  // 7. At least one event must contain the shim's OSC ready marker.
  // \x1b]633;A\x07 — emitted by swarmx-shim before exec.
  const concat = events.map((e) => e[2]).join("");
  check(
    concat.includes("\x1b]633;A\x07"),
    "asciicast captured the shim OSC ready marker (\\x1b]633;A\\x07)",
  );

  // Deltas should be non-decreasing.
  let prev = -1;
  let monotonic = true;
  for (const ev of events) {
    if (typeof ev[0] !== "number" || ev[0] < prev) {
      monotonic = false;
      break;
    }
    prev = ev[0];
  }
  check(monotonic, "event deltas are non-decreasing");

  if (failures.length === 0) {
    console.log("\nRECORDING SMOKE: PASS");
    process.exit(0);
  } else {
    console.error(`\nRECORDING SMOKE: FAIL (${failures.length} check(s))`);
    for (const f of failures) console.error(`  - ${f}`);
    process.exit(1);
  }
}

main().catch((e) => {
  console.error("crash", e);
  process.exit(2);
});
