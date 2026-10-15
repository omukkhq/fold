import { describe, expect, it } from "vitest";
import { EVENT_LIMIT, withEvent } from "./eventList";

describe("withEvent", () => {
  it("puts a newer event first", () => {
    expect(withEvent([{ seq: 2 }, { seq: 1 }], { seq: 3 })).toEqual([
      { seq: 3 },
      { seq: 2 },
      { seq: 1 },
    ]);
  });

  it("places a late event by its seq", () => {
    expect(withEvent([{ seq: 3 }, { seq: 1 }], { seq: 2 })).toEqual([
      { seq: 3 },
      { seq: 2 },
      { seq: 1 },
    ]);
  });

  it("adds no event twice", () => {
    expect(withEvent([{ seq: 2 }], { seq: 2 })).toEqual([{ seq: 2 }]);
  });

  it("drops the oldest beyond the limit", () => {
    const list = Array.from({ length: EVENT_LIMIT }, (_, i) => ({
      seq: EVENT_LIMIT - i,
    }));
    const next = withEvent(list, { seq: EVENT_LIMIT + 1 });
    expect(next).toHaveLength(EVENT_LIMIT);
    expect(next[0]?.seq).toBe(EVENT_LIMIT + 1);
    expect(next.at(-1)?.seq).toBe(2);
  });
});
