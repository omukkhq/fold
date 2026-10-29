import { describe, expect, it } from "vitest";
import {
  NARROW_ONLY,
  SWITCHER_FIT,
  WIDE_ONLY,
  menuAlert,
  menuLabel,
  switcherTooltip,
} from "./topBarLayout";
import themeCss from "../styles/index.css?raw";

describe("the top bar layout", () => {
  it("swaps the full row and the menu at the wide breakpoint of 1440px", () => {
    expect(WIDE_ONLY).toBe("max-wide:hidden");
    expect(NARROW_ONLY).toBe("wide:hidden");
    expect(themeCss).toMatch(/--breakpoint-wide:\s*90rem;/);
  });
});

describe("the project switcher's tooltip", () => {
  it("names only a slug the switcher cuts", () => {
    expect(switcherTooltip("a".repeat(SWITCHER_FIT))).toBeNull();
    expect(switcherTooltip("nobs-demo")).toBeNull();
    expect(switcherTooltip("frontend-rewrite-demo")).toBe(
      "frontend-rewrite-demo",
    );
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
