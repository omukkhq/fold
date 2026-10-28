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

  it("draws every tone as the same 13px chip", () => {
    for (const tone of BADGE_TONES)
      expect(badgeSpec(tone).shape).toBe("px-9 py-2 text-13 leading-tight");
  });

  it("keeps gold for what waits for a person", () => {
    expect(badgeSpec("needs-you").classes).toBe("bg-warn-soft text-warn");
    expect(badgeSpec("stalled").classes).toBe("bg-warn-soft text-warn");
    expect(badgeSpec("stale").classes).toBe("bg-fill text-text");
  });

  it("shows every tone with its own text", () => {
    const texts = BADGE_TONES.map((tone) => badgeSpec(tone).text);
    expect(new Set(texts).size).toBe(BADGE_TONES.length);
  });
});

describe("badgeText", () => {
  it("uses the tone text by default", () => {
    expect(badgeText("stalled")).toBe("Stalled");
  });

  it("uses the caller's text when given", () => {
    expect(badgeText("stalled", "Stalled for 4m")).toBe("Stalled for 4m");
  });

  it("falls back to the tone text for blank text", () => {
    expect(badgeText("stale", "   ")).toBe("Stale");
  });
});
