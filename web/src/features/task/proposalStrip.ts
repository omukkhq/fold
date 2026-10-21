import type { TaskDetail } from "@/api/tasks";
import { hasKind } from "@/features/board/attention";

export interface ProposalStrip {
  proposal: string;
  /** `P-7 v2 waits for your review`, or `P-7 v2 waits for a reviewer`. */
  lead: string;
  /** True while the version waits for this human; the strip is gold then and neutral otherwise. */
  waitsForYou: boolean;
}

/**
 * The strip under the status row that leads to the proposal screen. It shows while the task has a proposal that is
 * not integrated. The task read names no reviewer that holds the review claim, so the neutral wording does not either.
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
  return {
    proposal: proposal.id,
    lead: `${version} ${waitsForYou ? "waits for your review" : "waits for a reviewer"}`,
    waitsForYou,
  };
}
