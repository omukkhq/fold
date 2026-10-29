import { describe, expect, it } from "vitest";
import { chipFace, chipLabel } from "./chip";

describe("chipLabel", () => {
  it("drops the mark of an item or a file", () => {
    expect(chipLabel("#T-9")).toBe("T-9");
    expect(chipLabel("@web/src/report.tsx")).toBe("web/src/report.tsx");
  });

  it("keeps text without a mark", () => {
    expect(chipLabel("T-9")).toBe("T-9");
  });
});

describe("chipFace", () => {
  it("draws a path in the mono face and an item's id in the body face", () => {
    expect(chipFace("path")).toBe("font-mono text-13");
    for (const tag of ["task", "proposal", "request", "revision", "message"]) {
      expect(chipFace(tag)).toContain("font-body");
      expect(chipFace(tag)).toContain("tabular-nums");
      expect(chipFace(tag)).not.toContain("font-mono");
    }
  });
});
