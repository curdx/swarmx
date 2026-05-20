// 6-agent PTY 不串扰压测.
//
//   1. Spawn 6 fresh codex agents.
//   2. For each, open /ws/pty/:id, capture all binary frame bytes.
//   3. Wait for shim_ready.
//   4. Send unique input marker via WS (binary).
//   5. After settle, assert per-agent capture contains its own marker
//      AND does NOT contain any other agent's marker.
//   6. Cleanup: DELETE each agent.
//
// Run: node /tmp/flockmux-stress-6.mjs

const BASE = "http://127.0.0.1:7777";
const WS_BASE = "ws://127.0.0.1:7777";
const N = 6;
const SETTLE_MS = 4000;

async function spawn() {
  const res = await fetch(`${BASE}/api/agent`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ cli: "codex" }),
  });
  if (!res.ok) throw new Error(`spawn failed: ${res.status} ${await res.text()}`);
  return await res.json();
}

async function kill(id) {
  try {
    await fetch(`${BASE}/api/agent/${encodeURIComponent(id)}`, { method: "DELETE" });
  } catch {}
}

function bufferOf(chunks) {
  return Buffer.concat(chunks.map((c) => Buffer.from(c)));
}

async function runOne(idx) {
  const agent = await spawn();
  const id = agent.agent_id;
  const chunks = []; // Buffer[]
  const marker = `MARK-${idx}-FM`;
  let shimReady = false;

  const ws = new WebSocket(`${WS_BASE}/ws/pty/${encodeURIComponent(id)}`);
  ws.binaryType = "arraybuffer";

  const readyPromise = new Promise((resolve, reject) => {
    const t = setTimeout(() => reject(new Error(`agent ${idx} (${id}) shim_ready timeout`)), 15000);
    ws.addEventListener("message", (ev) => {
      if (typeof ev.data === "string") {
        try {
          const msg = JSON.parse(ev.data);
          if (msg.type === "shim_ready") {
            shimReady = true;
            clearTimeout(t);
            resolve();
          }
        } catch {}
        return;
      }
      // binary: [4B seq][bytes]
      const buf = Buffer.from(ev.data);
      if (buf.length >= 4) chunks.push(buf.subarray(4));
    });
    ws.addEventListener("error", (e) => {
      clearTimeout(t);
      reject(new Error(`ws error agent ${idx}: ${e?.message ?? e}`));
    });
  });

  await new Promise((resolve, reject) => {
    ws.addEventListener("open", resolve, { once: true });
    ws.addEventListener("error", reject, { once: true });
  });
  await readyPromise;

  // Send unique marker as binary (UTF-8). Codex's TUI will reflect this in
  // its input bar — the PTY echoes those bytes back to us.
  ws.send(new TextEncoder().encode(marker));

  return { idx, id, ws, chunks, marker };
}

async function settle(ms) {
  return new Promise((r) => setTimeout(r, ms));
}

async function main() {
  console.log(`[stress] spawning ${N} agents…`);
  const results = [];
  // Spawn sequentially so server log stays readable; could parallelise.
  for (let i = 0; i < N; i++) {
    try {
      const r = await runOne(i);
      results.push(r);
      console.log(`  agent ${i} ready: ${r.id}`);
    } catch (err) {
      console.error(`  agent ${i} setup failed:`, err.message);
    }
  }
  if (results.length !== N) {
    console.error(`only ${results.length}/${N} agents came up; cleaning up`);
    for (const r of results) {
      try { r.ws.close(); } catch {}
      await kill(r.id);
    }
    process.exit(2);
  }
  console.log(`[stress] waiting ${SETTLE_MS}ms for TUI redraw…`);
  await settle(SETTLE_MS);

  let pass = true;
  for (const r of results) {
    const all = bufferOf(r.chunks).toString("utf8");
    const hasOwn = all.includes(r.marker);
    const foreign = results
      .filter((o) => o.idx !== r.idx)
      .map((o) => o.marker)
      .filter((m) => all.includes(m));
    const bytes = all.length;
    const summary = `agent ${r.idx} (${r.id.slice(0, 16)}…) bytes=${bytes} own=${hasOwn ? "YES" : "NO"} foreign=${foreign.length ? foreign.join(",") : "none"}`;
    if (!hasOwn || foreign.length) {
      console.error(`  FAIL ${summary}`);
      pass = false;
    } else {
      console.log(`  ok   ${summary}`);
    }
  }

  console.log("[stress] cleaning up…");
  for (const r of results) {
    try { r.ws.close(); } catch {}
    await kill(r.id);
  }

  console.log(pass ? "[stress] PASS" : "[stress] FAIL");
  process.exit(pass ? 0 : 1);
}

main().catch((e) => {
  console.error("fatal:", e);
  process.exit(3);
});
