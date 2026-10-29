import type { BoardProposal, LiveCheckRun } from "@/api/board";
import type { LiveWorkspace } from "@/live/envelope";

/**
 * The check run that goes in the holder's workspace. A live frame of the workspace is newer than the payload, so its
 * answer wins, also when it says that no run goes; without one the payload decides.
 */
export function currentCheckRun(
  payload: LiveCheckRun | null,
  live: Pick<LiveWorkspace, "check_run"> | null | undefined,
): LiveCheckRun | null {
  return live ? live.check_run : payload;
}

/**
 * `Running checks for v1` before a new proposal, `Running checks for v3` before the next version of the open proposal
 * whose newest version is v2, and `Running checks` for a worker's own run or while the version is not known.
 */
export function checkRunText(
  run: LiveCheckRun,
  proposal: Pick<BoardProposal, "id" | "version"> | null,
): string {
  if (run.purpose === "create_proposal") {
    return "Running checks for v1";
  }
  const next =
    run.proposal !== null && proposal !== null && proposal.id === run.proposal
      ? proposal.version + 1
      : null;
  return next === null
    ? "Running checks"
    : `Running checks for v${String(next)}`;
}
