import { describe, expect, it } from "vitest";
import { summarize } from "./summary";

const file = (added: number, removed: number, binary = false) => ({
  path: "a",
  previous_path: null,
  change: "modified" as const,
  added_lines: added,
  removed_lines: removed,
  binary,
});

describe("summarize", () => {
  it("sums the lines of the files", () => {
    expect(summarize([file(40, 3), file(2, 1), file(0, 3)])).toBe(
      "3 files, +42 -7",
    );
  });

  it("uses the singular for one file", () => {
    expect(summarize([file(1, 0)])).toBe("1 file, +1 -0");
  });

  it("counts a binary file as a file with no lines", () => {
    expect(summarize([file(5, 1), file(0, 0, true)])).toBe("2 files, +5 -1");
  });

  it("reports no files", () => {
    expect(summarize([])).toBe("0 files, +0 -0");
  });
});
