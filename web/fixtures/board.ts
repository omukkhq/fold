import type { Board, BoardColumn, TaskStatus } from "../src/api/board";
import { demoCards } from "./demoBoard";
import { createdCards } from "./tasks";

const STATUSES: TaskStatus[] = [
  "backlog",
  "todo",
  "in_progress",
  "review",
  "done",
];

/** The `demo` project has a full board; every other project is empty. */
export function board(slug: string, now: number = Date.now()): Board {
  const all = [
    ...(slug === "demo" ? demoCards(now) : []),
    ...createdCards(slug),
  ];
  if (all.length === 0) {
    return { columns: [] };
  }
  const columns: BoardColumn[] = STATUSES.map((status) => ({
    status,
    cards: all.filter((c) => c.status === status),
  }));
  return { columns };
}
