import { describe, expect, it } from "vitest";
import { orderPanels } from "./order";

describe("orderPanels", () => {
  it("sorts a first render by last activity, newest first", () => {
    expect(
      orderPanels(
        [],
        [
          { id: "ws-1", lastActivityAt: 10 },
          { id: "ws-2", lastActivityAt: 30 },
          { id: "ws-3", lastActivityAt: 20 },
        ],
      ),
    ).toEqual(["ws-2", "ws-3", "ws-1"]);
  });

  it("keeps a panel in place when its activity changes", () => {
    const shown = ["ws-2", "ws-3", "ws-1"];
    expect(
      orderPanels(shown, [
        { id: "ws-1", lastActivityAt: 99 },
        { id: "ws-2", lastActivityAt: 30 },
        { id: "ws-3", lastActivityAt: 20 },
      ]),
    ).toEqual(shown);
  });

  it("puts a new panel on top, newest first among new ones", () => {
    expect(
      orderPanels(
        ["ws-2", "ws-1"],
        [
          { id: "ws-1", lastActivityAt: 10 },
          { id: "ws-2", lastActivityAt: 30 },
          { id: "ws-3", lastActivityAt: 5 },
          { id: "ws-4", lastActivityAt: 6 },
        ],
      ),
    ).toEqual(["ws-4", "ws-3", "ws-2", "ws-1"]);
  });

  it("drops a panel that is gone and breaks a tie by id", () => {
    expect(
      orderPanels(
        ["ws-9", "ws-1"],
        [
          { id: "ws-1", lastActivityAt: 1 },
          { id: "ws-5", lastActivityAt: 7 },
          { id: "ws-4", lastActivityAt: 7 },
        ],
      ),
    ).toEqual(["ws-4", "ws-5", "ws-1"]);
  });
});
