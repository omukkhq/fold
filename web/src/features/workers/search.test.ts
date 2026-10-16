import { describe, expect, it } from "vitest";
import { isBatchId, parseAgentsSearch } from "./search";

describe("parseAgentsSearch", () => {
  it("keeps a well-formed question id only", () => {
    expect(parseAgentsSearch({ question: "pq-3" })).toEqual({
      question: "pq-3",
    });
    expect(parseAgentsSearch({ question: "pq-03" })).toEqual({});
    expect(parseAgentsSearch({ question: 3 })).toEqual({});
    expect(parseAgentsSearch({})).toEqual({});
  });
});

describe("isBatchId", () => {
  it("accepts rb ids", () => {
    expect(isBatchId("rb-3")).toBe(true);
    expect(isBatchId("rb-0")).toBe(false);
    expect(isBatchId("P-3")).toBe(false);
  });
});
