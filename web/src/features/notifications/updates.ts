import type { TaskStatus } from "@/api/board";
import type { FeedEntry } from "./feed";

/** The moves of one task in the Inbox's Updates list, as one row. */
export interface TaskMoves {
  task: string;
  /** The newest title the moves carry, or null when no move had the card's title. */
  title: string | null;
  /** The columns the card passed through, oldest first: where it started, then where each move took it. */
  path: TaskStatus[];
  /** The time of the newest move, in epoch milliseconds. */
  at: number;
  /** True when one of the moves arrived since the list was last shown. */
  unread: boolean;
}

/**
 * The feed as one row per task, the task whose newest move is newest first. A row's path follows its moves in the order they happened, so `T-5 moved In progress → Review → Done` reads as the card travelled.
 */
export function movesByTask(feed: readonly FeedEntry[]): TaskMoves[] {
  const rows = new Map<string, FeedEntry[]>();
  for (const entry of feed) {
    rows.set(entry.task, [...(rows.get(entry.task) ?? []), entry]);
  }
  return [...rows.entries()]
    .map(([task, entries]): TaskMoves => {
      const oldestFirst = [...entries].sort(
        (a, b) => a.at - b.at || a.seq - b.seq,
      );
      return {
        task,
        title:
          [...oldestFirst].reverse().find((e) => e.title !== null)?.title ??
          null,
        path: [
          ...oldestFirst.slice(0, 1).map((e) => e.from),
          ...oldestFirst.map((e) => e.to),
        ],
        at: Math.max(...entries.map((e) => e.at)),
        unread: entries.some((e) => e.unread),
      };
    })
    .sort((a, b) => b.at - a.at);
}

/** The Updates tab's note of what arrived since the list was last shown, such as `3 new`; null when nothing is new. */
export function newText(unread: number): string | null {
  return unread === 0 ? null : `${String(unread)} new`;
}
