import type { TaskStatus } from "@/api/board";
import type { DurableEvent } from "@/api/events";
import { COLUMNS } from "@/features/board/columns";
import { isRecord, type EventEnvelope } from "@/live/envelope";
import { isHumanName } from "@/people/names";

/** A task card that left one board column for another. */
export interface CardMove {
  /** The `seq` of the `task.status_changed` event, unique in the project. */
  seq: number;
  task: string;
  from: TaskStatus;
  to: TaskStatus;
  /** When the daemon committed the change, in epoch milliseconds. */
  at: number;
  /** The human (`human:ana`) whose command made the change, or null when no human's command did. */
  by: string | null;
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
 * Reads the card moves from a project's events in `seq` order, one reader per project.
 * The events of one transaction share their `created_at` and come in a row, so the reader treats such a row as one command.
 * The daemon inserts a new task in Backlog and calculates its status in the same transaction, so a task that is ready at once gets a `task.status_changed` beside its `task.created`. That change is the card's arrival on the board, not a move, and the reader skips it.
 * Every `task.status_changed` carries the `fold` actor because the status is calculated; the human whose command caused it is the human actor of another event in the same row, such as the `integration.succeeded` of an Integrate.
 */
export function createMoveReader(): MoveReader {
  let createdAt: number | null = null;
  let human: string | null = null;
  const created = new Set<string>();
  return (event) => {
    if (event.kind !== "event") {
      return null;
    }
    if (createdAt !== event.created_at) {
      createdAt = event.created_at;
      human = null;
      created.clear();
    }
    if (human === null && isHumanName(event.actor)) {
      human = event.actor;
    }
    if (event.task === null) {
      return null;
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
    return {
      seq: event.seq,
      task: event.task,
      from,
      to,
      at: event.created_at,
      by: human,
    };
  };
}

/** The task a `task.deleted` event removes, or null for any other frame. */
export function deletedTask(event: EventEnvelope): string | null {
  return event.kind === "event" && event.type === "task.deleted"
    ? event.task
    : null;
}

/**
 * The moves in pages of stored events, newest first, without the moves of a task that a later event in the pages deletes.
 * While older pages remain (`complete` false), the oldest row of the pages may miss its first events, such as the `task.created` of an arrival, so it is left out.
 */
export function historyMoves(
  events: readonly DurableEvent[],
  complete: boolean,
): CardMove[] {
  const ordered = [...events].sort((a, b) => a.seq - b.seq);
  const oldest = ordered[0]?.created_at;
  const read = createMoveReader();
  const deleted = new Set<string>();
  const moves: CardMove[] = [];
  for (const stored of ordered) {
    if (!complete && stored.created_at === oldest) {
      continue;
    }
    const event: EventEnvelope = { ...stored, kind: "event" };
    const gone = deletedTask(event);
    if (gone !== null) {
      deleted.add(gone);
    }
    const move = read(event);
    if (move !== null) {
      moves.push(move);
    }
  }
  return moves.filter((move) => !deleted.has(move.task)).reverse();
}
