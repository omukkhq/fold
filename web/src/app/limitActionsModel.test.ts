import { describe, expect, it } from "vitest";
import {
  chosenTarget,
  switchButtonLabel,
  switchItems,
  switchPendingLabel,
  triggerLabel,
} from "./limitActionsModel";

describe("limitActionsModel", () => {
  it("leaves the control out without a target", () => {
    expect(switchItems([])).toBeNull();
  });

  it("lists every target in one list, one entry each in the daemon's order, for one, two or more", () => {
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

  it("keeps a choice only while it is a target", () => {
    expect(chosenTarget(null, ["work-b"])).toBeNull();
    expect(chosenTarget("work-b", ["work-b", "personal"])).toBe("work-b");
    expect(chosenTarget("gone", ["work-b"])).toBeNull();
  });

  it("names the trigger and the switch button before and after a choice, and a pending switch", () => {
    expect(triggerLabel(null)).toBe("Switch account");
    expect(triggerLabel("work-b")).toBe("work-b");
    expect(triggerLabel("system-default")).toBe(
      "system-default (Claude Code's own login)",
    );
    expect(switchButtonLabel(null)).toBe("Choose an account first");
    expect(switchButtonLabel("work-b")).toBe("Switch to work-b");
    expect(switchPendingLabel("work-b")).toBe("Switching to work-b…");
  });
});
