import type { AttentionItem } from "../../api/workspaces";
import type { LiveWorkspace } from "../../live/envelope";
import { revisionsAhead } from "../../format/revisions";
import { formatQuiet } from "../../format/time";
import { workerActivity } from "../../live/workerActivity";

export interface AttentionView {
  /** `running checks` while a check run goes in the workspace. */
  checks: string | null;
  /** What fold saw of the worker, `Running a command for 8 min`, in the neutral pill: work in progress or a stall that fold told an agent of; null while no check run goes and fold has nothing to say, or when the stall waits for a person. */
  activity: string | null;
  /** The stall that waits for a person, `No activity from @web-worker for 12 min`, or `no edits for 7 min` when only the daemon's item tells of it; null while no check run goes. */
  stalled: string | null;
  staleClaim: boolean;
}

/**
 * Shows the daemon's `stalled` and `stale_claim` items and ignores the kinds this screen does not draw; the web decides nothing.
 * `lastActivityAt` is the workspace's last edit, which is what a stalled workspace has been quiet since. `live` is the
 * workspace's live state, which is newer than the list's items: while it names a check run, the worker waits for its
 * result and the stall does not show. What fold saw of the worker comes from the live state too.
 */
export function attentionView(
  items: readonly AttentionItem[],
  lastActivityAt: number,
  now: number,
  live:
    | (Pick<LiveWorkspace, "check_run"> &
        Partial<Pick<LiveWorkspace, "activity" | "stalled" | "worker">>)
    | null = null,
): AttentionView {
  const checking = (live?.check_run ?? null) !== null;
  const seen =
    checking || live?.activity == null
      ? null
      : workerActivity(
          {
            activity: live.activity,
            stalled: live.stalled ?? false,
            worker: live.worker ?? "",
          },
          now,
        );
  const flagged = !checking && items.some((item) => item.kind === "stalled");
  return {
    checks: checking ? "running checks" : null,
    activity: seen !== null && !seen.needsYou ? seen.text : null,
    stalled:
      seen?.needsYou === true
        ? seen.text
        : seen === null && flagged
          ? formatQuiet(now, lastActivityAt)
          : null,
    staleClaim: items.some((item) => item.kind === "stale_claim"),
  };
}

interface MainState {
  head: string | null;
  recent: readonly { id: string; parent: string | null }[];
}

/** `Main is at r9`, said once for every workspace; null until main has been read or while it has no revision. */
export function mainHeadText(repository: MainState | undefined): string | null {
  const head = repository?.head ?? null;
  return head === null ? null : `Main is at ${head}`;
}

/** `2 behind` beside a base that main has moved past, `3+ behind` when the base is older than the revisions main lists; null for a base that is main, and until main has been read. */
export function behindTag(
  base: string,
  repository: MainState | undefined,
): string | null {
  const head = repository?.head ?? null;
  if (repository === undefined || head === null || head === base) {
    return null;
  }
  const ahead = revisionsAhead(repository.recent, base);
  return `${String(ahead.count)}${ahead.atLeast ? "+" : ""} behind`;
}
