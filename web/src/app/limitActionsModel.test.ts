import { describe, expect, it } from "vitest";
import { switchItems, switchPendingLabel } from "./limitActionsModel";

describe("limitActionsModel", () => {
  it("leaves the control out without a target", () => {
    expect(switchItems([])).toBeNull();
  });

  it("lists every target by its name in one list, in the daemon's order, for one, two or more", () => {
    expect(switchItems(["work-b"])).toEqual([
      { account: "work-b", label: "work-b" },
    ]);
    expect(switchItems(["default", "work-b"])).toEqual([
      { account: "default", label: "default" },
      { account: "work-b", label: "work-b" },
    ]);
    expect(
      switchItems(["work-b", "work-c", "personal", "spare"])?.map(
        (item) => item.account,
      ),
    ).toEqual(["work-b", "work-c", "personal", "spare"]);
  });

  it("names the account a pending switch goes to", () => {
    expect(switchPendingLabel("default")).toBe("Switching to default…");
  });
});
