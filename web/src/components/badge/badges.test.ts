import { describe, expect, it } from "vitest";
import { BADGE_TONES, badgeSpec, badgeText } from "./badges";

describe("badgeSpec", () => {
  it.each(BADGE_TONES)("gives %s an icon, text and token classes", (tone) => {
    const spec = badgeSpec(tone);
    expect(spec.text.length).toBeGreaterThan(0);
    expect(spec.icon).toBeDefined();
    expect(spec.classes).toMatch(/^bg-[a-z0-9-]+ text-[a-z0-9-]+$/);
    expect(spec.shape).toMatch(/^px-\d+ py-\d+ text-\d+ leading-tight$/);
  });

  it("draws the blocked mark 3 by 8 at 11px", () => {
    expect(badgeSpec("blocked").shape).toBe("px-8 py-3 text-13 leading-tight");
  });

  it("draws a requested change in the danger tone, as the proposal list does", () => {
    expect(badgeSpec("proposal-changes").classes).toBe("bg-bad-soft text-bad");
  });

  it("shows every tone with its own text", () => {
    const texts = BADGE_TONES.map((tone) => badgeSpec(tone).text);
    expect(new Set(texts).size).toBe(BADGE_TONES.length);
  });
});

describe("badgeText", () => {
  it("uses the tone text by default", () => {
    expect(badgeText("blocked")).toBe("Blocked");
  });

  it("uses the caller's text when given", () => {
    expect(badgeText("blocked", "Blocked by T-9")).toBe("Blocked by T-9");
  });

  it("falls back to the tone text for blank text", () => {
    expect(badgeText("stale", "   ")).toBe("Stale");
  });
});
