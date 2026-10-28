import { columnName, type CardMove } from "./moves";

/** A toast names at most this many tasks and counts the rest. */
export const TOAST_TASKS = 3;

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
 * The parts of one toast for a batch of moves, in the order the tasks last moved. A task that moved more than once is named once, with the column it reached last; a task that ended in the column it started from is left out, so the parts can be empty.
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
    last.delete(move.task);
    last.set(move.task, move);
  }
  const lines = [...last.values()]
    .filter((move) => started.get(move.task) !== move.to)
    .map(toastLine);
  const shown = lines.slice(0, TOAST_TASKS);
  const rest = lines.length - shown.length;
  return rest > 0 ? [...shown, `${String(rest)} more`] : shown;
}

/** The toast as one line, such as `T-12 moved to Review · T-14 moved to Done`. */
export function toastText(
  moves: readonly (CardMove & { title: string | null })[],
): string {
  return toastParts(moves).join(" · ");
}
