import type { NewReview, ProposalDetail } from "@/api/proposals";
import { reviewerActor } from "@/people/names";
import { reviewerSegment, seg, segmentsText } from "@/people/segments";
import { canSendForReviewAgain } from "./edit";
import { gateText, shownGates } from "./gates";
import { approvers } from "./present";
import type { DetailView } from "./view";

export type HumanAction =
  | "approve"
  | "request_changes"
  | "integrate"
  | "review_again";

/** The body an Approve sends: the daemon takes no verdict without a body or a comment, and the button has no text field. */
export const APPROVE_BODY = "approved in the web interface";

/**
 * The version a human's Approve, Request changes or Integrate acts on: the version on screen, never the current version the cache holds at the click.
 * A version that arrived after the human read the shown one makes the daemon refuse with `stale_version`, so nobody approves or integrates a version they did not read.
 */
export function actedVersion(view: Pick<DetailView, "version">): number {
  return view.version;
}

/** The verdict a human sends from the side panel, on the version on screen. */
export function humanVerdict(
  view: Pick<DetailView, "version">,
  verdict: "approve" | "request_changes",
  body: string,
): NewReview {
  return { version: actedVersion(view), verdict, body };
}

/**
 * The actions the signed-in human sees on the current version of an open proposal. The daemon decides whether each write holds and refuses with a code the screen shows on the action;
 * Integrate shows also after a `request_changes`, because an integrating human approves in the same action. Send for review again shows on a human edit whose kept approvals still count.
 */
export function visibleActions(
  proposal: ProposalDetail,
  human: string | null,
): HumanAction[] {
  if (human === null || proposal.summary.state !== "open") return [];
  const actions: HumanAction[] = ["approve", "request_changes", "integrate"];
  return canSendForReviewAgain(proposal, human)
    ? [...actions, "review_again"]
    : actions;
}

/** The line under a disabled Approve and Integrate once the integration checks of the current version failed: the daemon refuses that version until its author sends a new one. */
export const CHECKS_FAILED =
  "The integration checks failed; the author sends a new version.";

/** Whether the line under Approve shows. After failed integration checks Approve and Integrate share one line, which shows once under both buttons. */
export function showsApproveLine(
  approveBlock: string | null,
  integrateBlock: string | null,
  integrateShown: boolean,
): boolean {
  if (approveBlock === null) return false;
  return !(
    integrateShown &&
    approveBlock === CHECKS_FAILED &&
    integrateBlock === CHECKS_FAILED
  );
}

/** The line under a disabled Approve on a task with the human review mark: the human reviews after a reviewer agent. */
export const AGENT_APPROVES_FIRST = "A reviewer agent approves first.";

/** The line under a disabled Approve once the signed-in human's latest verdict on the current version is an approval. */
export function ownApprovalLine(version: number): string {
  return `You approved v${String(version)}.`;
}

/** The line under a disabled Approve while a check run of the current version runs. */
export function checkRunsLine(version: number): string {
  return `A check runs on v${String(version)}. Approve when it ends.`;
}

/** The line under a disabled Integrate while an integration check run of the current version runs. */
export function integrationRunsLine(version: number): string {
  return `fold runs the integration checks of v${String(version)}. Integrate when they end.`;
}

/** The check runs of the current version, or none when the proposal read selected an earlier version. */
function currentRuns(proposal: ProposalDetail) {
  return proposal.selected.version === proposal.summary.current_version
    ? proposal.checks
    : [];
}

function agentApprovesFirst(proposal: ProposalDetail): boolean {
  return (
    proposal.summary.human_review !== null &&
    !proposal.agent_approved &&
    !proposal.main_task
  );
}

/**
 * Why Approve cannot work for the signed-in human; null when it can.
 * An approval the human already holds on the current version would add nothing, and an approval given while a check run of that version runs, of either stage, would come before its result; a new version clears both.
 * On a task with the human review mark the daemon takes a human's approval only after an agent with review rights approved the current version, which it reports as `agent_approved`. A task of the main agent's own area (`main_task`) has no reviewer agent, so the human approves it first.
 */
export function approveBlock(
  proposal: ProposalDetail,
  human: string | null,
): string | null {
  const version = proposal.summary.current_version;
  if (proposal.summary.checks_failed) return CHECKS_FAILED;
  const actor = reviewerActor(human);
  if (actor !== null && approvers(version, proposal.reviews).includes(actor)) {
    return ownApprovalLine(version);
  }
  if (currentRuns(proposal).some((run) => run.state === "running")) {
    return checkRunsLine(version);
  }
  return agentApprovesFirst(proposal) ? AGENT_APPROVES_FIRST : null;
}

/**
 * Why Integrate cannot work for the signed-in human, from the first unmet gate in the daemon's report; null when no gate stands in the way.
 * For a human only the human review mark and the prerequisites can be unmet: the caller and approval gates hold, because the integrating human approves in the same action. For the same reason a human integrates a marked task only after a reviewer agent approved it, as for Approve.
 * An integration run of the current version that fold started by itself or for a review batch has no queued request, so the daemon would queue the human's request behind it, to be refused once that integration ends or to run the checks a second time.
 */
export function integrateBlock(
  proposal: ProposalDetail,
  main: string | null,
): string | null {
  if (proposal.summary.checks_failed) return CHECKS_FAILED;
  if (agentApprovesFirst(proposal)) return AGENT_APPROVES_FIRST;
  if (
    currentRuns(proposal).some(
      (run) => run.stage === "integration" && run.state === "running",
    )
  ) {
    return integrationRunsLine(proposal.summary.current_version);
  }
  const gate = shownGates(proposal.gates).find((g) => g.status === "unmet");
  if (!gate) return null;
  if (gate.gate === "human_review") {
    const who = reviewerSegment(
      proposal.summary.human_review?.reviewer ?? null,
      "a human",
    );
    return segmentsText(
      seg`Integrate waits for ${who} to approve v${String(proposal.summary.current_version)}.`,
    );
  }
  return `${segmentsText(gateText(gate, proposal, main))}, so you cannot integrate yet.`;
}
