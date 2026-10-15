const SECOND = 1000;
const MINUTE = 60 * SECOND;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

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

/** The span as `12 s`, `7 min`, `3 h` or `2 d`; it rounds down and never goes below zero. */
function span(now: number, then: number): string {
  const elapsed = Math.max(0, now - then);
  if (elapsed < MINUTE) {
    return `${String(Math.floor(elapsed / SECOND))} s`;
  }
  if (elapsed < HOUR) {
    return `${String(Math.floor(elapsed / MINUTE))} min`;
  }
  if (elapsed < DAY) {
    return `${String(Math.floor(elapsed / HOUR))} h`;
  }
  return `${String(Math.floor(elapsed / DAY))} d`;
}

/** `2 min ago`. Times are UTC milliseconds and `now` is always passed in. */
export function formatSince(now: number, then: number): string {
  return `${span(now, then)} ago`;
}

/** `3 min`, for a list whose column already reads as an age, such as W03's checkpoints and changesets. */
export function formatAge(now: number, then: number): string {
  return span(now, then);
}

/** `no edits for 7 min`. */
export function formatQuiet(now: number, then: number): string {
  return `no edits for ${span(now, then)}`;
}

function pad(value: number): string {
  return String(value).padStart(2, "0");
}

function stamp(now: number, at: number, seconds: boolean): string {
  const when = new Date(at);
  const minutes = `${pad(when.getHours())}:${pad(when.getMinutes())}`;
  const clock = seconds ? `${minutes}:${pad(when.getSeconds())}` : minutes;
  if (when.toDateString() === new Date(now).toDateString()) {
    return clock;
  }
  const month = MONTHS[when.getMonth()] ?? "";
  return `${month} ${String(when.getDate())}, ${clock}`;
}

/** The local time of `at` as `14:12:31`, with the date in front when it is not the day of `now`. */
export function formatAsOf(now: number, at: number): string {
  return stamp(now, at, true);
}

/** The local time of `at` to the minute, `14:12`, with the date in front when it is not the day of `now`. */
export function formatAsOfMinute(now: number, at: number): string {
  return stamp(now, at, false);
}
