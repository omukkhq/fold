import { describe, expect, it } from "vitest";
import { createQueryClient, shouldRetry } from "./queryClient";

describe("createQueryClient", () => {
  it("never polls and never refetches on focus", () => {
    const q = createQueryClient().getDefaultOptions().queries;
    expect(q?.refetchOnWindowFocus).toBe(false);
    expect(q?.refetchInterval).toBe(false);
  });
});

describe("shouldRetry", () => {
  it("retries a network error once", () => {
    expect(shouldRetry(0, new TypeError("Failed to fetch"))).toBe(true);
    expect(shouldRetry(1, new TypeError("Failed to fetch"))).toBe(false);
  });
  it("never retries other errors", () => {
    expect(shouldRetry(0, new Error("boom"))).toBe(false);
  });
});
