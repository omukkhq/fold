import { describe, expect, it } from "vitest";
import { diffLineStyle, diffRowClass, type DiffLineKind } from "./diffLine";

const KINDS: DiffLineKind[] = ["add", "remove", "context", "hunk"];

describe("diffLineStyle", () => {
  it("signs additions and removals", () => {
    expect(diffLineStyle("add").sign).toBe("+");
    expect(diffLineStyle("remove").sign).toBe("\u2212");
    expect(diffLineStyle("context").sign).toBe(" ");
    expect(diffLineStyle("hunk").sign).toBe("");
  });

  it.each(KINDS)("describes %s in words", (kind) => {
    expect(diffLineStyle(kind).description.length).toBeGreaterThan(0);
  });
});

describe("W06 diff rows", () => {
  it("draws only the hunk header across the number columns", () => {
    expect(KINDS.filter((kind) => diffLineStyle(kind).header)).toEqual([
      "hunk",
    ]);
  });

  it("signs an addition in primary text and a removal in danger", () => {
    expect(diffLineStyle("add").signClass).toBe("text-primary-text");
    expect(diffLineStyle("remove").signClass).toBe("text-danger");
  });
});

describe("diffRowClass", () => {
  it.each(KINDS)("uses the diff token of %s", (kind) => {
    expect(diffRowClass(kind, false)).toMatch(/^bg-diff-/);
  });

  it("uses the selected fill for every kind when selected", () => {
    for (const kind of KINDS) {
      expect(diffRowClass(kind, true)).toBe("bg-diff-selected");
    }
  });
});
