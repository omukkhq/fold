import { formatAsOf } from "@/format/time";

const SECOND = 1000;

export interface ReconnectingParts {
  paused: string;
  /** Null without `retryAt`. It changes each second, so the banner keeps it out of its live region. */
  countdown: string | null;
  asOf: string;
}

/**
 * The reconnecting banner's sentence in its parts; `retryAt` adds the countdown W02 draws. Times are UTC milliseconds.
 * `shown` names what is on screen: W02 says "the board".
 */
export function reconnectingParts(
  now: number,
  asOf: number,
  retryAt?: number,
  shown = "the state",
): ReconnectingParts {
  let countdown: string | null = null;
  if (retryAt !== undefined) {
    const seconds = Math.ceil((retryAt - now) / SECOND);
    countdown =
      seconds > 0
        ? `Reconnecting in ${String(seconds)} s.`
        : "Reconnecting now.";
  }
  return {
    paused: "Live updates paused.",
    countdown,
    asOf: `Showing ${shown} as of ${formatAsOf(now, asOf)}; missed events replay when the connection returns.`,
  };
}

export function reconnectingText(
  now: number,
  asOf: number,
  retryAt?: number,
  shown = "the state",
): string {
  const parts = reconnectingParts(now, asOf, retryAt, shown);
  return [parts.paused, parts.countdown, parts.asOf]
    .filter((part) => part !== null)
    .join(" ");
}
