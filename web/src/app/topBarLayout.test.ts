import { describe, expect, it } from "vitest";
import {
  MORE_TRIGGER_CLASS,
  TOP_BAR_ITEMS,
  inMore,
  moreAlert,
  moreItems,
  moreLabel,
  rowItemClass,
} from "./topBarLayout";

describe("the top bar layout", () => {
  it("keeps Needs you, the bell and the name in the compact row", () => {
    expect(TOP_BAR_ITEMS.filter((item) => !inMore(item))).toEqual([
      "needsYou",
      "bell",
      "name",
    ]);
  });

  it("puts the live state, the pause control, the agents and the theme switch behind More, in row order", () => {
    expect(moreItems()).toEqual(["stream", "pause", "agents", "theme"]);
  });

  it("hides only the More items below lg, and the More button from lg", () => {
    expect(rowItemClass("stream")).toBe("max-lg:hidden");
    expect(rowItemClass("agents")).toBe("max-lg:hidden");
    expect(rowItemClass("theme")).toBe("max-lg:hidden");
    expect(rowItemClass("bell")).toBe("");
    expect(MORE_TRIGGER_CLASS).toBe("lg:hidden");
  });
});

describe("the More button's alert", () => {
  it("says nothing while the stream is live or connecting and the agents run", () => {
    expect(moreAlert("live", false)).toBeNull();
    expect(moreAlert("connecting", false)).toBeNull();
    expect(moreLabel(null)).toBe("More");
  });

  it("tells a reconnecting stream before a pause", () => {
    const alert = moreAlert("reconnecting", true);
    expect(alert).toEqual({ label: "Reconnecting", dot: "bg-warn" });
    expect(moreLabel(alert)).toBe("More, Reconnecting");
  });

  it("tells paused agents", () => {
    const alert = moreAlert("live", true);
    expect(alert).toEqual({ label: "Agents paused", dot: "bg-warn" });
    expect(moreLabel(alert)).toBe("More, Agents paused");
  });
});
