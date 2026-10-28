import type { BoardCard, BoardProposal, CardProposalState } from "@/api/board";
import { planWait, planWaitLabel } from "@/features/proposals/planWait";
import { projectPath } from "@/routes/paths";

/**
 * The task dialog's path; the dialog route renders the board beneath itself. `search` is the board's filters as a
 * query string, which the dialog route takes as its own, so the board beneath it and after it keep them.
 */
export function taskPath(slug: string, task: string, search = ""): string {
  return `${projectPath(slug, "t", task)}${search}`;
}

/** The task dialog's create form, over the board. */
export function newTaskPath(slug: string): string {
  return projectPath(slug, "t", "new");
}

/** Plain steps and linked items counted together, `2/5`; null for a task with neither. */
export function checklistText(
  card: Pick<BoardCard, "checklist">,
): string | null {
  const c = card.checklist;
  const total = c.steps_total + c.linked_total;
  if (total === 0) {
    return null;
  }
  return `${String(c.steps_done + c.linked_done)}/${String(total)}`;
}

/** `needs T-3, T-9` while the task waits in Backlog, naming the prerequisites that are not Done; a blocked task names them in its Blocked badge. */
export function needsText(
  card: Pick<BoardCard, "status" | "blocked" | "blocked_by">,
): string | null {
  if (
    card.blocked_by.length === 0 ||
    card.blocked ||
    card.status !== "backlog"
  ) {
    return null;
  }
  return `needs ${card.blocked_by.join(", ")}`;
}

/** The blocked indicator, listing the unfinished prerequisites. */
export function blockedText(
  card: Pick<BoardCard, "blocked" | "blocked_by">,
): string | null {
  if (!card.blocked) {
    return null;
  }
  return card.blocked_by.length > 0
    ? `Blocked by ${card.blocked_by.join(", ")}`
    : "Blocked";
}

const PROPOSAL_TEXT: Record<CardProposalState, string> = {
  open: "waiting for review",
  approved: "approved",
  changes_requested: "changes requested",
  conflict: "conflict",
  integrated: "integrated",
};

/** An open proposal that waits in a review plan reads as waiting for its batch; the plan holds only open proposals. */
export function proposalText(proposal: BoardProposal): string {
  const state =
    proposal.review_plan !== null && proposal.state === "open"
      ? planWaitLabel(proposal.review_plan)
      : proposal.state === "integrated" && proposal.revision !== null
        ? `integrated as ${proposal.revision}`
        : PROPOSAL_TEXT[proposal.state];
  return `${proposal.id} · ${state}`;
}

/** The line under the pill of a proposal that waits in a review plan: the awaited tasks and when the wait ends, or that the batch waits for its reviewer; null otherwise. */
export function planWaitText(
  proposal: BoardProposal,
  now: number,
): string | null {
  return planWait(proposal.review_plan, proposal.id, now)?.detail ?? null;
}

export type ProposalLook = "attention" | "active" | "merged";

/** How the pill's icon looks: changes requested and conflict draw attention, an integrated proposal is history. */
export function proposalLook(proposal: BoardProposal): ProposalLook {
  switch (proposal.state) {
    case "changes_requested":
    case "conflict":
      return "attention";
    case "integrated":
      return "merged";
    default:
      return "active";
  }
}

/** `no proposal` on the card of a task that a lead check completes, `human task` on work a person does; null for a task that ends with a proposal. */
export function kindText(card: Pick<BoardCard, "kind">): string | null {
  switch (card.kind) {
    case "no_proposal":
      return "no proposal";
    case "human":
      return "human task";
    case "proposal":
      return null;
  }
}

export interface LeadCheckMark {
  text: string;
  /** The task worker's report note. */
  note: string;
}

/** The mark of a task without a proposal whose report waits in Review for a lead check; null otherwise. */
export function leadCheckMark(
  card: Pick<BoardCard, "kind" | "status" | "report">,
): LeadCheckMark | null {
  if (
    card.kind !== "no_proposal" ||
    card.status !== "review" ||
    card.report === null
  ) {
    return null;
  }
  return { text: "Waits for lead check", note: card.report };
}
