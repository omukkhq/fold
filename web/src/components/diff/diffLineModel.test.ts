import { describe, expect, it } from "vitest";
import {
  diffLineStyle,
  diffNumberClass,
  diffRowClass,
  type DiffLineKind,
} from "./diffLineModel";

const KINDS: DiffLineKind[] = ["added", "removed", "context", "hunk"];

describe("diffLineStyle", () => {
  it("signs additions and removals", () => {
    expect(diffLineStyle("added").sign).toBe("+");
    expect(diffLineStyle("removed").sign).toBe("\u2212");
    expect(diffLineStyle("context").sign).toBe(" ");
    expect(diffLineStyle("hunk").sign).toBe("");
  });

  it.each(KINDS)("describes %s in words", (kind) => {
    expect(diffLineStyle(kind).description.length).toBeGreaterThan(0);
  });
});

describe("diff rows", () => {
  it("draws only the hunk header across the number columns", () => {
    expect(KINDS.filter((kind) => diffLineStyle(kind).header)).toEqual([
      "hunk",
    ]);
  });

  it("signs an addition in primary text and a removal in danger", () => {
    expect(diffLineStyle("added").signClass).toBe("text-primary-text");
    expect(diffLineStyle("removed").signClass).toBe("text-danger");
  });
});

describe("diffRowClass", () => {
  it.each(KINDS)("uses the diff token of %s", (kind) => {
    expect(diffRowClass(kind, false)).toMatch(/^bg-diff-/);
  });

  it("uses the selected fill and the primary left edge for every kind when selected", () => {
    for (const kind of KINDS) {
      const classes = diffRowClass(kind, true).split(" ");
      expect(classes).toContain("bg-diff-selected");
      expect(classes).toContain("before:w-3");
      expect(classes).toContain("before:bg-primary");
      expect(classes).toContain("before:left-0");
      expect(classes).toContain("relative");
    }
  });
});

describe("diffNumberClass", () => {
  it("writes a selected row's numbers in semibold primary text and the others muted", () => {
    expect(diffNumberClass(true)).toBe("font-semibold text-primary-text");
    expect(diffNumberClass(false)).toBe("text-text-muted");
  });
});
