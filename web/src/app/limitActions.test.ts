import { describe, expect, it } from "vitest";
import { switchLabel, switchPendingLabel } from "./limitActions";

describe("limitActions", () => {
  it("names the account the button switches to", () => {
    expect(switchLabel("work-b")).toBe("Switch to work-b");
    expect(switchPendingLabel("work-b")).toBe("Switching to work-b…");
  });
});
