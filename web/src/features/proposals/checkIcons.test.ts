import { describe, expect, it } from "vitest";
import { OUTCOME_ICONS } from "./checkIcons";

describe("OUTCOME_ICONS", () => {
  it("fills an open step green only when it passed", () => {
    expect(OUTCOME_ICONS.passed.selected).toContain("bg-ok-soft");
    expect(OUTCOME_ICONS.failed.selected).toContain("bg-bad-soft");
    expect(OUTCOME_ICONS.timed_out.selected).toContain("bg-bad-soft");
    for (const outcome of ["skipped", "not_run"] as const) {
      expect(OUTCOME_ICONS[outcome].selected).not.toContain("ok");
      expect(OUTCOME_ICONS[outcome].selected).toContain("bg-accent-soft");
    }
  });
});
