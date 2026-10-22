import { describe, expect, it } from "vitest";
import {
  COMPOSER_MAX_ROWS,
  composerHeight,
  composerKey,
  lineHeightOf,
  parsePx,
  type ComposerKeyInput,
} from "./composer";

const enter: ComposerKeyInput = {
  key: "Enter",
  shiftKey: false,
  isComposing: false,
  pending: false,
  text: "Use the short versions.",
};

describe("composerKey", () => {
  it("sends on Enter", () => {
    expect(composerKey(enter)).toBe("send");
  });

  it("leaves Shift+Enter to the textarea for a new line", () => {
    expect(composerKey({ ...enter, shiftKey: true })).toBe("pass");
  });

  it("never sends while an input method composes", () => {
    expect(composerKey({ ...enter, isComposing: true })).toBe("pass");
  });

  it("swallows Enter on empty or blank text", () => {
    expect(composerKey({ ...enter, text: "" })).toBe("swallow");
    expect(composerKey({ ...enter, text: "  \n\t " })).toBe("swallow");
  });

  it("swallows Enter while a send is pending", () => {
    expect(composerKey({ ...enter, pending: true })).toBe("swallow");
  });

  it("passes every other key", () => {
    expect(composerKey({ ...enter, key: "a" })).toBe("pass");
    expect(composerKey({ ...enter, key: "Tab", text: "" })).toBe("pass");
  });
});

describe("composerHeight", () => {
  const metrics = { lineHeight: 20, paddingY: 18, borderY: 0 };

  it("is one line high for an empty box", () => {
    expect(composerHeight({ ...metrics, scrollHeight: 0 })).toEqual({
      height: 38,
      scrolls: false,
    });
  });

  it("grows with the text", () => {
    expect(composerHeight({ ...metrics, scrollHeight: 78 })).toEqual({
      height: 78,
      scrolls: false,
    });
  });

  it("stops at six lines and scrolls for the rest", () => {
    const six = 20 * COMPOSER_MAX_ROWS + 18;
    expect(composerHeight({ ...metrics, scrollHeight: six })).toEqual({
      height: six,
      scrolls: false,
    });
    expect(composerHeight({ ...metrics, scrollHeight: six + 40 })).toEqual({
      height: six,
      scrolls: true,
    });
  });

  it("takes another row limit and adds the border", () => {
    expect(
      composerHeight({
        ...metrics,
        borderY: 2,
        maxRows: 2,
        scrollHeight: 200,
      }),
    ).toEqual({ height: 60, scrolls: true });
  });
});

describe("parsePx", () => {
  it("reads a pixel length", () => {
    expect(parsePx("19.25px", 0)).toBe(19.25);
  });

  it("falls back for anything else", () => {
    expect(parsePx("normal", 7)).toBe(7);
    expect(parsePx("", 7)).toBe(7);
    expect(parsePx("1.5", 7)).toBe(7);
  });
});

describe("lineHeightOf", () => {
  it("takes a pixel line height", () => {
    expect(lineHeightOf("22px", "16px")).toBe(22);
  });

  it("derives a normal line height from the font size", () => {
    expect(lineHeightOf("normal", "15px")).toBeCloseTo(18);
    expect(lineHeightOf("", "")).toBeCloseTo(19.2);
  });
});
