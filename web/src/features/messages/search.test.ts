import { describe, expect, it } from "vitest";
import { messagesHref, normalizeSearch } from "./search";

describe("normalizeSearch", () => {
  it("keeps well-formed fields", () => {
    expect(
      normalizeSearch({
        agent: "frontend/ui",
        task: "T-12",
        proposal: "P-3",
        thread: "m-31",
      }),
    ).toEqual({
      agent: "frontend/ui",
      task: "T-12",
      proposal: "P-3",
      thread: "m-31",
    });
  });
  it("drops malformed, wrong-kind and non-string values", () => {
    expect(
      normalizeSearch({
        agent: "",
        task: "P-3",
        proposal: "nope",
        thread: 5,
      }),
    ).toEqual({});
    expect(normalizeSearch({ agent: "x".repeat(81) })).toEqual({});
  });
});

describe("messagesHref", () => {
  it("writes only the set fields", () => {
    expect(messagesHref("demo", {})).toBe("/p/demo/messages");
    expect(messagesHref("demo", { agent: "a/b", thread: "m-1" })).toBe(
      "/p/demo/messages?agent=a%2Fb&thread=m-1",
    );
  });
});
