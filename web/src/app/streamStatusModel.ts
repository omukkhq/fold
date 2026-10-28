import { formatSince } from "../format/time";
import type { LiveStatus } from "../live/status";

export interface StreamView {
  /** The connection state; only this part is announced, so the ticking time never interrupts a screen reader. */
  label: string;
  /** The text after the label and a middle dot, or null. */
  detail: string | null;
  /** Tailwind classes for the dot and the text. */
  dot: string;
  tone: string;
}

/** The time of the newest thing the stream delivered, or null before the first one. */
export function syncedAt(status: LiveStatus): number | null {
  const times = [status.lastEventAt, status.stateAsOf].filter(
    (t): t is number => t !== null,
  );
  return times.length === 0 ? null : Math.max(...times);
}

/** The connection state in words, which the top bar announces whether or not a panel that shows it is open. */
export function streamLabel(state: LiveStatus["state"]): string {
  switch (state) {
    case "connecting":
      return "Connecting…";
    case "reconnecting":
      return "Reconnecting…";
    case "live":
      return "Live";
  }
}

export function streamView(status: LiveStatus, now: number): StreamView {
  switch (status.state) {
    case "connecting":
      return {
        label: streamLabel("connecting"),
        detail: null,
        dot: "bg-text-2",
        tone: "text-text-2",
      };
    case "reconnecting":
      return {
        label: streamLabel("reconnecting"),
        detail: null,
        dot: "bg-warn",
        tone: "text-warn",
      };
    case "live": {
      const at = syncedAt(status);
      return {
        label: streamLabel("live"),
        detail: at === null ? "synced" : `synced ${formatSince(now, at)}`,
        dot: "bg-ok",
        tone: "text-text-2",
      };
    }
  }
}
