import { describe, expect, it } from "vitest";
import { atName } from "./names";
import { actorSegment, joinSegments, seg, segmentsText } from "./segments";

describe("seg", () => {
  it("merges text and keeps actors as data", () => {
    const parts = seg`Waiting for ${actorSegment("human:ana")} on v${2}.`;
    expect(parts).toEqual(["Waiting for ", { actor: "human:ana" }, " on v2."]);
  });

  it("splices a list of segments and drops empty text", () => {
    const who = [actorSegment("a"), " and ", actorSegment("b")];
    expect(seg`${who}`).toEqual([{ actor: "a" }, " and ", { actor: "b" }]);
  });
});

describe("joinSegments", () => {
  it("joins with commas and a last and", () => {
    const items = ["a", "b", "c"].map((n) => [actorSegment(n)]);
    expect(segmentsText(joinSegments(items))).toBe("@a, @b and @c");
    expect(joinSegments([])).toEqual([]);
  });
});

describe("segmentsText", () => {
  it("shows an actor as @name", () => {
    expect(segmentsText(seg`by ${actorSegment("human:tanim")}`)).toBe(
      "by @tanim",
    );
  });
});

describe("atName", () => {
  it("writes the person with an @", () => {
    expect(atName("human:tanim")).toBe("@tanim");
    expect(atName("frontend/ui")).toBe("@frontend/ui");
  });
});
