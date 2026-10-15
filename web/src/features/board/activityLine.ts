import type { BoardCard } from "@/api/board";
import { formatQuiet, formatSince } from "@/format/time";
import type { LiveWorkspace } from "@/live/envelope";
import { hasKind } from "./attention";

export interface ActivityLine {
  /** `Editing src/a.ts`, or `No edits for 6 min` when the worker is stalled, with `· blocked` when the task also waits on a prerequisite. */
  text: string;
  /** `12 s ago`. */
  since: string;
  stalled: boolean;
  /** Changed files in the live workspace; null when the stream has not reported it. */
  files: number | null;
}

/**
 * The card's live line. A board payload that carries `activity` names the file and the time of the last edit; a live frame
 * of the same workspace that arrived later moves the time forward, and names the file when exactly one changed. `stalled` comes only from the daemon's `attention` list.
 */
export function activityLine(
  card: Pick<BoardCard, "activity" | "attention" | "blocked">,
  live: LiveWorkspace | undefined,
  now: number,
): ActivityLine | null {
  const activity = card.activity ?? null;
  if (activity === null && live === undefined) {
    return null;
  }
  const at = Math.max(
    activity?.at ?? Number.NEGATIVE_INFINITY,
    live?.last_activity_at ?? Number.NEGATIVE_INFINITY,
  );
  const stalled = hasKind(card.attention, "stalled");
  const [only, ...rest] = live?.files ?? [];
  const path =
    activity?.path ??
    (only !== undefined && rest.length === 0 ? only.path : null);
  let text: string;
  if (stalled) {
    const quiet = formatQuiet(now, at);
    text = quiet.charAt(0).toUpperCase() + quiet.slice(1);
    if (card.blocked) {
      text += " · blocked";
    }
  } else {
    text = path === null ? "Editing" : `Editing ${path}`;
  }
  return {
    text,
    since: formatSince(now, at),
    stalled,
    files: live ? live.files.length : null,
  };
}

/** The footer pill, `4 files`, beside the activity line that carries the time; null while stalled or before the stream reports a change. */
export function liveFilesText(line: ActivityLine | null): string | null {
  if (
    line === null ||
    line.stalled ||
    line.files === null ||
    line.files === 0
  ) {
    return null;
  }
  const noun = line.files === 1 ? "file" : "files";
  return `${String(line.files)} ${noun}`;
}
