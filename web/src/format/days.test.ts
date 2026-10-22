import { describe, expect, it } from "vitest";
import {
  addDays,
  costDays,
  dayText,
  hourOfDay,
  hourText,
  localDayStart,
  weekdayText,
} from "./days";

const NOW = new Date(2026, 9, 21, 15, 30).getTime();

describe("local days", () => {
  it("starts the day at local midnight and steps whole days", () => {
    const today = localDayStart(NOW);
    expect(today).toBe(new Date(2026, 9, 21).getTime());
    expect(addDays(today, -6)).toBe(new Date(2026, 9, 15).getTime());
    expect(addDays(today, 11)).toBe(new Date(2026, 10, 1).getTime());
    expect(hourOfDay(today, 9)).toBe(new Date(2026, 9, 21, 9).getTime());
    expect(costDays(NOW)).toEqual({
      today,
      week: new Date(2026, 9, 15).getTime(),
    });
  });

  it("names days, weekdays and hours", () => {
    expect(dayText(NOW)).toBe("Oct 21");
    expect(weekdayText(NOW)).toBe("Wed");
    expect(hourText(NOW)).toBe("15:00");
    expect(hourText(new Date(2026, 0, 1, 7).getTime())).toBe("07:00");
  });
});
