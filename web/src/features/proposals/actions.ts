import type { ProposalDetail } from "@/api/proposals";
import { gateText, shownGates } from "./gates";

export type HumanAction = "approve" | "request_changes" | "integrate";

/** The body an Approve sends: the daemon takes no verdict without a body or a comment, and the button has no text field. */
export const APPROVE_BODY = "approved in the web interface";

/**
 * The actions the signed-in human sees on the current version of an open proposal. The daemon decides whether each write holds and refuses with a code the screen shows on the action;
 * Integrate shows also after a `request_changes`, because an integrating human approves in the same action.
 */
export function visibleActions(
  proposal: ProposalDetail,
  human: string | null,
): HumanAction[] {
  if (human === null || proposal.summary.state !== "open") return [];
  return ["approve", "request_changes", "integrate"];
}

/** The composer opens only on the current version of an open proposal, for a named human. */
export function canComment(
  proposal: ProposalDetail,
  human: string | null,
  shownVersion: number | null,
): boolean {
  return (
    human !== null &&
    proposal.summary.state === "open" &&
    shownVersion === proposal.summary.current_version
  );
}

/**
 * Why Integrate cannot work for the signed-in human, from the first unmet gate in the daemon's report; null when no gate stands in the way.
 * For a human only the human review mark and the prerequisites can be unmet: the caller and approval gates hold, because the integrating human approves in the same action.
 */
export function integrateBlock(
  proposal: ProposalDetail,
  main: string | null,
): string | null {
  const gate = shownGates(proposal.gates).find((g) => g.status === "unmet");
  if (!gate) return null;
  if (gate.gate === "human_review") {
    const who = proposal.summary.human_review?.reviewer ?? "a human";
    return `Integrate waits for ${who} to approve v${String(proposal.summary.current_version)}.`;
  }
  return `${gateText(gate, proposal, main)}, so you cannot integrate yet.`;
}
