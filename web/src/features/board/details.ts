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
  | {
      kind: "prose";
      name: string;
      /** Markdown that a person or an agent wrote. */
      text: string;
      /** The rendered lines the panel shows of a long text; null shows it whole. */
      clamp: typeof PREVIEW_LINES | null;
      /** The note under a clamped text that sends the person to the task, where the text shows in full. */
      more: string | null;
    }
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

/** A long free text shows this many rendered lines in the panel, so the panel stays short. */
export const PREVIEW_LINES = 4;
/** About the characters that fill those lines at the panel's width; a longer text is clamped. */
export const PREVIEW_CHARS = 160;

/** A text row that a person or an agent wrote: whole when it is short, cut to its first lines with a note when it is long. */
export function proseRow(
  name: string,
  text: string,
  what: string,
): Extract<DetailRow, { kind: "prose" }> {
  const lines = text
    .trim()
    .split("\n")
    .filter((line) => line.trim() !== "").length;
  const long = lines > PREVIEW_LINES || text.trim().length > PREVIEW_CHARS;
  return {
    kind: "prose",
    name,
    text,
    clamp: long ? PREVIEW_LINES : null,
    more: long ? `Open the task to read the full ${what}` : null,
  };
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
    rows.push(proseRow("Blocker", card.blocker.reason, "reason"));
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
    rows.push(proseRow("Report", card.report, "report"));
  }
  if (line?.run) {
    rows.push({ kind: "text", name: "Checks", text: `${line.run} running` });
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
  // The stall note under the activity line says who was told and what waits, so the stall needs no sentence of its own here.
  const mark = attentionMark(
    card.attention.filter((item) => item.kind !== "stalled"),
  );
  return {
    head: `${card.task} · ${column?.name ?? card.status}`,
    rows,
    why: mark === null ? null : `${mark.tooltip}.`,
    notes,
  };
}
