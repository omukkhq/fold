import type { MessageView } from "@/api/messages";
import { formatClock } from "@/format/time";

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

function startOfDay(at: number): number {
  const when = new Date(at);
  return new Date(
    when.getFullYear(),
    when.getMonth(),
    when.getDate(),
  ).getTime();
}

/** The local day of `at` as a divider names it: `Today`, `Yesterday`, `Oct 20` in the year of `now`, `Oct 20, 2025` before it. */
export function dayLabel(now: number, at: number): string {
  const day = startOfDay(at);
  const today = startOfDay(now);
  if (day === today) return "Today";
  const yesterday = new Date(today);
  yesterday.setDate(yesterday.getDate() - 1);
  if (day === yesterday.getTime()) return "Yesterday";
  const when = new Date(at);
  const date = `${MONTHS[when.getMonth()] ?? ""} ${String(when.getDate())}`;
  return when.getFullYear() === new Date(now).getFullYear()
    ? date
    : `${date}, ${String(when.getFullYear())}`;
}

/** A thread row's time: the clock for a message of today, else the day as a divider names it. */
export function rowTime(now: number, at: number): string {
  return startOfDay(at) === startOfDay(now)
    ? formatClock(at)
    : dayLabel(now, at);
}

/** A message's time where no day divider stands above it: the clock for today, else the day as a divider names it and the clock, `Yesterday 14:02`. */
export function dayClock(now: number, at: number): string {
  return startOfDay(at) === startOfDay(now)
    ? formatClock(at)
    : `${dayLabel(now, at)} ${formatClock(at)}`;
}

export type ThreadItem =
  | { kind: "day"; key: string; label: string }
  | { kind: "message"; key: string; message: MessageView };

/** The messages in order with a day divider before the first message of each local day. */
export function withDays(
  messages: readonly MessageView[],
  now: number,
): ThreadItem[] {
  const items: ThreadItem[] = [];
  let last: number | null = null;
  for (const message of messages) {
    const day = startOfDay(message.sent_at);
    if (day !== last) {
      items.push({
        kind: "day",
        key: `day-${String(day)}`,
        label: dayLabel(now, message.sent_at),
      });
      last = day;
    }
    items.push({ kind: "message", key: message.id, message });
  }
  return items;
}
