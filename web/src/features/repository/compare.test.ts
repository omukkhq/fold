import { describe, expect, it } from "vitest";
import {
  compareLabel,
  comparePath,
  defaultCompareFrom,
  normalizeCompare,
} from "./compare";

describe("normalizeCompare", () => {
  it("keeps an ordered pair", () => {
    expect(normalizeCompare("r3", "r8")).toEqual({
      ok: true,
      pair: { from: "r3", to: "r8" },
    });
  });

  it("puts the older revision first", () => {
    expect(normalizeCompare("r8", "r3")).toEqual({
      ok: true,
      pair: { from: "r3", to: "r8" },
    });
  });

  it("orders by number, not text", () => {
    expect(normalizeCompare("r10", "r9")).toEqual({
      ok: true,
      pair: { from: "r9", to: "r10" },
    });
  });

  it("rejects the same revision twice", () => {
    expect(normalizeCompare("r4", "r4")).toEqual({ ok: false, reason: "same" });
  });

  it("rejects anything that is not a revision id", () => {
    for (const bad of ["T-4", "r0", "r04", "main", "", "r4 "]) {
      expect(normalizeCompare(bad, "r8")).toEqual({
        ok: false,
        reason: "invalid",
      });
      expect(normalizeCompare("r8", bad)).toEqual({
        ok: false,
        reason: "invalid",
      });
    }
  });
});

describe("comparePath", () => {
  it("opens the diff route with from and to", () => {
    expect(comparePath("demo", { from: "r3", to: "r8" })).toBe(
      "/p/demo/diff?from=r3&to=r8",
    );
  });
});

describe("defaultCompareFrom", () => {
  const revisions = [{ id: "r5" }, { id: "r4" }, { id: "r3" }];

  it("pairs a revision with the one before it", () => {
    expect(defaultCompareFrom(revisions, "r5")).toBe("r4");
    expect(defaultCompareFrom(revisions, "r4")).toBe("r3");
  });

  it("pairs the oldest revision with the one after it", () => {
    expect(defaultCompareFrom(revisions, "r3")).toBe("r4");
  });

  it("has nothing to pair with a single or an unknown revision", () => {
    expect(defaultCompareFrom([{ id: "r1" }], "r1")).toBeNull();
    expect(defaultCompareFrom(revisions, "r9")).toBeNull();
  });
});

describe("compareLabel", () => {
  it("names the pair older first", () => {
    expect(compareLabel("r8", "r7")).toBe("Compare r7 → r8");
  });

  it("names no pair when the choice is not valid", () => {
    expect(compareLabel("r4", "r4")).toBe("Compare");
  });
});
