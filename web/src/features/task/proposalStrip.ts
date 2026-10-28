import type { TaskDetail } from "@/api/tasks";
import { hasKind } from "@/features/board/attention";
import { reviewerText } from "./text";

export interface ProposalStrip {
  proposal: string;
  /** `P-7 v2 waits for your review`, `P-7 v2 is approved`, `P-7 v2 is with @review-lead` or `P-7 v2 waits for a reviewer`. */
  lead: string;
  /** `Add the form · 3 files`, the line under the lead; empty while the read names neither. */
  rest: string;
  /** True while the version waits for this human; the strip is gold then and neutral otherwise. */
  waitsForYou: boolean;
}

function files(count: number): string {
  return `${String(count)} ${count === 1 ? "file" : "files"}`;
}

/**
 * The strip under the status row that leads to the proposal screen. It shows only while the task is in Review, because
 * outside Review the next move on the proposal belongs to its author or the proposal is integrated. The lead says who
 * has the version: this human first, then an approval, then the reviewer that holds the review claim, else any reviewer.
 */
export function proposalStrip(
  task: Pick<TaskDetail, "attention" | "proposal" | "status">,
): ProposalStrip | null {
  const { proposal } = task;
  if (proposal === null || task.status !== "review") {
    return null;
  }
  const waitsForYou = hasKind(task.attention, "human_review");
  const version = `${proposal.id} v${String(proposal.version)}`;
  let wait = "waits for a reviewer";
  if (waitsForYou) {
    wait = "waits for your review";
  } else if (proposal.state === "approved") {
    wait = "is approved";
  } else if (proposal.review_claim !== null) {
    wait = `is with ${reviewerText(proposal.review_claim)}`;
  }
  const parts = [
    proposal.title === "" ? null : proposal.title,
    proposal.files > 0 ? files(proposal.files) : null,
  ].filter((part) => part !== null);
  return {
    proposal: proposal.id,
    lead: `${version} ${wait}`,
    rest: parts.join(" · "),
    waitsForYou,
  };
}
