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

export type ProposalState =
  | "open"
  | "approved"
  | "changes_requested"
  | "conflict"
  | "integrated";

export interface BoardProposal {
  /** @example P-3 */
  id: string;
  state: ProposalState;
  /** The revision an integrated proposal became, such as `r8`; null before integration. */
  revision: string | null;
}

/** The last tracked edit in the task's workspace. */
export interface BoardActivity {
  /** Relative to the workspace root; null when the daemon cannot name one file. */
  path: string | null;
  /** UTC milliseconds. */
  at: number;
}

/**
 * The generated card plus fields the card can draw that the board payload does not carry. Each is optional; an absent
 * field draws no proposal pill and no payload activity line.
 */
export type BoardCard = Schemas["BoardCard"] & {
  proposal?: BoardProposal | null;
  activity?: BoardActivity | null;
};

export type BoardColumn = Omit<Schemas["BoardColumn"], "cards"> & {
  cards: BoardCard[];
};

export type Board = Omit<Schemas["BoardView"], "columns"> & {
  columns: BoardColumn[];
};

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
