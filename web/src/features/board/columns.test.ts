import { describe, expect, it } from "vitest";
import { boardCards, type TaskStatus } from "@/api/board";
import { boardCard, boardView } from "@/test/payloads";
import {
  COLUMNS,
  groupColumns,
  hintText,
  moreText,
  taskNumber,
} from "./columns";

const make = (task: string, status: TaskStatus) => boardCard(task, { status });

describe("COLUMNS", () => {
  it("is the five statuses from Backlog to Done", () => {
    expect(COLUMNS.map((c) => c.status)).toEqual([
      "backlog",
      "todo",
      "in_progress",
      "review",
      "done",
    ]);
    expect(COLUMNS.map((c) => c.name)).toEqual([
      "Backlog",
      "To do",
      "In progress",
      "Review",
      "Done",
    ]);
  });

  it("gives every column a status dot token and a hint", () => {
    for (const column of COLUMNS) {
      expect(column.dot).toMatch(/^bg-status-/);
      expect(column.hint.length).toBeGreaterThan(0);
    }
  });
});

describe("taskNumber", () => {
  it("reads the number of a task id", () => {
    expect(taskNumber("T-12")).toBe(12);
  });

  it("sorts anything that is not a task id last", () => {
    expect(taskNumber("P-3")).toBe(Number.MAX_SAFE_INTEGER);
    expect(taskNumber("nonsense")).toBe(Number.MAX_SAFE_INTEGER);
  });
});

describe("groupColumns", () => {
  it("always gives five columns, also for no cards", () => {
    const columns = groupColumns([]);
    expect(columns).toHaveLength(5);
    expect(columns.every((c) => c.cards.length === 0)).toBe(true);
  });

  it("puts each card in the column of its status, sorted by task number", () => {
    const columns = groupColumns([
      make("T-10", "todo"),
      make("T-2", "todo"),
      make("T-9", "done"),
      make("T-1", "todo"),
    ]);
    const todo = columns.find((c) => c.status === "todo");
    expect(todo?.cards.map((c) => c.task)).toEqual(["T-1", "T-2", "T-10"]);
    expect(columns.find((c) => c.status === "done")?.cards).toHaveLength(1);
    expect(columns.find((c) => c.status === "backlog")?.cards).toHaveLength(0);
  });
});

describe("groupColumns totals", () => {
  it("counts every task of the status and reports the cards the payload leaves out", () => {
    const board = boardView([make("T-1", "done"), make("T-2", "done")], {
      done: 5,
    });
    const done = groupColumns(boardCards(board), board).find(
      (c) => c.status === "done",
    );
    expect(done?.count).toBe(5);
    expect(done?.hidden).toBe(3);
  });

  it("counts only the cards left by the filters while any is set", () => {
    const board = boardView([make("T-1", "done"), make("T-2", "done")], {
      done: 5,
    });
    const done = groupColumns([make("T-2", "done")], board, true).find(
      (c) => c.status === "done",
    );
    expect(done?.count).toBe(1);
    expect(done?.hidden).toBe(3);
  });

  it("lists the Done column newest completion first", () => {
    const done = groupColumns([
      boardCard("T-1", { status: "done", completed_at: 10 }),
      boardCard("T-2", { status: "done", completed_at: 30 }),
      boardCard("T-3", { status: "done", completed_at: 20 }),
      boardCard("T-4", { status: "done", completed_at: null }),
    ]).find((c) => c.status === "done");
    expect(done?.cards.map((c) => c.task)).toEqual([
      "T-2",
      "T-3",
      "T-1",
      "T-4",
    ]);
  });

  it("says how many tasks the Done column holds and links the rest", () => {
    const board = boardView([make("T-1", "done"), make("T-2", "done")], {
      done: 5,
    });
    const columns = groupColumns(boardCards(board), board);
    const done = columns.find((c) => c.status === "done");
    const todo = columns.find((c) => c.status === "todo");
    expect(done && hintText(done)).toBe("Integrated into main · latest 2");
    expect(done && moreText(done)).toBe("3 more in Repository");
    expect(todo && hintText(todo)).toBe("Ready to start");
    expect(todo && moreText(todo)).toBeNull();
  });

  it("never reports a negative hidden count", () => {
    const board = boardView([make("T-1", "todo"), make("T-2", "todo")], {
      todo: 1,
    });
    const todo = groupColumns(boardCards(board), board).find(
      (c) => c.status === "todo",
    );
    expect(todo?.hidden).toBe(0);
  });
});

describe("boardCards", () => {
  it("follows each card's status, not the column it arrived in", () => {
    const moved = { ...make("T-3", "review") };
    const cards = boardCards({
      columns: [
        { status: "todo", cards: [make("T-1", "todo"), moved], total: 2 },
        { status: "review", cards: [], total: 0 },
      ],
    });
    const review = groupColumns(cards).find((c) => c.status === "review");
    expect(review?.cards.map((c) => c.task)).toEqual(["T-3"]);
  });

  it("keeps a card that appears twice once", () => {
    const cards = boardCards({
      columns: [
        { status: "todo", cards: [make("T-1", "todo")], total: 1 },
        {
          status: "in_progress",
          cards: [make("T-1", "in_progress")],
          total: 1,
        },
      ],
    });
    expect(cards.map((c) => c.task)).toEqual(["T-1"]);
  });
});
