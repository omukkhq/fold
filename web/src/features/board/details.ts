import type { BoardCard } from "@/api/board";
import { COLUMNS } from "./columns";
import { attentionMark } from "./attention";
import { liveFilesText, type ActivityLine } from "./activityLine";
import {
  checklistText,
  kindText,
  planWaitText,
  proposalLook,
  proposalText,
  type ProposalLook,
} from "./card";

export type DetailRow =
  | { kind: "text"; name: string; text: string }
  | { kind: "person"; name: string; person: string }
  | { kind: "proposal"; name: string; text: string; look: ProposalLook }
  | { kind: "labels"; name: string };

export interface CardDetails {
  /** `T-4 · In progress`. */
  head: string;
  rows: DetailRow[];
  /** Why the card needs a person, one sentence per attention item; null without one. */
  why: string | null;
  /** What a proposal in a review plan waits for. */
  notes: string[];
}

function plural(count: number, noun: string): string {
  return `${String(count)} ${noun}${count === 1 ? "" : "s"}`;
}

/**
 * Everything the detail panel of a card shows, the facts the card leaves out: the worker, the kind of work, the
 * links to other tasks, the proposal, the live files, the messages, human review, the checklist and the labels.
 */
export function cardDetails(
  card: BoardCard,
  line: ActivityLine | null,
  now: number,
): CardDetails {
  const rows: DetailRow[] = [];
  const notes: string[] = [];
  if (card.worker !== null) {
    rows.push({ kind: "person", name: "Worker", person: card.worker });
  }
  const kind = kindText(card);
  if (kind !== null) {
    rows.push({ kind: "text", name: "Kind", text: kind });
  }
  if (card.blocker !== null) {
    rows.push({ kind: "text", name: "Blocker", text: card.blocker.reason });
  }
  if (card.blocked_by.length > 0) {
    rows.push({
      kind: "text",
      name: "Needs",
      text: card.blocked_by.join(", "),
    });
  }
  if (card.blocks > 0) {
    rows.push({
      kind: "text",
      name: "Blocks",
      text: plural(card.blocks, "task"),
    });
  }
  if (card.proposal !== null) {
    rows.push({
      kind: "proposal",
      name: "Proposal",
      text: proposalText(card.proposal),
      look: proposalLook(card.proposal),
    });
    const wait = planWaitText(card.proposal, now);
    if (wait !== null) {
      notes.push(wait);
    }
  }
  if (card.report !== null && card.kind === "no_proposal") {
    rows.push({ kind: "text", name: "Report", text: card.report });
  }
  const files = liveFilesText(line);
  if (files !== null) {
    rows.push({ kind: "text", name: "Files", text: files });
  }
  if (card.messages > 0) {
    rows.push({ kind: "text", name: "Messages", text: String(card.messages) });
  }
  if (card.human_review !== null) {
    rows.push(
      card.human_review.reviewer === null
        ? { kind: "text", name: "Human review", text: "any human" }
        : {
            kind: "person",
            name: "Human review",
            person: card.human_review.reviewer,
          },
    );
  }
  const checklist = checklistText(card);
  if (checklist !== null) {
    rows.push({ kind: "text", name: "Checklist", text: checklist });
  }
  if (card.labels.length > 0) {
    rows.push({ kind: "labels", name: "Labels" });
  }
  const column = COLUMNS.find((c) => c.status === card.status);
  const mark = attentionMark(card.attention);
  return {
    head: `${card.task} · ${column?.name ?? card.status}`,
    rows,
    why: mark === null ? null : `${mark.tooltip}.`,
    notes,
  };
}
