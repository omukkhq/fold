import type { ThreadView } from "@/api/messages";
import type { CommentSide, ProposalDetail } from "@/api/proposals";
import type { DiffSelection, DiffThread } from "@/features/diff/selection";
import { idHref } from "@/routes/paths";
import { commentNote } from "./commentNote";
import { linesText } from "./threadsModel";
import { commentsOnSide, type DetailSearch, type DetailView } from "./view";

/** A human's question to a proposal's author about lines of one version, read from its thread. */
export interface LineQuestion {
  /** The question's message id. */
  id: string;
  thread: string;
  version: number;
  range: DiffSelection;
  /** The workers the question went to. */
  to: string[];
  /** A recipient wrote in the thread after the question. */
  answered: boolean;
}

function idNumber(id: string): number {
  return Number(id.slice(id.indexOf("-") + 1));
}

/** The questions about lines of `proposal` in `threads`, oldest first. */
export function lineQuestions(
  threads: readonly ThreadView[],
  proposal: string,
): LineQuestion[] {
  return threads
    .flatMap((thread) =>
      thread.messages.flatMap((message) => {
        const lines = message.lines;
        if (lines === null || message.proposal !== proposal) return [];
        const to = message.recipients.map((r) => r.name);
        const answered = thread.messages.some(
          (later) =>
            idNumber(later.id) > idNumber(message.id) &&
            to.includes(later.sender),
        );
        const question: LineQuestion = {
          id: message.id,
          thread: message.thread,
          version: lines.version,
          range: {
            path: lines.path,
            side: lines.side,
            start_line: lines.start_line,
            end_line: lines.end_line,
          },
          to,
          answered,
        };
        return [question];
      }),
    )
    .sort((a, b) => idNumber(a.id) - idNumber(b.id));
}

/** The questions the diff on screen marks: those about the shown version, and on a change since an earlier version only those about the new side, whose lines are the version's own. */
export function questionMarks(
  questions: readonly LineQuestion[],
  view: DetailView,
): DiffThread[] {
  return questions
    .filter(
      (q) => q.version === view.version && commentsOnSide(view, q.range.side),
    )
    .map((q) => ({ id: q.id, range: q.range }));
}

/** The marker's name for a screen reader and its tooltip: `Question m-12 to frontend about line 4, waiting for an answer`. */
export function questionLabel(question: LineQuestion): string {
  const to = question.to.join(", ");
  const state = question.answered ? "answered" : "waiting for an answer";
  return `Question ${question.id} to ${to} about ${linesText(question.range)}, ${state}`;
}

/** The thread on the Messages screen. */
export function questionHref(slug: string, question: LineQuestion): string {
  return idHref(slug, "message", question.thread);
}

export interface MenuNote {
  text: string;
  link: { label: string; search: DetailSearch } | null;
}

/** What a line selection offers: the human who asks or comments, `null` for an action the selection does not take, and the reasons for those. */
export interface LineMenu {
  ask: string | null;
  comment: string | null;
  notes: MenuNote[];
}

export const NAME_NOTE = "Tell fold your name first, then ask or comment.";

export const SINCE_OLD_SIDE_NOTE =
  "A change since an earlier version takes questions and comments on the new side only: its lines are the version's own.";

/**
 * The actions of a selection. A question goes to the author about any version of a proposal in any state; a comment only on the current version of an open proposal.
 * The old side of a change since an earlier version holds that version's lines, which neither reads, and a missing name stops both, so each shows one note in place of the menu.
 */
export function lineMenu(
  proposal: ProposalDetail,
  human: string | null,
  view: DetailView,
  side: CommentSide,
  path: string,
): LineMenu {
  if (human === null) {
    return {
      ask: null,
      comment: null,
      notes: [{ text: NAME_NOTE, link: null }],
    };
  }
  if (!commentsOnSide(view, side)) {
    return {
      ask: null,
      comment: null,
      notes: [{ text: SINCE_OLD_SIDE_NOTE, link: null }],
    };
  }
  const comment = commentNote(proposal, human, view, side, path);
  if (comment.kind === "comment") {
    return { ask: human, comment: human, notes: [] };
  }
  return {
    ask: human,
    comment: null,
    notes: [{ text: comment.text, link: comment.link }],
  };
}

/** The question box's head: `Ask the author about lines 2–4 · new side · v2`. */
export function askTitle(selection: DiffSelection, version: number): string {
  return `Ask the author about ${linesText(selection)} · ${selection.side} side · v${String(version)}`;
}
