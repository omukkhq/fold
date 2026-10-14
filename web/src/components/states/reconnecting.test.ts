import { describe, expect, it } from "vitest";
import { reconnectingText } from "./reconnecting";

const now = new Date(2026, 9, 15, 14, 20, 0).getTime();
const asOf = new Date(2026, 9, 15, 14, 12, 31).getTime();

describe("reconnectingText", () => {
  it("says updates paused, gives the time and says missed events replay", () => {
    expect(reconnectingText(now, asOf)).toBe(
      "Live updates paused. Showing the state as of 14:12:31; missed events replay when the connection returns.",
    );
  });

  it("counts down to the next attempt in whole seconds, rounding up", () => {
    expect(reconnectingText(now, asOf, now + 3200)).toContain(
      "Reconnecting in 4 s.",
    );
  });

  it("says it is reconnecting now once the attempt is due", () => {
    expect(reconnectingText(now, asOf, now)).toContain("Reconnecting now.");
    expect(reconnectingText(now, asOf, now - 500)).toContain(
      "Reconnecting now.",
    );
  });
});
