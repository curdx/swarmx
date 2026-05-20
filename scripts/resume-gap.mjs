#!/usr/bin/env node
// Resume GAP smoke: spawn codex, capture seq=1, then send a yes-equivalent
// burst by spamming printf via stdin so the ring buffer >1MiB and evicts the
// early bytes. Reconnect with last_seq=1 → expect an Error frame + Hello
// seq_start > 2.

const BASE = "http://127.0.0.1:7777";

function pause(ms) {
  return new Promise((r) => setTimeout(r, ms));
}

function attach(agentId, lastSeq, holdMs, opts = {}) {
  return new Promise((resolve, reject) => {
    const qs = lastSeq > 0 ? `?last_seq=${lastSeq}` : "";
    const ws = new WebSocket(`ws://127.0.0.1:7777/ws/pty/${agentId}${qs}`);
    ws.binaryType = "arraybuffer";
    const out = { hello: null, errors: [], frames: [], totalBytes: 0 };
    ws.onopen = () => {
      if (opts.onOpen) opts.onOpen(ws);
    };
    ws.onmessage = (ev) => {
      if (typeof ev.data === "string") {
        const msg = JSON.parse(ev.data);
        if (msg.type === "hello") out.hello = msg;
        else if (msg.type === "error") out.errors.push(msg.message);
      } else {
        const buf = new Uint8Array(ev.data);
        const dv = new DataView(buf.buffer, buf.byteOffset, buf.byteLength);
        const seq = dv.getUint32(0, false);
        const len = buf.byteLength - 4;
        out.frames.push({ seq, len });
        out.totalBytes += len;
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
    body: JSON.stringify({ cli: "codex" }),
  }).then((r) => r.json());
  const agentId = spawn.agent_id;
  console.log(`spawned ${agentId}`);

  // First attach: short, just to learn that seq=1 was the first byte.
  const first = await attach(agentId, 0, 1500);
  const firstSeq = first.frames[0]?.seq ?? 1;
  const lastSeqSeen = first.frames[first.frames.length - 1]?.seq ?? 0;
  console.log(
    `first: seq_start=${first.hello?.seq_start}, first frame seq=${firstSeq}, last seq=${lastSeqSeen}, bytes=${first.totalBytes}`,
  );

  // Now generate >1MiB of PTY output by sending an injection that codex
  // would echo. Easier path: send a flood of unique input characters so
  // codex echoes them through its TUI. We pump a 200KB string ~10 times.
  await attach(agentId, lastSeqSeen, 5000, {
    onOpen: (ws) => {
      const big = "X".repeat(64 * 1024);
      let n = 0;
      const t = setInterval(() => {
        if (ws.readyState !== WebSocket.OPEN) {
          clearInterval(t);
          return;
        }
        // Send raw bytes — codex's TUI may not echo X's directly, but
        // OSC/cursor redraws + the input itself add up fast on each keystroke.
        ws.send(new TextEncoder().encode(big));
        n += big.length;
        if (n > 3 * 1024 * 1024) clearInterval(t);
      }, 50);
    },
  });
  console.log(`flood phase done`);

  await pause(300);

  // Reconnect asking for the very first seq we saw.
  const gapAttach = await attach(agentId, firstSeq, 1500);
  console.log(
    `gap attach: hello.seq_start=${gapAttach.hello?.seq_start}, errors=${JSON.stringify(gapAttach.errors)}, frames=${gapAttach.frames.length}`,
  );

  await fetch(`${BASE}/api/agent/${agentId}`, { method: "DELETE" });

  // Pass condition: server either (a) replayed firstSeq+1 cleanly because
  // the buffer still holds it, OR (b) reported a gap with an error and a
  // jumped seq_start.
  const gappy =
    gapAttach.errors.length > 0 &&
    gapAttach.hello &&
    gapAttach.hello.seq_start > firstSeq + 1;
  const clean =
    gapAttach.errors.length === 0 &&
    gapAttach.hello &&
    gapAttach.hello.seq_start === firstSeq + 1;

  if (gappy) {
    console.log("RESUME GAP: PASS (server reported gap as expected)");
    process.exit(0);
  } else if (clean) {
    console.log(
      "RESUME GAP: NEUTRAL (buffer wasn't pressured enough to evict; replay was clean)",
    );
    process.exit(0);
  } else {
    console.error("RESUME GAP: FAIL", gapAttach);
    process.exit(1);
  }
}

main().catch((e) => {
  console.error("crash", e);
  process.exit(2);
});
