import { describe, expect, it } from "vitest";
import type { MessageRecord } from "../api/types";
import { mergeServerSnapshot } from "./messageMerge";

/** Minimal MessageRecord fixture; only id + thread_id matter for the merge. */
function msg(id: number, thread: string | null = "X"): MessageRecord {
  return {
    id,
    from_agent: "claude-1",
    to_agent: "user",
    kind: "reply",
    body: `m${id}`,
    sent_at: id, // monotone with id is fine for these tests
    delivered_at: null,
    read_at: null,
    in_reply_to: null,
    thread_id: thread,
  };
}

const ids = (rows: MessageRecord[]) => rows.map((m) => m.id);

describe("mergeServerSnapshot", () => {
  it("server superset → clean adopt, no preserves, addedCount counts new rows", () => {
    const r = mergeServerSnapshot([msg(1), msg(2)], [msg(1), msg(2), msg(3)], "X");
    expect(ids(r.next)).toEqual([1, 2, 3]);
    expect(r.preservedIds).toEqual([]);
    expect(r.lostSameThread).toEqual([]);
    expect(r.addedCount).toBe(1);
  });

  it("RESCUES an in-window race victim (the BUG4 fix the old id>maxServerId dropped)", () => {
    // Local has optimistic id=100; server snapshot committed it AFTER the read,
    // so it returns 95..99 + 101..105 (a gap at 100) — 100 is inside the window.
    const prev = [msg(99), msg(100)];
    const server = [msg(95), msg(96), msg(97), msg(98), msg(99), msg(101), msg(105)];
    const r = mergeServerSnapshot(prev, server, "X");
    expect(ids(r.next)).toContain(100); // NOT dropped
    expect(r.preservedIds).toEqual([100]);
    expect(r.lostSameThread).toEqual([]); // no false regression signal
    // stays in chronological (id) order
    expect(ids(r.next)).toEqual([95, 96, 97, 98, 99, 100, 101, 105]);
  });

  it("direction switch → clean replace, no cross-thread leakage", () => {
    const prev = [msg(1, "A"), msg(2, "A"), msg(3, "A")];
    const server = [msg(10, "B"), msg(11, "B")];
    const r = mergeServerSnapshot(prev, server, "B");
    expect(ids(r.next)).toEqual([10, 11]);
    expect(r.preservedIds).toEqual([]);
    expect(r.lostSameThread).toEqual([]); // dropped rows are thread A ≠ active B
  });

  it("drops genuinely-older same-thread history that scrolled out of the window", () => {
    // id=50 is below the server window [100..101] → correctly dropped.
    const prev = [msg(50), msg(100), msg(101)];
    const server = [msg(100), msg(101)];
    const r = mergeServerSnapshot(prev, server, "X");
    expect(ids(r.next)).toEqual([100, 101]);
    expect(r.droppedIds).toEqual([50]);
  });

  it("empty snapshot keeps an optimistic send to a brand-new thread", () => {
    const r = mergeServerSnapshot([msg(5)], [], "X");
    expect(ids(r.next)).toEqual([5]);
    expect(r.preservedIds).toEqual([5]);
  });

  it("empty snapshot on direction switch to an empty thread → empty, no leak", () => {
    const r = mergeServerSnapshot([msg(1, "A")], [], "B");
    expect(ids(r.next)).toEqual([]);
    expect(r.preservedIds).toEqual([]);
  });

  it("null thread_id (main) matches null activeThreadId", () => {
    const prev = [msg(7, null)];
    const r = mergeServerSnapshot(prev, [msg(5, null)], null);
    expect(ids(r.next)).toEqual([5, 7]); // id=7 is a race victim ≥ min(5)
    expect(r.preservedIds).toEqual([7]);
  });
});
