import { describe, expect, it } from "vitest";
import { MAX_DELAY_MS, nextDelay } from "./backoff";

describe("nextDelay", () => {
  it("doubles from one second", () => {
    expect([0, 1, 2, 3].map((n) => nextDelay(n, () => 0.999999))).toEqual([
      1000, 2000, 4000, 8000,
    ]);
  });

  it("never passes the ceiling, however many attempts", () => {
    expect(nextDelay(5, () => 0.999999)).toBe(MAX_DELAY_MS);
    expect(nextDelay(1000, () => 0.999999)).toBe(MAX_DELAY_MS);
  });

  it("takes jitter from the random argument, between half and all", () => {
    expect(nextDelay(2, () => 0)).toBe(2000);
    expect(nextDelay(2, () => 0.5)).toBe(3000);
  });

  it("treats a negative attempt as the first", () => {
    expect(nextDelay(-3, () => 0)).toBe(500);
  });
});
