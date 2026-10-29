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
    name: "To do",
    hint: "Ready to start",
    dot: "bg-status-todo",
  },
  {
    status: "in_progress",
    name: "In progress",
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
  /** The number beside the column name: every task of the status, or the cards left by the filters while any is set. */
  count: number;
  /** Tasks of the status that the board payload does not carry; only the Done column holds a latest few. */
  hidden: number;
  /** How many cards of the status the board payload carries. */
  loaded: number;
}

/** The number of a task id; anything else sorts last. */
export function taskNumber(task: string): number {
  const id = parseId(task);
  return id?.kind === "task" ? id.number : Number.MAX_SAFE_INTEGER;
}

function byNumber(a: BoardCard, b: BoardCard): number {
  return taskNumber(a.task) - taskNumber(b.task);
}

/** The Done column holds the most recently completed tasks, newest first. */
function byCompletion(a: BoardCard, b: BoardCard): number {
  return (b.completed_at ?? 0) - (a.completed_at ?? 0) || byNumber(a, b);
}

/**
 * Always the five columns, each card in the column of its own `status`. Backlog to Review are sorted by task number
 * and Done by completion time; the live layer appends a moved card to the end of its new column, so the order is
 * restored here. `board` supplies the column totals.
 */
export function groupColumns(
  cards: readonly BoardCard[],
  board?: Board,
  filtered = false,
): ColumnView[] {
  return COLUMNS.map((spec) => {
    const mine = cards
      .filter((card) => card.status === spec.status)
      .sort(spec.status === "done" ? byCompletion : byNumber);
    const source = board?.columns.find((c) => c.status === spec.status);
    const total = source?.total ?? mine.length;
    return {
      ...spec,
      cards: mine,
      count: filtered ? mine.length : total,
      hidden: Math.max(0, total - (source?.cards.length ?? total)),
      loaded: source?.cards.length ?? mine.length,
    };
  });
}

/** The line under the column name; a column that holds only the latest tasks says how many. */
export function hintText(column: ColumnView): string {
  return column.hidden > 0
    ? `${column.hint} · latest ${String(column.loaded)}`
    : column.hint;
}

/** The link text for the tasks the payload leaves out, or null when it left none. */
export function moreText(column: ColumnView): string | null {
  return column.hidden > 0
    ? `${String(column.hidden)} more in Repository`
    : null;
}
