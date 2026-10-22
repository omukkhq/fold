import { describe, expect, it } from "vitest";
import { clampClass, isCut, showsToggle, toggleLabel } from "./clamp";

describe("clampClass", () => {
  it("cuts a collapsed text to its lines", () => {
    expect(clampClass(2, false)).toBe("line-clamp-2");
    expect(clampClass(3, false)).toBe("line-clamp-3");
  });

  it("shows every line of an open text", () => {
    expect(clampClass(3, true)).toBe("");
  });
});

describe("isCut", () => {
  it("is cut when the text is taller than its box", () => {
    expect(isCut({ scrollHeight: 90, clientHeight: 54 })).toBe(true);
  });

  it("is not cut when the text fits, or differs by a rounding pixel", () => {
    expect(isCut({ scrollHeight: 54, clientHeight: 54 })).toBe(false);
    expect(isCut({ scrollHeight: 55, clientHeight: 54 })).toBe(false);
  });
});

describe("the toggle", () => {
  it("shows only while the text is cut or open", () => {
    expect(showsToggle(false, false)).toBe(false);
    expect(showsToggle(false, true)).toBe(true);
    expect(showsToggle(true, false)).toBe(true);
  });

  it("says More to open and Less to close", () => {
    expect(toggleLabel(false)).toBe("More");
    expect(toggleLabel(true)).toBe("Less");
  });
});
