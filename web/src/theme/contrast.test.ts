import { describe, expect, it } from "vitest";
import { composite, contrastRatio, parseHex } from "./contrast";

describe("contrast", () => {
  it("rates black on white at 21:1", () => {
    expect(contrastRatio(parseHex("#000000"), parseHex("#FFFFFF"))).toBeCloseTo(
      21,
      5,
    );
  });
  it("parses alpha and composites over a base", () => {
    expect(parseHex("#ff000080")[3]).toBeCloseTo(0.5, 2);
    const c = composite(parseHex("#FFFFFF80"), parseHex("#000000"));
    expect(c[0]).toBeCloseTo(128, 0);
    expect(c[3]).toBe(1);
  });
  it("rejects a non-hex colour", () => {
    expect(() => parseHex("red")).toThrow();
  });
});
