import { describe, expect, it } from "vitest";
import { namesComparison, parseDiffSearch } from "./search";

describe("parseDiffSearch", () => {
  it("passes from, to and path on unparsed", () => {
    expect(
      parseDiffSearch({ from: "r7", to: "ws-7", path: "src/a.ts" }),
    ).toEqual({ from: "r7", to: "ws-7", path: "src/a.ts" });
  });

  it("trims and leaves out an empty path", () => {
    expect(parseDiffSearch({ from: " r7 ", to: "r8", path: " " })).toEqual({
      from: "r7",
      to: "r8",
    });
  });

  it("takes to alone and leaves out a missing or blank from", () => {
    expect(parseDiffSearch({ to: "C-40" })).toEqual({ to: "C-40" });
    expect(parseDiffSearch({ from: " ", to: "ws-7" })).toEqual({ to: "ws-7" });
  });

  it("reads a missing or non-text value as empty", () => {
    expect(parseDiffSearch({ from: 7 })).toEqual({ to: "" });
  });
});

describe("namesComparison", () => {
  it("needs to and takes from as optional", () => {
    expect(namesComparison({ from: "r7", to: "r8" })).toBe(true);
    expect(namesComparison({ to: "r8" })).toBe(true);
    expect(namesComparison({ from: "r7", to: "" })).toBe(false);
    expect(namesComparison(parseDiffSearch({ from: "r7" }))).toBe(false);
  });
});
