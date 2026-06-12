import { describe, it, expect } from "vitest";
import type { MessageRecord } from "../api/types";
import { applyMarkedRead, collectObservableIds } from "./markReadBatch";

function msg(
  p: Partial<MessageRecord> &
    Pick<MessageRecord, "id" | "from_agent" | "to_agent">,
): MessageRecord {
  return {
    kind: "chat",
    body: "",
    sent_at: p.id,
    delivered_at: null,
    read_at: null,
    in_reply_to: null,
    ...p,
  };
}

describe("collectObservableIds", () => {
  it("returns only unread, user-bound message ids", () => {
    const items = [
      msg({ id: 1, from_agent: "a", to_agent: "user", read_at: null }), // ✓ unread → user
      msg({ id: 2, from_agent: "a", to_agent: "user", read_at: 5 }), // ✗ already read
      msg({ id: 3, from_agent: "user", to_agent: "a", read_at: null }), // ✗ user → agent
      msg({ id: 4, from_agent: "b", to_agent: "user", read_at: null }), // ✓ unread → user
    ];
    expect(collectObservableIds(items)).toEqual([1, 4]);
  });

  it("is empty when nothing is unread/user-bound", () => {
    expect(collectObservableIds([])).toEqual([]);
    expect(
      collectObservableIds([
        msg({ id: 1, from_agent: "a", to_agent: "user", read_at: 9 }),
      ]),
    ).toEqual([]);
  });
});

describe("applyMarkedRead", () => {
  it("stamps read_at on marked ids only", () => {
    const items = [
      msg({ id: 1, from_agent: "a", to_agent: "user", read_at: null }),
      msg({ id: 2, from_agent: "a", to_agent: "user", read_at: null }),
    ];
    const out = applyMarkedRead(items, [1], 999);
    expect(out[0].read_at).toBe(999);
    expect(out[1].read_at).toBe(null);
  });

  it("returns the SAME reference when nothing was marked (skips re-render)", () => {
    const items = [msg({ id: 1, from_agent: "a", to_agent: "user" })];
    expect(applyMarkedRead(items, [], 999)).toBe(items);
  });

  it("does not mutate the input (returns a fresh array + fresh marked rows)", () => {
    const items = [msg({ id: 1, from_agent: "a", to_agent: "user", read_at: null })];
    const out = applyMarkedRead(items, [1], 999);
    expect(items[0].read_at).toBe(null); // original row untouched
    expect(out).not.toBe(items);
    expect(out[0]).not.toBe(items[0]);
  });
});
