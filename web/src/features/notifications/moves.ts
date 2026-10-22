import type { TaskStatus } from "@/api/board";
import { COLUMNS } from "@/features/board/columns";
import { isRecord, type EventEnvelope } from "@/live/envelope";

/** A task card that left one board column for another. */
export interface CardMove {
  /** The `seq` of the `task.status_changed` event, unique in the project. */
  seq: number;
  task: string;
  from: TaskStatus;
  to: TaskStatus;
  /** When the daemon committed the change, in epoch milliseconds. */
  at: number;
}

const NAMES = new Map<string, string>(
  COLUMNS.map((column) => [column.status, column.name]),
);

export function isTaskStatus(value: unknown): value is TaskStatus {
  return typeof value === "string" && NAMES.has(value);
}

/** The board column's name, such as `In Progress`. */
export function columnName(status: TaskStatus): string {
  return NAMES.get(status) ?? status;
}

/** The title of the task's card in a cached board, or null when the cache does not hold the card. */
export function cardTitle(board: unknown, task: string): string | null {
  if (!isRecord(board) || !Array.isArray(board.columns)) {
    return null;
  }
  for (const column of board.columns) {
    if (!isRecord(column) || !Array.isArray(column.cards)) {
      continue;
    }
    for (const card of column.cards) {
      if (
        isRecord(card) &&
        card.task === task &&
        typeof card.title === "string"
      ) {
        return card.title;
      }
    }
  }
  return null;
}

export type MoveReader = (event: EventEnvelope) => CardMove | null;

/**
 * Reads the card moves from a project's stream, one reader per project.
 * The daemon inserts a new task in Backlog and calculates its status in the same transaction, so a task that is ready at once gets a `task.status_changed` with the `created_at` of its `task.created`. That change is the card's arrival on the board, not a move, and the reader skips it.
 */
export function createMoveReader(): MoveReader {
  let createdAt: number | null = null;
  const created = new Set<string>();
  return (event) => {
    if (event.kind !== "event" || event.task === null) {
      return null;
    }
    if (createdAt !== event.created_at) {
      createdAt = event.created_at;
      created.clear();
    }
    if (event.type === "task.created") {
      created.add(event.task);
      return null;
    }
    if (event.type !== "task.status_changed" || created.has(event.task)) {
      return null;
    }
    const { from, to } = event.data;
    if (!isTaskStatus(from) || !isTaskStatus(to) || from === to) {
      return null;
    }
    return { seq: event.seq, task: event.task, from, to, at: event.created_at };
  };
}
