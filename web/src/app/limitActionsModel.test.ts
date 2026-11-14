import { describe, expect, it } from "vitest";
import { switchItems, switchPendingLabel } from "./limitActionsModel";

describe("limitActionsModel", () => {
  it("leaves the menu out without a target", () => {
    expect(switchItems([])).toBeNull();
  });

  it("offers every target in one menu, one item each in the daemon's order, for one, two or more", () => {
    expect(switchItems(["work-b"])).toEqual([
      { account: "work-b", label: "work-b" },
    ]);
    expect(switchItems(["system-default", "work-b"])).toEqual([
      {
        account: "system-default",
        label: "system-default (Claude Code's own login)",
      },
      { account: "work-b", label: "work-b" },
    ]);
    expect(
      switchItems(["work-b", "work-c", "personal", "spare"])?.map(
        (item) => item.account,
      ),
    ).toEqual(["work-b", "work-c", "personal", "spare"]);
  });

  it("names the account a pending switch goes to", () => {
    expect(switchPendingLabel("work-b")).toBe("Switching to work-b…");
  });
});
