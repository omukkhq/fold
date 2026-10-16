import { formatAsOfMinute } from "@/format/time";
import { atName } from "@/people/names";

export type PauseAction = "pause" | "resume";

export interface PauseControlView {
  /** What a press asks the daemon for. */
  action: PauseAction;
  label: string;
  /** The paused look: the control offers the resume. */
  paused: boolean;
  pending: boolean;
}

/**
 * The top bar's pause control. `pausedSince` is the daemon's state, `undefined` while the project is not read yet; `sent` is the action in flight, which keeps the control pending until the daemon's state shows it done.
 */
export function pauseControl(
  pausedSince: number | null | undefined,
  sent: PauseAction | null,
): PauseControlView | null {
  if (pausedSince === undefined) {
    return null;
  }
  const paused = pausedSince !== null;
  if (sent !== null && (sent === "pause") !== paused) {
    return {
      action: sent,
      label: sent === "pause" ? "Pausing…" : "Resuming…",
      paused,
      pending: true,
    };
  }
  return paused
    ? { action: "resume", label: "Resume agents", paused, pending: false }
    : { action: "pause", label: "Pause agents", paused, pending: false };
}

export interface PausedBannerText {
  title: string;
  detail: string;
}

/** The paused banner's sentences, or null while the agents run. Times are UTC milliseconds. */
export function pausedBanner(
  now: number,
  pausedSince: number | null,
  pausedBy: string | null,
): PausedBannerText | null {
  if (pausedSince === null) {
    return null;
  }
  const by = pausedBy === null ? "" : ` by ${atName(pausedBy)}`;
  return {
    title: `Agents paused since ${formatAsOfMinute(now, pausedSince)}${by}`,
    detail:
      "Every worker is stopped and keeps its session. Claims keep their leases. Approved proposals still integrate. Resume starts the workers again.",
  };
}
