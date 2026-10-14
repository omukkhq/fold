import { describe, expect, it } from "vitest";
import type { BoardCard, TaskStatus } from "@/api/board";
import { COLUMNS, boardCards, groupColumns, taskNumber } from "./columns";

function make(task: string, status: TaskStatus): BoardCard {
  return {
    task,
    title: task,
    status,
    blocked: false,
    labels: [],
    checklist: {
      steps_done: 0,
      steps_total: 0,
      linked_done: 0,
      linked_total: 0,
    },
    needs: [],
    blocks: 0,
    worker: null,
    workspace: null,
    activity: null,
    proposal: null,
    human_review: null,
    attention: [],
  };
}

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
      "Todo",
      "In Progress",
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

describe("boardCards", () => {
  it("follows each card's status, not the column it arrived in", () => {
    const moved = { ...make("T-3", "review") };
    const cards = boardCards({
      columns: [
        { status: "todo", cards: [make("T-1", "todo"), moved] },
        { status: "review", cards: [] },
      ],
    });
    const review = groupColumns(cards).find((c) => c.status === "review");
    expect(review?.cards.map((c) => c.task)).toEqual(["T-3"]);
  });

  it("keeps a card that appears twice once", () => {
    const cards = boardCards({
      columns: [
        { status: "todo", cards: [make("T-1", "todo")] },
        { status: "in_progress", cards: [make("T-1", "in_progress")] },
      ],
    });
    expect(cards.map((c) => c.task)).toEqual(["T-1"]);
  });
});
