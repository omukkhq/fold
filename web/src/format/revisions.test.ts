import { describe, expect, it } from "vitest";
import { revisionCount, revisionsAhead } from "./revisions";

const recent = [
  { id: "r9", parent: "r8" },
  { id: "r8", parent: "r7" },
  { id: "r7", parent: null },
];

describe("revisionsAhead", () => {
  it("is 0 when the base is the head", () => {
    expect(revisionsAhead(recent, "r9")).toEqual({ count: 0, atLeast: false });
  });

  it("counts the parent steps from the head to the base", () => {
    expect(revisionsAhead(recent, "r7")).toEqual({ count: 2, atLeast: false });
  });

  it("is a lower bound when the base is older than the list", () => {
    expect(revisionsAhead(recent, "r3")).toEqual({ count: 3, atLeast: true });
    expect(revisionsAhead([], "r3")).toEqual({ count: 0, atLeast: true });
  });
});

describe("revisionCount", () => {
  it("words one, many and a lower bound", () => {
    expect(revisionCount({ count: 1, atLeast: false })).toBe("1 revision");
    expect(revisionCount({ count: 3, atLeast: false })).toBe("3 revisions");
    expect(revisionCount({ count: 10, atLeast: true })).toBe(
      "at least 10 revisions",
    );
  });
});
