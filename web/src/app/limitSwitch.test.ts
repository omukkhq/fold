import { describe, expect, it } from "vitest";
import {
  SWITCH_BUTTONS_UP_TO,
  switchLabel,
  switchLayout,
  switchPendingLabel,
} from "./limitSwitch";

describe("limitSwitch", () => {
  it("names the account the button switches to", () => {
    expect(switchLabel("work-b")).toBe("Switch to work-b");
    expect(switchPendingLabel("work-b")).toBe("Switching to work-b…");
  });

  it("shows up to two targets as buttons and more as one menu", () => {
    expect(SWITCH_BUTTONS_UP_TO).toBe(2);
    expect(switchLayout(0)).toBe("none");
    expect(switchLayout(1)).toBe("buttons");
    expect(switchLayout(2)).toBe("buttons");
    expect(switchLayout(3)).toBe("menu");
    expect(switchLayout(4)).toBe("menu");
  });
});
