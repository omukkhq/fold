import type {
  BoardCard,
  BoardProposal,
  HumanReviewMark,
  ProposalState,
} from "@/api/board";

/**
 * The task dialog's path; the dialog route renders the board beneath itself. `search` is the board's filters as a
 * query string, which the dialog route takes as its own, so the board beneath it and after it keep them.
 */
export function taskPath(slug: string, task: string, search = ""): string {
  return `/p/${encodeURIComponent(slug)}/t/${encodeURIComponent(task)}${search}`;
}

/** The task dialog's create form, over the board. */
export function newTaskPath(slug: string): string {
  return `/p/${encodeURIComponent(slug)}/t/new`;
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

/** `needs T-3, T-9` while the task waits in Backlog; a blocked task names them in its Blocked badge. */
export function needsText(
  card: Pick<BoardCard, "status" | "blocked" | "needs">,
): string | null {
  if (card.needs.length === 0 || card.blocked || card.status !== "backlog") {
    return null;
  }
  return `needs ${card.needs.join(", ")}`;
}

export function blocksText(card: Pick<BoardCard, "blocks">): string | null {
  return card.blocks > 0 ? `blocks ${String(card.blocks)}` : null;
}

/** The blocked indicator, listing the unfinished prerequisites. */
export function blockedText(
  card: Pick<BoardCard, "blocked" | "needs">,
): string | null {
  if (!card.blocked) {
    return null;
  }
  return card.needs.length > 0
    ? `Blocked by ${card.needs.join(", ")}`
    : "Blocked";
}

const PROPOSAL_TEXT: Record<ProposalState, string> = {
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

export function humanReviewText(mark: HumanReviewMark): string {
  return mark.reviewer === null
    ? "Human review"
    : `Human review · ${mark.reviewer}`;
}
