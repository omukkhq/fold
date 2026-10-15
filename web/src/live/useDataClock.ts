import { useState } from "react";

type Time = number | null | undefined;

/** Never earlier than the data it labels, so a fresh event never reads as from the future. */
export function dataTime(
  times: readonly Time[],
  current: number = Date.now(),
): number {
  return Math.max(current, ...times.map((t) => t ?? 0));
}

/**
 * The time a screen last received new data, for its relative times. It never ticks: the screens run no timer, so
 * "3 min ago" moves when a fetch or an event arrives. `renewOn` reads the time again without counting as data, such as
 * the next reconnect attempt, which lies in the future.
 */
export function useDataClock(
  times: readonly Time[],
  renewOn: readonly Time[] = [],
): number {
  const stamp = [...times, ...renewOn].map(String).join(":");
  const [clock, setClock] = useState(() => ({ stamp, now: dataTime(times) }));
  if (clock.stamp !== stamp) {
    const next = { stamp, now: dataTime(times) };
    setClock(next);
    return next.now;
  }
  return clock.now;
}
