import type { LiveStatus } from "@/live/status";
import type { ColumnView } from "./columns";

export function cardCount(columns: readonly ColumnView[]): number {
  return columns.reduce((sum, column) => sum + column.cards.length, 0);
}

/** `17 tasks · 3 ready`, where ready is the Todo column; an empty board says `0 tasks`. */
export function summaryText(columns: readonly ColumnView[]): string {
  const total = cardCount(columns);
  if (total === 0) {
    return "0 tasks";
  }
  const ready = columns.find((c) => c.status === "todo")?.cards.length ?? 0;
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

/** Never earlier than the data it labels, so a fresh event never reads as from the future. */
export function clockTime(
  times: readonly (number | null)[],
  current: number = Date.now(),
): number {
  return Math.max(current, ...times.map((t) => t ?? 0));
}

export interface NeedsYouLook {
  variant: "primary" | "secondary";
  /** The count pill's text, or null when the button says `· 0` in its label. */
  pill: string | null;
}

/** The Needs you button: ink with a count while items wait, ink with `–` while the board loads, plain at zero. */
export function needsYouLook(count: number | null): NeedsYouLook {
  if (count === null) {
    return { variant: "primary", pill: "–" };
  }
  return count > 0
    ? { variant: "primary", pill: String(count) }
    : { variant: "secondary", pill: null };
}
