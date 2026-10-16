import type { BoardCard, BoardProposal, CardProposalState } from "@/api/board";
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

export function blocksText(card: Pick<BoardCard, "blocks">): string | null {
  return card.blocks > 0 ? `blocks ${String(card.blocks)}` : null;
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

/** The message-square count on a card; null when no message names the task. */
export function messagesText(card: Pick<BoardCard, "messages">): string | null {
  return card.messages > 0 ? String(card.messages) : null;
}

const PROPOSAL_TEXT: Record<CardProposalState, string> = {
  open: "waiting for review",
  approved: "approved",
  changes_requested: "changes requested",
  conflict: "conflict",
  integrated: "integrated",
};

export function proposalText(proposal: BoardProposal): string {
  const state =
    proposal.state === "integrated" && proposal.revision !== null
      ? `integrated as ${proposal.revision}`
      : PROPOSAL_TEXT[proposal.state];
  return `${proposal.id} · ${state}`;
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

export interface BlockerMark {
  text: string;
  /** Why the task waits for a person. */
  reason: string;
}

/** The blocker of a task that agents may not take until a human clears it; null without one. */
export function blockerMark(
  card: Pick<BoardCard, "blocker">,
): BlockerMark | null {
  return card.blocker === null
    ? null
    : { text: "Blocker", reason: card.blocker.reason };
}

export interface LeadCheckMark {
  text: string;
  /** The coder's report note. */
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
