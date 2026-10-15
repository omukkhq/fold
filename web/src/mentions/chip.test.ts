import { describe, expect, it } from "vitest";
import { chipLabel } from "./chip";

describe("chipLabel", () => {
  it("drops the mark of an item or a file", () => {
    expect(chipLabel("#T-9")).toBe("T-9");
    expect(chipLabel("@web/src/report.tsx")).toBe("web/src/report.tsx");
  });

  it("keeps text without a mark", () => {
    expect(chipLabel("T-9")).toBe("T-9");
  });
});
