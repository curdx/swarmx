import { describe, it, expect } from "vitest";
import { roleDisplayName } from "./agent";

// Regression guard for the TaskActivity bug: the orchestrator spawns workers
// with FREE-FORM role labels ("Code Reviewer", "Backend Engineer"), and the
// dispatch card used to .join() them raw — leaking English into a zh UI. The
// fix maps each through roleDisplayName in the display layer. These assert the
// normalization that makes that fix work (spaced/title-cased → localized).
describe("roleDisplayName — orchestrator-minted free-form roles", () => {
  it("normalizes spaced/title-cased labels to the known role", () => {
    // "Code Reviewer" must resolve to the SAME label as the canonical slug
    // "reviewer" — proving the spaced spelling isn't left as raw English.
    expect(roleDisplayName("Code Reviewer")).toBe(roleDisplayName("reviewer"));
    expect(roleDisplayName("Backend Engineer")).toBe(roleDisplayName("backend"));
  });

  it("renders the orchestrator as the friendly captain label, not the slug", () => {
    expect(roleDisplayName("orchestrator")).not.toBe("orchestrator");
  });

  it("passes unknown custom roles through unchanged (no crash, no blanking)", () => {
    expect(roleDisplayName("Quantum Whisperer")).toBe("Quantum Whisperer");
  });

  it("simulates the TaskActivity join — no raw English slug survives", () => {
    const spawnedRoles = ["Code Reviewer", "Backend Engineer"];
    const joined = spawnedRoles.map(roleDisplayName).join(" · ");
    // The canonical-slug labels must appear (localized), proving the map ran.
    expect(joined).toContain(roleDisplayName("reviewer"));
    expect(joined).toContain(roleDisplayName("backend"));
  });
});
