import type { TaskDetail } from "@/api/tasks";
import { hasKind } from "@/features/board/attention";
import { reviewerText } from "./text";

export interface ProposalStrip {
  proposal: string;
  /** `P-7 v2 waits for your review`, `P-7 v2 is with @review-lead` or `P-7 v2 waits for a reviewer`. */
  lead: string;
  /** `· Add the form · 3 files`; empty while the read names neither. */
  rest: string;
  /** True while the version waits for this human; the strip is gold then and neutral otherwise. */
  waitsForYou: boolean;
}

function files(count: number): string {
  return `${String(count)} ${count === 1 ? "file" : "files"}`;
}

/**
 * The strip under the status row that leads to the proposal screen. It shows while the task has a proposal that is
 * not integrated. The lead says who waits: this human first, else the reviewer that holds the review claim, else any reviewer.
 */
export function proposalStrip(
  task: Pick<TaskDetail, "attention" | "proposal">,
): ProposalStrip | null {
  const { proposal } = task;
  if (proposal === null || proposal.state === "integrated") {
    return null;
  }
  const waitsForYou = hasKind(task.attention, "human_review");
  const version = `${proposal.id} v${String(proposal.version)}`;
  let wait = "waits for a reviewer";
  if (waitsForYou) {
    wait = "waits for your review";
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
    rest: parts.map((part) => `· ${part}`).join(" "),
    waitsForYou,
  };
}
