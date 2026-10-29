import { queryOptions } from "@tanstack/react-query";
import { api } from "./client";
import { keys } from "./keys";
import { unwrap } from "./result";
import type { components } from "./schema";

type Schemas = components["schemas"];

export type BoardLabel = Schemas["Label"];
export type TaskStatus = Schemas["TaskStatus"];
export type AttentionItem = Schemas["AttentionItem"];
export type AttentionKind = Schemas["AttentionKind"];
export type ChecklistCounts = Schemas["ChecklistCounts"];
export type HumanReviewMark = Schemas["HumanReviewMark"];

/** The review state a card shows for its task's proposal: an open proposal is told apart by its latest verdict or a conflict. A closed proposal leaves the card. */
export type CardProposalState = Schemas["ProposalState"];
export type BoardProposal = Schemas["BoardProposal"];
export type BoardActivity = Schemas["BoardActivity"];
export type LiveCheckRun = Schemas["LiveCheckRun"];
export type BoardCard = Schemas["BoardCard"];

export type BoardColumn = Schemas["BoardColumn"];
export type Board = Schemas["BoardView"];

export function getBoard(slug: string): Promise<Board> {
  return unwrap(
    api.GET("/api/v1/projects/{slug}/board", {
      params: { path: { slug } },
    }),
  );
}

/** The event stream keeps the board fresh, so a cached answer never goes stale by time. */
export const boardQuery = (slug: string) =>
  queryOptions({
    queryKey: keys.board(slug),
    queryFn: () => getBoard(slug),
    staleTime: Infinity,
  });

/** Every card of the board once, whichever column it arrived in. */
export function boardCards(board: Board): BoardCard[] {
  const seen = new Set<string>();
  const cards: BoardCard[] = [];
  for (const column of board.columns) {
    for (const card of column.cards) {
      if (!seen.has(card.task)) {
        seen.add(card.task);
        cards.push(card);
      }
    }
  }
  return cards;
}
