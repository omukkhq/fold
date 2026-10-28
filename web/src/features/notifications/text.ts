import { COLUMNS } from "@/features/board/columns";
import { columnName, type CardMove } from "./moves";

/** A toast cuts a longer title to this many characters. */
export const TOAST_TITLE = 40;

export function moveText(move: Pick<CardMove, "task" | "to">): string {
  return `${move.task} moved to ${columnName(move.to)}`;
}

function shortTitle(title: string): string {
  return title.length > TOAST_TITLE
    ? `${title.slice(0, TOAST_TITLE - 1).trimEnd()}…`
    : title;
}

/** `T-12 “Add the bell” moved to Review`, or without the title when the board did not hold the card. */
export function toastLine(
  move: Pick<CardMove, "task" | "to"> & { title: string | null },
): string {
  return move.title === null
    ? moveText(move)
    : `${move.task} “${shortTitle(move.title)}” moved to ${columnName(move.to)}`;
}

/**
 * The parts of one toast for a batch of moves. A task that moved more than once counts once, with the column it reached last; a task that ended in the column it started from is left out, so the parts can be empty. One task left gets its own line; more are counted per column, in the board's column order.
 */
export function toastParts(
  moves: readonly (CardMove & { title: string | null })[],
): string[] {
  const started = new Map<string, CardMove["from"]>();
  const last = new Map<string, CardMove & { title: string | null }>();
  for (const move of [...moves].sort((a, b) => a.seq - b.seq)) {
    if (!started.has(move.task)) {
      started.set(move.task, move.from);
    }
    last.set(move.task, move);
  }
  const ended = [...last.values()].filter(
    (move) => started.get(move.task) !== move.to,
  );
  if (ended.length === 1) {
    return ended.map(toastLine);
  }
  const counts = new Map<CardMove["to"], number>();
  for (const move of ended) {
    counts.set(move.to, (counts.get(move.to) ?? 0) + 1);
  }
  return COLUMNS.flatMap((column) => {
    const count = counts.get(column.status);
    return count === undefined ? [] : [{ name: column.name, count }];
  }).map(({ name, count }, index) => {
    const cards = index > 0 ? "" : count === 1 ? " card" : " cards";
    return `${String(count)}${cards} moved to ${name}`;
  });
}

/** The toast as one line, such as `3 cards moved to Review, 1 moved to Done`. */
export function toastText(
  moves: readonly (CardMove & { title: string | null })[],
): string {
  return toastParts(moves).join(", ");
}

/** The accessible name of the bell button. */
export function bellLabel(unread: number): string {
  return unread === 0
    ? "Notifications"
    : `Notifications, ${String(unread)} unread`;
}
