import { describe, expect, it } from "vitest";
import { listNote, listState } from "./listNote";

const WORDS = { loading: "Loading", failed: "Could not load.", empty: "None." };

describe("listState", () => {
  it("waits, fails or is ready", () => {
    expect(listState({ isPending: true, isError: false })).toBe("pending");
    expect(listState({ isPending: false, isError: true })).toBe("error");
    expect(listState({ isPending: false, isError: false })).toBe("ready");
  });
});

describe("listNote", () => {
  it("says the list loads, failed or is empty", () => {
    expect(listNote("pending", 0, WORDS)).toEqual({
      text: "Loading",
      failed: false,
    });
    expect(listNote("error", 0, WORDS)).toEqual({
      text: "Could not load.",
      failed: true,
    });
    expect(listNote("ready", 0, WORDS)).toEqual({
      text: "None.",
      failed: false,
    });
  });

  it("gives way to the rows, also those of an earlier answer when a later read fails", () => {
    expect(listNote("ready", 2, WORDS)).toBeNull();
    expect(listNote("error", 2, WORDS)).toBeNull();
  });
});
