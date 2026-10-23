import type { DurableEvent } from "@/api/events";
import { historyMoves, type CardMove } from "./moves";
import { FEED_LIMIT } from "./feed";

/** One page of the daemon's stored events, newest first, and the `before` of the next older page or null at the first event. */
export interface HistoryPage {
  events: DurableEvent[];
  before: number | null;
}

export type PageReader = (before: number | null) => Promise<HistoryPage>;

/** The events of one history page; the daemon's most. */
export const HISTORY_PAGE = 500;

/** The pages read at most for one fill, so a project with few moves among many events is not read to its first event. */
export const HISTORY_PAGES = 10;

/** The project's newest moves that `keeps` lets through, newest first: older pages are read until {@link FEED_LIMIT} such moves are found, the first event is reached or {@link HISTORY_PAGES} pages are read. */
export async function loadMoves(
  read: PageReader,
  keeps: (move: CardMove) => boolean,
): Promise<CardMove[]> {
  const events: DurableEvent[] = [];
  let before: number | null = null;
  for (let page = 0; page < HISTORY_PAGES; page += 1) {
    const next = await read(before);
    events.push(...next.events);
    before = next.before;
    const moves = historyMoves(events, before === null).filter(keeps);
    if (before === null || moves.length >= FEED_LIMIT) {
      return moves.slice(0, FEED_LIMIT);
    }
  }
  return historyMoves(events, false).filter(keeps).slice(0, FEED_LIMIT);
}
