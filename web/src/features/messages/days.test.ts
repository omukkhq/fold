import { describe, expect, it } from "vitest";
import { message } from "@/test/messages";
import { dayLabel, rowTime, withDays } from "./days";

const local = (y: number, m: number, d: number, h = 12, min = 0) =>
  new Date(y, m, d, h, min).getTime();
const NOW = local(2026, 9, 22, 15);

describe("dayLabel", () => {
  it("names today and yesterday by word, even across midnight", () => {
    expect(dayLabel(NOW, local(2026, 9, 22, 0, 1))).toBe("Today");
    expect(dayLabel(NOW, local(2026, 9, 21, 23, 59))).toBe("Yesterday");
  });
  it("names an older day by its date, with the year only before this year", () => {
    expect(dayLabel(NOW, local(2026, 9, 20))).toBe("Oct 20");
    expect(dayLabel(NOW, local(2025, 11, 31))).toBe("Dec 31, 2025");
  });
  it("finds yesterday across the first of a month", () => {
    expect(dayLabel(local(2026, 10, 1, 9), local(2026, 9, 31, 22))).toBe(
      "Yesterday",
    );
  });
});

describe("rowTime", () => {
  it("shows the clock today and the day before today", () => {
    expect(rowTime(NOW, local(2026, 9, 22, 9, 5))).toBe("09:05");
    expect(rowTime(NOW, local(2026, 9, 21, 9, 5))).toBe("Yesterday");
    expect(rowTime(NOW, local(2026, 9, 2))).toBe("Oct 2");
  });
});

describe("withDays", () => {
  it("puts a divider before the first message of each local day", () => {
    const items = withDays(
      [
        message("m-1", { sent_at: local(2026, 9, 21, 22) }),
        message("m-2", { sent_at: local(2026, 9, 21, 23) }),
        message("m-3", { sent_at: local(2026, 9, 22, 8) }),
      ],
      NOW,
    );
    expect(
      items.map((item) => (item.kind === "day" ? item.label : item.key)),
    ).toEqual(["Yesterday", "m-1", "m-2", "Today", "m-3"]);
    expect(new Set(items.map((item) => item.key)).size).toBe(items.length);
  });
  it("has no item for a thread without messages", () => {
    expect(withDays([], NOW)).toEqual([]);
  });
});
