import { describe, expect, it } from "vitest";
import { cardKindLabel, cardNeeds } from "./cardTasks";

describe("cardKindLabel", () => {
  it("names the kinds the board names and leaves the default unmarked", () => {
    expect(cardKindLabel({})).toBeNull();
    expect(cardKindLabel({ kind: "proposal" })).toBeNull();
    expect(cardKindLabel({ kind: "no_proposal" })).toBe("no proposal");
    expect(cardKindLabel({ kind: "human" })).toBe("human task");
  });
});

describe("cardNeeds", () => {
  it("links each needed task in order", () => {
    expect(cardNeeds({ needs: ["T-12", "T-14"] }, "demo")).toEqual([
      { id: "T-12", href: "/p/demo/t/T-12" },
      { id: "T-14", href: "/p/demo/t/T-14" },
    ]);
  });

  it("is empty for a card with none or with the field left out", () => {
    expect(cardNeeds({}, "demo")).toEqual([]);
    expect(cardNeeds({ needs: [] }, "demo")).toEqual([]);
  });
});
