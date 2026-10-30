import type { CheckRun, ProposalDetail, Review } from "@/api/proposals";
import {
  actorSegment,
  joinSegments,
  seg,
  type Segment,
  type Segments,
} from "@/people/segments";
import { approvals, keptNote, versionVerdict } from "./present";

export type StateTone = "approved" | "changes" | "neutral";

/** The people whose approvals stand, each kept approval followed by the version it was kept from. */
export function approverSegments(standing: readonly Review[]): Segment[] {
  return joinSegments(
    standing.map((r): Segments => {
      const note = keptNote(r);
      return note === ""
        ? [actorSegment(r.reviewer)]
        : [actorSegment(r.reviewer), note];
    }),
  );
}

/** The review half of the Changes tab's one-line state: who approved the version, each kept approval with the version it was kept from, else its latest verdict. */
export function approvalLine(
  p: ProposalDetail,
  version: number,
): { tone: StateTone; segments: Segment[] } {
  const v = `v${String(version)}`;
  const standing = approvals(version, p.reviews);
  if (standing.length > 0) {
    return {
      tone: "approved",
      segments: seg`Approved on ${v} by ${approverSegments(standing)}`,
    };
  }
  const verdict = versionVerdict(version, p.reviews);
  if (verdict === "no verdict yet") {
    return { tone: "neutral", segments: [`No verdict on ${v} yet`] };
  }
  const word = verdict.charAt(0).toUpperCase() + verdict.slice(1);
  return { tone: "changes", segments: [`${word} on ${v}`] };
}

const RUN_WORDS: Record<CheckRun["state"], string> = {
  passed: "checks passed",
  failed: "checks failed",
  running: "checks running",
  stopped: "checks stopped",
};

/** The checks half: what the version's newest run did; null until its runs arrive. */
export function checksWord(runs: readonly CheckRun[] | null): string | null {
  if (runs === null) return null;
  const newest = [...runs].sort((a, b) => b.created_at - a.created_at)[0];
  return newest ? RUN_WORDS[newest.state] : "no checks ran";
}

/** The count on a tab, such as `33` or `2 runs`; null while it is not known. */
export function runsCount(n: number | null): string | null {
  if (n === null) return null;
  return `${String(n)} ${n === 1 ? "run" : "runs"}`;
}
