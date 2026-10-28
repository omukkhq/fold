import { describe, expect, it } from "vitest";
import { TAB_BY_STATE, TAB_LIST } from "./tabStyles";

describe("tab styles", () => {
  it("marks the current Radix tab with the same underline", () => {
    expect(TAB_BY_STATE).toContain("data-[state=active]:after:bg-ink");
    expect(TAB_BY_STATE).toContain("after:h-2");
  });

  it("draws the row divider inside the row, under the tabs", () => {
    const classes = TAB_LIST.split(" ");
    expect(classes).toContain("before:h-px");
    expect(classes).toContain("before:bottom-0");
    expect(classes).not.toContain("border-b");
  });
});
