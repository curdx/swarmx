import { describe, it, expect } from "vitest";
import { loadDraft, saveDraft } from "./draftStore";

/** Map-backed Storage stand-in — enough of the Web Storage surface to drive
 *  loadDraft/saveDraft without a DOM. */
function fakeStorage() {
  const m = new Map<string, string>();
  return {
    store: m,
    getItem: (k: string): string | null => (m.has(k) ? m.get(k)! : null),
    setItem: (k: string, v: string): void => void m.set(k, v),
    removeItem: (k: string): void => void m.delete(k),
  };
}

describe("loadDraft", () => {
  it("returns the saved value", () => {
    const s = fakeStorage();
    s.store.set("k", "hello");
    expect(loadDraft(s, "k")).toBe("hello");
  });

  it("returns empty string when the key is absent", () => {
    expect(loadDraft(fakeStorage(), "missing")).toBe("");
  });

  it("swallows storage errors and returns empty string", () => {
    const throwing = {
      getItem(): string | null {
        throw new Error("storage disabled");
      },
    };
    expect(loadDraft(throwing, "k")).toBe("");
  });
});

describe("saveDraft", () => {
  it("writes a non-blank draft", () => {
    const s = fakeStorage();
    saveDraft(s, "k", "draft text");
    expect(s.store.get("k")).toBe("draft text");
  });

  it("removes the key when the draft is empty", () => {
    const s = fakeStorage();
    s.store.set("k", "old");
    saveDraft(s, "k", "");
    expect(s.store.has("k")).toBe(false);
  });

  it("treats whitespace-only as blank and removes the key", () => {
    const s = fakeStorage();
    s.store.set("k", "old");
    saveDraft(s, "k", "   \n\t ");
    expect(s.store.has("k")).toBe(false);
  });

  it("swallows storage errors without throwing (set and remove paths)", () => {
    const throwing = {
      setItem(): void {
        throw new Error("quota exceeded");
      },
      removeItem(): void {
        throw new Error("quota exceeded");
      },
    };
    expect(() => saveDraft(throwing, "k", "x")).not.toThrow();
    expect(() => saveDraft(throwing, "k", "")).not.toThrow();
  });
});
