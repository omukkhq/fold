const MONTHS = [
  "Jan",
  "Feb",
  "Mar",
  "Apr",
  "May",
  "Jun",
  "Jul",
  "Aug",
  "Sep",
  "Oct",
  "Nov",
  "Dec",
];

const WEEKDAYS = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

/** The viewer's local midnight of the day that holds `at`. */
export function localDayStart(at: number): number {
  const day = new Date(at);
  day.setHours(0, 0, 0, 0);
  return day.getTime();
}

/** The local midnight `days` days after the day that starts at `dayStart`; a negative count goes back. A day with a clock change keeps its own length. */
export function addDays(dayStart: number, days: number): number {
  const day = new Date(dayStart);
  day.setDate(day.getDate() + days);
  day.setHours(0, 0, 0, 0);
  return day.getTime();
}

/** The start of the hour `hour` of the local day that starts at `dayStart`. */
export function hourOfDay(dayStart: number, hour: number): number {
  const at = new Date(dayStart);
  at.setHours(hour, 0, 0, 0);
  return at.getTime();
}

/** Where the viewer's today and last seven days start, in UTC milliseconds, as the daemon's cost totals take them. */
export function costDays(now: number): { today: number; week: number } {
  const today = localDayStart(now);
  return { today, week: addDays(today, -6) };
}

/** `Oct 21` in the viewer's time zone. */
export function dayText(at: number): string {
  const day = new Date(at);
  return `${MONTHS[day.getMonth()] ?? ""} ${String(day.getDate())}`;
}

/** `Mon` in the viewer's time zone. */
export function weekdayText(at: number): string {
  return WEEKDAYS[new Date(at).getDay()] ?? "";
}

/** `09:00` in the viewer's time zone. */
export function hourText(at: number): string {
  return `${String(new Date(at).getHours()).padStart(2, "0")}:00`;
}
