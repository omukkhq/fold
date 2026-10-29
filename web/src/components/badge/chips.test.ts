import { describe, expect, it } from "vitest";
import { COUNT_CLASSES, FILTER_COUNT_CLASSES, chipClasses } from "./chips";

describe("chipClasses", () => {
  it("draws every tone as a 13px pill without an outline", () => {
    for (const tone of ["neutral", "warn", "ok", "bad"] as const) {
      const classes = chipClasses(tone);
      expect(classes).toContain("rounded-pill");
      expect(classes).toContain("text-13");
      expect(classes).not.toContain("inset-ring");
    }
  });

  it("puts a neutral chip on the fill tone and keeps gold for warn", () => {
    expect(chipClasses("neutral")).toContain("bg-fill text-text");
    expect(chipClasses("warn")).toContain("bg-warn-soft text-warn");
    expect(chipClasses("ok")).toContain("bg-ok-soft text-ok");
    expect(chipClasses("bad")).toContain("bg-bad-soft text-bad");
  });
});

describe("COUNT_CLASSES", () => {
  it("counts what waits for a person, in gold", () => {
    expect(COUNT_CLASSES).toContain("text-12");
    expect(COUNT_CLASSES).toContain("text-warn");
    expect(COUNT_CLASSES).toContain("tabular-nums");
  });
});

describe("FILTER_COUNT_CLASSES", () => {
  it("draws the count badge's shape in the accent tone, never gold", () => {
    expect(FILTER_COUNT_CLASSES).toContain("text-12");
    expect(FILTER_COUNT_CLASSES).toContain("text-accent");
    expect(FILTER_COUNT_CLASSES).not.toContain("warn");
    expect(FILTER_COUNT_CLASSES.replace("bg-accent-soft text-accent", "")).toBe(
      COUNT_CLASSES.replace("bg-warn-soft text-warn", ""),
    );
  });
});
