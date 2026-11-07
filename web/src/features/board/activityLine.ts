import type { BoardCard } from "@/api/board";
import { formatQuiet, formatSince } from "@/format/time";
import type { LiveWorkspace } from "@/live/envelope";
import { workerActivity } from "@/live/workerActivity";
import { checkRunText, currentCheckRun } from "./checkRun";
import { sentText, sentVersion, showsStall, waitText } from "./sentVersion";

export interface ActivityLine {
  /** `Running checks for v2` while a check run goes in the workspace; what fold saw of the worker, such as `Running a command for 8 min` or `No activity from @web-worker for 12 min`; `Editing src/a.ts`; `Sent P-1 v1 · waits for review` while the workspace files are the open proposal's newest version; or `No edits for 6 min` when the worker is stalled and no sent version waits on others, with `· blocked` when the task also waits on a prerequisite. */
  text: string;
  /** `12 s ago`: since the run started while one goes, else since the last edit; null when the text already says how long, as what fold saw of the worker does. */
  since: string | null;
  /** The worker's stall waits for a person, which the line marks in gold. */
  stalled: boolean;
  /** The check run that goes in the workspace, `cr-4`; null while none goes. */
  run: string | null;
  /** Changed files in the live workspace; null when the stream has not reported it. */
  files: number | null;
}

/**
 * The card's live line. A board payload that carries `activity` names the file and the time of the last edit; a live frame
 * of the same workspace that arrived later moves the time forward, and names the file when exactly one changed. What fold saw of the worker comes from the
 * live frame, and a stall is gold only while it waits for a person; before the first frame `stalled` comes from the daemon's `attention` list, and never shows
 * while the holder's sent version waits on others. A check run that goes leads: the worker waits for its result, so it is neither editing nor stalled.
 */
export function activityLine(
  card: Pick<
    BoardCard,
    | "activity"
    | "attention"
    | "blocked"
    | "check_run"
    | "proposal"
    | "edits_after_version"
  >,
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
  const sent = sentVersion(
    card.proposal,
    card.edits_after_version,
    live ?? null,
    card.attention,
  );
  const run = currentCheckRun(card.check_run, live);
  const seen = run === null ? workerActivity(live, now) : null;
  const stalled =
    seen === null
      ? run === null &&
        live?.activity == null &&
        showsStall(sent, card.attention)
      : seen.needsYou;
  const [only, ...rest] = live?.files ?? [];
  const path =
    activity?.path ??
    (only !== undefined && rest.length === 0 ? only.path : null);
  let text: string;
  if (run !== null) {
    text = checkRunText(run, card.proposal);
  } else if (seen !== null) {
    text = seen.text;
    if (seen.stall && card.blocked) {
      text += " · blocked";
    }
  } else if (stalled) {
    const quiet = formatQuiet(now, at);
    text = quiet.charAt(0).toUpperCase() + quiet.slice(1);
    if (card.blocked) {
      text += " · blocked";
    }
  } else if (sent !== null) {
    const verb = sentText(sent);
    text = `${verb.charAt(0).toUpperCase()}${verb.slice(1)} · ${waitText(sent.waits)}`;
  } else {
    text = path === null ? "Editing" : `Editing ${path}`;
  }
  return {
    text,
    since: seen === null ? formatSince(now, run?.started_at ?? at) : null,
    stalled,
    run: run?.run ?? null,
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
