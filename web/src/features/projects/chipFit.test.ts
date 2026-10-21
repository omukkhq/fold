import { describe, expect, it } from "vitest";
import { fittingChips, moreLabel, moreTitle } from "./chipFit";

describe("fittingChips", () => {
  it("shows every chip when the whole line fits", () => {
    expect(fittingChips([50, 60, 70], 50 + 60 + 70 + 2 * 6, 6, 30)).toBe(3);
    expect(fittingChips([], 0, 6, 30)).toBe(0);
  });

  it("keeps room for the +N chip when some chips do not fit", () => {
    // 50 + 6 + 60 + 6 + 30 = 152 fits; adding the 70 chip does not.
    expect(fittingChips([50, 60, 70], 160, 6, 30)).toBe(2);
    expect(fittingChips([50, 60, 70], 152, 6, 30)).toBe(2);
    expect(fittingChips([50, 60, 70], 151, 6, 30)).toBe(1);
  });

  it("shows only the +N chip when not even the first chip fits beside it", () => {
    expect(fittingChips([120, 60], 100, 6, 30)).toBe(0);
    expect(fittingChips([120], 100, 6, 30)).toBe(0);
  });
});

describe("the +N chip", () => {
  it("counts and names the hidden agents", () => {
    expect(moreLabel(3)).toBe("+3");
    expect(moreTitle(["backend-lead"])).toBe("1 more agent: @backend-lead");
    expect(moreTitle(["a", "b"])).toBe("2 more agents: @a, @b");
  });
});
