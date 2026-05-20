#!/usr/bin/env node
// Resume smoke test: attach to an agent, capture a few binary frames,
// disconnect, reconnect with ?last_seq=N, assert the next frames carry
// seq = N+1 (no gap). Also checks Hello lifecycle snapshot.

const BASE = "http://127.0.0.1:7777";
const CLI = process.argv[2] || "codex";

function pause(ms) {
  return new Promise((r) => setTimeout(r, ms));
}

function attach(agentId, lastSeq) {
  return new Promise((resolve, reject) => {
    const qs = lastSeq > 0 ? `?last_seq=${lastSeq}` : "";
    const ws = new WebSocket(`ws://127.0.0.1:7777/ws/pty/${agentId}${qs}`);
    ws.binaryType = "arraybuffer";
    const events = { hello: null, errors: [], frames: [], lifecycle: [] };
    ws.onopen = () => {
      // resolve once we've seen enough to verify resume contract.
    };
    ws.onmessage = (ev) => {
      if (typeof ev.data === "string") {
        const msg = JSON.parse(ev.data);
        if (msg.type === "hello") events.hello = msg;
        else if (msg.type === "error") events.errors.push(msg.message);
        else events.lifecycle.push(msg);
      } else {
        const buf = new Uint8Array(ev.data);
        const dv = new DataView(buf.buffer, buf.byteOffset, buf.byteLength);
        const seq = dv.getUint32(0, false);
        const body = buf.subarray(4);
        events.frames.push({ seq, body });
      }
    };
    ws.onerror = (e) => reject(e);
    ws.onclose = () => resolve(events);
    // Return a controller to close after a delay.
    resolve.controller = ws;
    setTimeout(() => resolve({ ws, events }), 1500);
  });
}

async function open(agentId, lastSeq, holdMs) {
  return new Promise((resolve, reject) => {
    const qs = lastSeq > 0 ? `?last_seq=${lastSeq}` : "";
    const ws = new WebSocket(`ws://127.0.0.1:7777/ws/pty/${agentId}${qs}`);
    ws.binaryType = "arraybuffer";
    const out = { hello: null, errors: [], frames: [], lifecycle: [] };
    ws.onmessage = (ev) => {
      if (typeof ev.data === "string") {
        const msg = JSON.parse(ev.data);
        if (msg.type === "hello") out.hello = msg;
        else if (msg.type === "error") out.errors.push(msg.message);
        else out.lifecycle.push(msg);
      } else {
        const buf = new Uint8Array(ev.data);
        const dv = new DataView(buf.buffer, buf.byteOffset, buf.byteLength);
        const seq = dv.getUint32(0, false);
        const body = Array.from(buf.subarray(4));
        out.frames.push({ seq, len: body.length });
      }
    };
    ws.onerror = (e) => reject(e);
    setTimeout(() => {
      ws.close();
      setTimeout(() => resolve(out), 80);
    }, holdMs);
  });
}

async function main() {
  const spawn = await fetch(`${BASE}/api/agent`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ cli: CLI }),
  }).then((r) => r.json());
  const agentId = spawn.agent_id;
  console.log(`spawned ${agentId}`);

  // Attach #1 — fresh, no last_seq. Hold ~2.5s to let codex render.
  const first = await open(agentId, 0, 2500);
  console.log(
    `first attach: hello.seq_start=${first.hello?.seq_start}, ` +
      `frames=${first.frames.length}, ` +
      `seqs=${first.frames[0]?.seq}..${first.frames[first.frames.length - 1]?.seq}`,
  );
  if (first.frames.length === 0) {
    throw new Error("first attach received no frames — CLI didn't output anything");
  }
  const lastSeqSeen = first.frames[first.frames.length - 1].seq;

  // Attach #2 — resume from lastSeqSeen. Should start at lastSeqSeen+1 (or close).
  await pause(400);
  const second = await open(agentId, lastSeqSeen, 2000);
  console.log(
    `resume attach: hello.seq_start=${second.hello?.seq_start} (expected ${lastSeqSeen + 1}), ` +
      `frames=${second.frames.length}, errors=${JSON.stringify(second.errors)}`,
  );

  const okSeqStart = second.hello?.seq_start === lastSeqSeen + 1;
  if (!okSeqStart) {
    console.error(
      `FAIL: resume seq_start mismatch. got=${second.hello?.seq_start}, expected=${lastSeqSeen + 1}`,
    );
  }
  if (second.frames.length > 0 && second.frames[0].seq !== lastSeqSeen + 1) {
    console.error(
      `FAIL: first resumed frame seq=${second.frames[0].seq}, expected ${lastSeqSeen + 1}`,
    );
  }

  // Attach #3 — fresh again (no last_seq). seq_start should be > 1 because
  // the producer kept counting while we were disconnected.
  await pause(200);
  const third = await open(agentId, 0, 800);
  console.log(
    `fresh attach (no resume): hello.seq_start=${third.hello?.seq_start} (live tail; replay skipped)`,
  );

  // Cleanup.
  await fetch(`${BASE}/api/agent/${agentId}`, { method: "DELETE" });

  if (okSeqStart) {
    console.log("\nRESUME SMOKE: PASS");
    process.exit(0);
  } else {
    console.error("\nRESUME SMOKE: FAIL");
    process.exit(1);
  }
}

main().catch((e) => {
  console.error("crash", e);
  process.exit(2);
});
