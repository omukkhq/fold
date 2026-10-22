import { columnName, type CardMove } from "./moves";

/** A toast names at most this many tasks and counts the rest. */
export const TOAST_TASKS = 3;

export function moveText(move: Pick<CardMove, "task" | "to">): string {
  return `${move.task} moved to ${columnName(move.to)}`;
}

/** The parts of one toast for a batch of moves, such as `T-12 moved to Review` and `T-14 moved to Done`. A task that moved more than once in the batch is named once, with the column it reached last. */
export function toastParts(moves: readonly CardMove[]): string[] {
  const last = new Map<string, CardMove>();
  for (const move of [...moves].sort((a, b) => a.seq - b.seq)) {
    last.delete(move.task);
    last.set(move.task, move);
  }
  const lines = [...last.values()].map(moveText);
  const shown = lines.slice(0, TOAST_TASKS);
  const rest = lines.length - shown.length;
  return rest > 0 ? [...shown, `${String(rest)} more`] : shown;
}

/** The toast as one line, such as `T-12 moved to Review · T-14 moved to Done`. */
export function toastText(moves: readonly CardMove[]): string {
  return toastParts(moves).join(" · ");
}

/** The accessible name of the bell button. */
export function bellLabel(unread: number): string {
  return unread === 0
    ? "Notifications"
    : `Notifications, ${String(unread)} unread`;
}
