import { queryOptions } from "@tanstack/react-query";
import { fromFixture } from "./fixture";
import { keys } from "./keys";

/** The daemon's status strings, which are also the board's column keys. */
export type TaskStatus = "backlog" | "todo" | "in_progress" | "review" | "done";

/** The daemon computes every kind; the web derives none of them. */
export type AttentionKind =
  | "question"
  | "human_review"
  | "stalled"
  | "stale_claim";

export interface AttentionItem {
  kind: AttentionKind;
}

export interface BoardLabel {
  name: string;
  /** One of the eight label colour names. */
  color: string;
}

/** Plain steps and linked items are counted apart, so the card can show both together. */
export interface ChecklistCounts {
  steps_done: number;
  steps_total: number;
  linked_done: number;
  linked_total: number;
}

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

export interface HumanReviewMark {
  /** The named human whose approval integration waits for; null when any human may approve. */
  reviewer: string | null;
}

/** The last tracked edit in the task's workspace. */
export interface BoardActivity {
  /** Relative to the workspace root; null when the daemon cannot name one file. */
  path: string | null;
  /** UTC milliseconds. */
  at: number;
}

export interface BoardCard {
  /** @example T-12 */
  task: string;
  title: string;
  status: TaskStatus;
  blocked: boolean;
  labels: BoardLabel[];
  checklist: ChecklistCounts;
  /** The unfinished prerequisites, as public task ids. */
  needs: string[];
  /** How many open tasks wait for this one. */
  blocks: number;
  /** The worker holding the implementation claim, `agent` or `agent/worker`. */
  worker: string | null;
  /** @example ws-9 */
  workspace: string | null;
  activity: BoardActivity | null;
  proposal: BoardProposal | null;
  human_review: HumanReviewMark | null;
  attention: AttentionItem[];
}

export interface BoardColumn {
  status: TaskStatus;
  cards: BoardCard[];
}

/** Column-shaped because the live layer moves a card between columns by its `task` and the column `status`. */
export interface Board {
  columns: BoardColumn[];
}

const fixture = import.meta.env.DEV
  ? () => import("../../fixtures/board")
  : undefined;

export async function getBoard(slug: string): Promise<Board> {
  return fromFixture(fixture, (m) => m.board(slug), { columns: [] });
}

export const boardQuery = (slug: string) =>
  queryOptions({ queryKey: keys.board(slug), queryFn: () => getBoard(slug) });
