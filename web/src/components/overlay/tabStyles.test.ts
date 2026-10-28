import { describe, expect, it } from "vitest";
import { TAB_BY_STATE, TAB_LIST, tabClasses } from "./tabStyles";

describe("tabClasses", () => {
  it("draws a 2px ink underline inside the tab for the current tab", () => {
    const classes = tabClasses(true).split(" ");
    expect(classes).toContain("after:h-2");
    expect(classes).toContain("after:bottom-0");
    expect(classes).toContain("after:bg-ink");
    expect(classes).not.toContain("border-b-2");
  });

  it("keeps the same box and hides the underline otherwise", () => {
    const classes = tabClasses(false).split(" ");
    expect(classes).toContain("after:h-2");
    expect(classes).toContain("after:bg-transparent");
    expect(classes).toContain("text-text-2");
  });

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
