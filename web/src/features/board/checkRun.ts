import type { BoardProposal, CheckStage, LiveCheckRun } from "@/api/board";
import type { LiveWorkspace } from "@/live/envelope";

/** A running check run. A card's run names its stage; a run of a live workspace frame or a task has none. */
export type CheckRunView = LiveCheckRun & { stage?: CheckStage };

export const INTEGRATION_CHECKS_LABEL = "Integration checks running";
export const PROPOSAL_CHECKS_LABEL = "Checks running";

/** The stage of a run: its own when it names one, else the one its purpose implies. */
export function checkStage(run: CheckRunView): CheckStage {
  return (
    run.stage ?? (run.purpose === "integrate" ? "integration" : "proposal")
  );
}

/**
 * The check run that goes for a task. A live frame of the holder's workspace is newer than the payload, so its answer
 * wins, also when it says that no run goes; but an integration run has no workspace, so the frame never speaks for it
 * and the payload's integration run stays.
 */
export function currentCheckRun(
  payload: CheckRunView | null,
  live: Pick<LiveWorkspace, "check_run"> | null | undefined,
): CheckRunView | null {
  if (payload !== null && checkStage(payload) === "integration") {
    return payload;
  }
  return live ? live.check_run : payload;
}

/** The label of the running state on a board card: `Integration checks running` or `Checks running`. */
export function checkRunLabel(run: CheckRunView): string {
  return checkStage(run) === "integration"
    ? INTEGRATION_CHECKS_LABEL
    : PROPOSAL_CHECKS_LABEL;
}

/**
 * `Running checks for v1` before a new proposal, `Running checks for v3` before the next version of the open proposal
 * whose newest version is v2, `Running checks` for a worker's own run or while the version is not known, and
 * `Integration checks running` for an integration run, which makes no version.
 */
export function checkRunText(
  run: CheckRunView,
  proposal: Pick<BoardProposal, "id" | "version"> | null,
): string {
  if (checkStage(run) === "integration") {
    return INTEGRATION_CHECKS_LABEL;
  }
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
