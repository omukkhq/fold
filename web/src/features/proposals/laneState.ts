import type {
  ConflictedReason,
  ProposalLane,
  ProposalSummary,
} from "@/api/proposals";
import { actorSegment, seg, type Segments } from "@/people/segments";

/** A proposal row with the lane fields of the list and the detail; a row without them reads as no lane and a merge conflict. */
export type WithLane = ProposalSummary & {
  conflicted_reason?: ConflictedReason | null;
  lane?: ProposalLane | null;
};

export type LaneFailure = Extract<
  ConflictedReason,
  { kind: "lane_checks_failed" }
>;

/** The reason when it is a lane check failure; null for a merge conflict and for no reason. */
export function laneFailure(
  reason: ConflictedReason | null | undefined,
): LaneFailure | null {
  return reason?.kind === "lane_checks_failed" ? reason : null;
}

/** A conflicted proposal whose flag is a merge conflict: a lane check failure is not one. */
export function isMergeConflict(p: WithLane): boolean {
  return p.conflicted && laneFailure(p.conflicted_reason) === null;
}

/** `Integration checks failed after lane migrations moved`. */
export function laneFailureText(failure: LaneFailure): string {
  return `Integration checks failed after lane ${failure.lane} moved`;
}

/** Who fixes a lane check failure: the reviewer the daemon names, else a reviewer the review lead starts. */
export function fixerSegments(failure: LaneFailure): Segments {
  return failure.fixer === null
    ? ["a reviewer fixes it"]
    : seg`${actorSegment(failure.fixer)} fixes it`;
}

/** The failure and its fixer in one line, such as `Integration checks failed after lane migrations moved · @reviewer fixes it`. */
export function laneFailureLine(failure: LaneFailure): Segments {
  return seg`${laneFailureText(failure)} · ${fixerSegments(failure)}`;
}

/** `Lane migrations, position 2`; just the lane while the proposal has no place before its first approval; null for a proposal outside every lane. */
export function laneText(lane: ProposalLane | null | undefined): string | null {
  if (lane === null || lane === undefined) return null;
  return lane.position === null
    ? `Lane ${lane.name}`
    : `Lane ${lane.name}, position ${String(lane.position)}`;
}
