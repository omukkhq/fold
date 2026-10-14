import type { Board, BoardCard, TaskStatus } from "@/api/board";
import { parseId } from "@/format/ids";

export interface ColumnSpec {
  status: TaskStatus;
  name: string;
  /** The line under the column name. */
  hint: string;
  /** The class of the status dot, from the `status-*` token. */
  dot: string;
}

/** The board status table's order, Backlog to Done. Full class names, so the Tailwind scanner sees every one. */
export const COLUMNS: readonly ColumnSpec[] = [
  {
    status: "backlog",
    name: "Backlog",
    hint: "Waiting on prerequisites",
    dot: "bg-status-backlog",
  },
  {
    status: "todo",
    name: "Todo",
    hint: "Ready to start",
    dot: "bg-status-todo",
  },
  {
    status: "in_progress",
    name: "In Progress",
    hint: "Author working",
    dot: "bg-status-progress",
  },
  {
    status: "review",
    name: "Review",
    hint: "Waiting on a reviewer",
    dot: "bg-status-review",
  },
  {
    status: "done",
    name: "Done",
    hint: "Integrated into main",
    dot: "bg-status-done",
  },
];

export interface ColumnView extends ColumnSpec {
  cards: BoardCard[];
}

/** The number of a task id; anything else sorts last. */
export function taskNumber(task: string): number {
  const id = parseId(task);
  return id?.kind === "task" ? id.number : Number.MAX_SAFE_INTEGER;
}

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

/**
 * Always the five columns, each card in the column of its own `status` and each column sorted by task number.
 * The live layer appends a moved card to the end of its new column, so the order is restored here.
 */
export function groupColumns(cards: readonly BoardCard[]): ColumnView[] {
  return COLUMNS.map((spec) => ({
    ...spec,
    cards: cards
      .filter((card) => card.status === spec.status)
      .sort((a, b) => taskNumber(a.task) - taskNumber(b.task)),
  }));
}
