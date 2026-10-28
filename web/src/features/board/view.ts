import type { LiveStatus } from "@/live/status";
import type { TaskStatus } from "@/api/board";
import type { ColumnView } from "./columns";

export function cardCount(columns: readonly ColumnView[]): number {
  return columns.reduce((sum, column) => sum + column.count, 0);
}

/** `17 tasks · 3 ready`, where ready is the To do column; an empty board says `0 tasks`. */
export function summaryText(columns: readonly ColumnView[]): string {
  const total = cardCount(columns);
  if (total === 0) {
    return "0 tasks";
  }
  const ready = columns.find((c) => c.status === "todo")?.count ?? 0;
  const noun = total === 1 ? "task" : "tasks";
  return `${String(total)} ${noun} · ${String(ready)} ready`;
}

/**
 * The time of the board on screen while the stream reconnects, or null while it is connected: the later of the last
 * applied event and the last fetch. The workspace snapshot is left out, because events after it also changed the board.
 */
export function reconnectAsOf(
  status: LiveStatus,
  dataUpdatedAt: number,
): number | null {
  if (status.state !== "reconnecting") {
    return null;
  }
  return Math.max(status.lastEventAt ?? 0, dataUpdatedAt);
}

/** The Attention button's gold count: the number while items wait, `–` while the board loads, and none at zero, where the label says `· 0`. */
export function needsYouPill(count: number | null): string | null {
  if (count === null) {
    return "–";
  }
  return count > 0 ? String(count) : null;
}

/** Below this width the board shows one column at a time under a column switcher. */
export const PHONE_LAYOUT = "(max-width: 47.99rem)";

/** The column a phone opens on: the first that holds a card that needs a person, else the first with a card, else To do. */
export function phoneColumn(columns: readonly ColumnView[]): TaskStatus {
  const needs = columns.find((c) =>
    c.cards.some((card) => card.attention.length > 0),
  );
  const filled = columns.find((c) => c.cards.length > 0);
  return (needs ?? filled)?.status ?? "todo";
}

/** The line under the phone's column switcher: `In progress: author working`. */
export function phoneHint(name: string, hint: string): string {
  return `${name}: ${hint.charAt(0).toLowerCase()}${hint.slice(1)}`;
}

/** `4 tasks`, beside the phone's column line. */
export function taskCountText(count: number): string {
  return `${String(count)} ${count === 1 ? "task" : "tasks"}`;
}
