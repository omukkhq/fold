import { formatAsOfMinute } from "@/format/time";
import { actorSegment, seg, type Segments } from "@/people/segments";

export type PauseAction = "pause" | "resume";

const IDLE_LABEL: Record<PauseAction, string> = {
  pause: "Pause agents",
  resume: "Resume agents",
};

const PENDING_LABEL: Record<PauseAction, string> = {
  pause: "Pausing…",
  resume: "Resuming…",
};

/** Every pause and resume write of one project shares this key, so a control mounted again, such as in a reopened More popover, finds the write in flight and its error. */
export function pauseWrites(slug: string) {
  return ["pause-write", slug] as const;
}

function isPauseAction(value: unknown): value is PauseAction {
  return value === "pause" || value === "resume";
}

/** The newest pause or resume write of the project, as the mutation cache keeps it: the action in flight, else the error of a write that failed. */
export function lastPauseWrite(
  writes: readonly { status: string; variables: unknown; error: unknown }[],
): { sent: PauseAction | null; error: unknown } {
  const last = writes.at(-1);
  if (last === undefined) return { sent: null, error: null };
  return {
    sent:
      last.status === "pending" && isPauseAction(last.variables)
        ? last.variables
        : null,
    error: last.status === "error" ? last.error : null,
  };
}

export interface PauseControlView {
  /** What a press asks the daemon for. */
  action: PauseAction;
  label: string;
  /** What the control says while the action is in flight. */
  pendingLabel: string;
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
      label: IDLE_LABEL[sent],
      pendingLabel: PENDING_LABEL[sent],
      paused,
      pending: true,
    };
  }
  return paused
    ? {
        action: "resume",
        label: IDLE_LABEL.resume,
        pendingLabel: PENDING_LABEL.resume,
        paused,
        pending: false,
      }
    : {
        action: "pause",
        label: IDLE_LABEL.pause,
        pendingLabel: PENDING_LABEL.pause,
        paused,
        pending: false,
      };
}

export interface PausedBannerText {
  /** The sentence with the person who paused as an actor, so the screen draws a coloured `@name`. */
  title: Segments;
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
  const since = formatAsOfMinute(now, pausedSince);
  return {
    title:
      pausedBy === null
        ? seg`Agents paused since ${since}`
        : seg`Agents paused since ${since} by ${actorSegment(pausedBy)}`,
    detail:
      "Every worker is stopped and keeps its session. Claims keep their leases. Approved proposals still integrate. Resume starts the workers again.",
  };
}
