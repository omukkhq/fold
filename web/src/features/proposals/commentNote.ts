import type { CommentSide, ProposalDetail } from "@/api/proposals";
import {
  commentsOnSide,
  OLD_SIDE_NOTE,
  type DetailSearch,
  type DetailView,
} from "./view";

/** What a selection shows: the comment box under the human's name, or the reason it takes no comment, with the way out when there is one. */
export type SelectionComment =
  | { kind: "comment"; human: string }
  | {
      kind: "note";
      text: string;
      link: { label: string; search: DetailSearch } | null;
    };

/**
 * What a selection on the shown version takes; the daemon files a human comment only on the current version of an open proposal.
 * The first reason shown is the one still in the way after the human answers it: a proposal that is not open takes no comment at all, an older version or the old side of a change needs another view, and a missing name matters only then.
 */
export function commentNote(
  proposal: ProposalDetail,
  human: string | null,
  view: DetailView,
  side: CommentSide,
  path: string,
): SelectionComment {
  const { id, state, current_version: current } = proposal.summary;
  if (state !== "open") {
    return {
      kind: "note",
      text: `${id} is ${state}, so it takes no new comments.`,
      link: null,
    };
  }
  if (view.version !== current) {
    return {
      kind: "note",
      text: `This is v${String(view.version)}. Comments go on the current version, v${String(current)}.`,
      link: { label: `Open v${String(current)}`, search: { path } },
    };
  }
  if (!commentsOnSide(view, side)) {
    return { kind: "note", text: OLD_SIDE_NOTE, link: null };
  }
  if (human === null) {
    return {
      kind: "note",
      text: "Tell fold your name first, then comment.",
      link: null,
    };
  }
  return { kind: "comment", human };
}
