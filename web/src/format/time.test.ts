import { describe, expect, it } from "vitest";
import { formatAsOf, formatAsOfMinute, formatQuiet, formatSince } from "./time";

const NOW = 1_800_000_000_000;
const SEC = 1000;
const MIN = 60 * SEC;
const HOUR = 60 * MIN;
const DAY = 24 * HOUR;

describe("formatSince", () => {
  it.each([
    [0, "0 s ago"],
    [59 * SEC + 999, "59 s ago"],
    [MIN, "1 min ago"],
    [2 * MIN, "2 min ago"],
    [HOUR - 1, "59 min ago"],
    [HOUR, "1 h ago"],
    [DAY - 1, "23 h ago"],
    [DAY, "1 d ago"],
    [3 * DAY + HOUR, "3 d ago"],
  ])("renders %i ms as %s", (elapsed, text) => {
    expect(formatSince(NOW, NOW - elapsed)).toBe(text);
  });

  it("treats a time ahead of now as zero seconds", () => {
    expect(formatSince(NOW, NOW + 5 * SEC)).toBe("0 s ago");
  });
});

describe("formatQuiet", () => {
  it.each([
    [30 * SEC, "no edits for 30 s"],
    [7 * MIN, "no edits for 7 min"],
    [HOUR - 1, "no edits for 59 min"],
    [HOUR, "no edits for 1 h"],
    [DAY - 1, "no edits for 23 h"],
    [DAY, "no edits for 1 d"],
  ])("renders %i ms as %s", (elapsed, text) => {
    expect(formatQuiet(NOW, NOW - elapsed)).toBe(text);
  });
});

describe("formatAsOf", () => {
  it("shows only the clock on the same local day", () => {
    const now = new Date(2026, 9, 15, 14, 20, 0).getTime();
    const at = new Date(2026, 9, 15, 14, 12, 31).getTime();
    expect(formatAsOf(now, at)).toBe("14:12:31");
  });

  it("pads single digits", () => {
    const now = new Date(2026, 9, 15, 14, 20, 0).getTime();
    const at = new Date(2026, 9, 15, 9, 5, 7).getTime();
    expect(formatAsOf(now, at)).toBe("09:05:07");
  });

  it("adds the date on another day", () => {
    const now = new Date(2026, 9, 15, 0, 1, 0).getTime();
    const at = new Date(2026, 9, 14, 23, 59, 59).getTime();
    expect(formatAsOf(now, at)).toBe("Oct 14, 23:59:59");
  });
});

describe("formatAsOfMinute", () => {
  it("shows hours and minutes on the same local day", () => {
    const now = new Date(2026, 9, 15, 14, 20, 0).getTime();
    const at = new Date(2026, 9, 15, 9, 5, 31).getTime();
    expect(formatAsOfMinute(now, at)).toBe("09:05");
  });

  it("adds the date on another day", () => {
    const now = new Date(2026, 9, 15, 0, 1, 0).getTime();
    const at = new Date(2026, 9, 14, 23, 59, 59).getTime();
    expect(formatAsOfMinute(now, at)).toBe("Oct 14, 23:59");
  });
});
