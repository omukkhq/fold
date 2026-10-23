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
  it("keeps Needs you, the bell, the name and the theme switch in the phone row", () => {
    expect(TOP_BAR_ITEMS.filter((item) => !inMore(item))).toEqual([
      "needsYou",
      "bell",
      "name",
      "theme",
    ]);
  });

  it("puts the live state, the pause control and the agents behind More, in row order", () => {
    expect(moreItems()).toEqual(["stream", "pause", "agents"]);
  });

  it("hides only the More items below md, and the More button from md", () => {
    expect(rowItemClass("stream")).toBe("max-md:hidden");
    expect(rowItemClass("agents")).toBe("max-md:hidden");
    expect(rowItemClass("bell")).toBe("");
    expect(MORE_TRIGGER_CLASS).toBe("md:hidden");
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
    expect(alert).toEqual({ label: "Reconnecting", dot: "bg-secondary" });
    expect(moreLabel(alert)).toBe("More, Reconnecting");
  });

  it("tells paused agents", () => {
    const alert = moreAlert("live", true);
    expect(alert).toEqual({ label: "Agents paused", dot: "bg-warning" });
    expect(moreLabel(alert)).toBe("More, Agents paused");
  });
});
