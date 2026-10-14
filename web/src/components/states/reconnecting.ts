import { formatAsOf } from "@/format/time";

const SECOND = 1000;

/** The reconnecting banner's sentence; `retryAt` adds the countdown W02 draws. Times are UTC milliseconds. */
export function reconnectingText(
  now: number,
  asOf: number,
  retryAt?: number,
): string {
  const parts = ["Live updates paused."];
  if (retryAt !== undefined) {
    const seconds = Math.ceil((retryAt - now) / SECOND);
    parts.push(
      seconds > 0
        ? `Reconnecting in ${String(seconds)} s.`
        : "Reconnecting now.",
    );
  }
  parts.push(
    `Showing the state as of ${formatAsOf(now, asOf)}; missed events replay when the connection returns.`,
  );
  return parts.join(" ");
}
