import { describe, expect, it } from "vitest";
import { MAX_TEXT_CHARS, charCount } from "./text";

describe("charCount", () => {
  it("counts a surrogate pair as one character, as the daemon does", () => {
    expect(charCount("ab")).toBe(2);
    expect(charCount("😀x")).toBe(2);
    expect("😀".length).toBe(2);
  });

  it("counts a lone surrogate as one", () => {
    expect(charCount("\uD83D")).toBe(1);
  });
});

describe("MAX_TEXT_CHARS", () => {
  it("is the daemon's limit for a body", () => {
    expect(MAX_TEXT_CHARS).toBe(20_000);
  });
});
