import { describe, expect, it } from "vitest";
import { findIds, parseId } from "./ids";

describe("findIds", () => {
  it("finds all nine id kinds with exact spans", () => {
    const text = "T-12 P-3 C-40 r17 ws-9 ck-88 m-210 RQ-4 n-5";
    const found = findIds(text);
    expect(found.map((id) => [id.kind, id.number, id.text])).toEqual([
      ["task", 12, "T-12"],
      ["proposal", 3, "P-3"],
      ["changeset", 40, "C-40"],
      ["revision", 17, "r17"],
      ["workspace", 9, "ws-9"],
      ["checkpoint", 88, "ck-88"],
      ["message", 210, "m-210"],
      ["request", 4, "RQ-4"],
      ["note", 5, "n-5"],
    ]);
    for (const id of found) {
      expect(text.slice(id.start, id.end)).toBe(id.text);
    }
  });

  it("ignores a prefix with no number", () => {
    expect(findIds("T- and P-x and r and ws-")).toEqual([]);
  });

  it("ignores an id inside a longer word", () => {
    expect(findIds("xT-12 T-12x a-T-12 T-12-a T-12_1 rc-3 ar17")).toEqual([]);
  });

  it("ignores numbers with a leading zero, zero and numbers past 32 bits", () => {
    expect(findIds("T-012 T-0 r00 r01 T-4294967296 T-99999999999")).toEqual([]);
    expect(findIds("T-4294967295")).toHaveLength(1);
  });

  it("finds the genesis revision r0", () => {
    const found = findIds("from r0 to r1");
    expect(found.map((id) => [id.kind, id.number])).toEqual([
      ["revision", 0],
      ["revision", 1],
    ]);
  });

  it("finds ids next to punctuation", () => {
    const found = findIds("(T-1), see P-2. Then [r3]:");
    expect(found.map((id) => id.text)).toEqual(["T-1", "P-2", "r3"]);
    expect(found[1]?.start).toBe(11);
  });

  it("finds the same id twice", () => {
    expect(findIds("T-1 T-1")).toHaveLength(2);
  });

  it("returns nothing for text without ids", () => {
    expect(findIds("")).toEqual([]);
  });
});

describe("parseId", () => {
  it("parses a whole id", () => {
    expect(parseId("ws-9")).toMatchObject({ kind: "workspace", number: 9 });
  });

  it("rejects an id with anything around it", () => {
    expect(parseId(" T-12")).toBeNull();
    expect(parseId("T-12 ")).toBeNull();
    expect(parseId("T-12 P-3")).toBeNull();
    expect(parseId("xT-12")).toBeNull();
    expect(parseId("T-")).toBeNull();
    expect(parseId("")).toBeNull();
  });
});
