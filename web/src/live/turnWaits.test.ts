import { describe, expect, it } from "vitest";
import { limitWords, waitWords } from "./turnWaits";

describe("limitWords", () => {
  it("names each limit as a sentence does", () => {
    expect(limitWords("session")).toBe("session limit");
    expect(limitWords("weekly")).toBe("weekly limit");
    expect(limitWords("opus")).toBe("Opus limit");
    expect(limitWords("sonnet")).toBe("Sonnet limit");
    expect(limitWords("usage")).toBe("usage limit");
  });

  it("reads an unknown or missing kind as the usage limit", () => {
    expect(limitWords("daily")).toBe("usage limit");
    expect(limitWords(null)).toBe("usage limit");
    expect(limitWords(undefined)).toBe("usage limit");
  });
});

describe("waitWords", () => {
  it("says what a waiting worker does in a chip's words", () => {
    expect(waitWords("network")).toBe("waiting for the network");
    expect(waitWords("usage_limit")).toBe("waits for the usage limit");
  });
});
