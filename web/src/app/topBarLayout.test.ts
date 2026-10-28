import { describe, expect, it } from "vitest";
import { NARROW_ONLY, WIDE_ONLY, menuAlert, menuLabel } from "./topBarLayout";

describe("the top bar layout", () => {
  it("swaps the full row and the menu at xl", () => {
    expect(WIDE_ONLY).toBe("max-xl:hidden");
    expect(NARROW_ONLY).toBe("xl:hidden");
  });
});

describe("the menu button's alert", () => {
  it("says nothing while the stream is live or connecting and the agents run", () => {
    expect(menuAlert("live", false)).toBeNull();
    expect(menuAlert("connecting", false)).toBeNull();
    expect(menuLabel(null)).toBe("Menu");
  });

  it("tells a reconnecting stream before a pause", () => {
    const alert = menuAlert("reconnecting", true);
    expect(alert).toEqual({ label: "Reconnecting", dot: "bg-warn" });
    expect(menuLabel(alert)).toBe("Menu, Reconnecting");
  });

  it("tells paused agents", () => {
    const alert = menuAlert("live", true);
    expect(alert).toEqual({ label: "Agents paused", dot: "bg-warn" });
    expect(menuLabel(alert)).toBe("Menu, Agents paused");
  });
});
