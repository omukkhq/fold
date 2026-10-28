import { describe, expect, it } from "vitest";
import type { EventEnvelope, StoredEvent } from "@/live/envelope";
import type { DurableEvent } from "@/api/events";
import {
  cardTitle,
  columnName,
  createMoveReader,
  deletedTask,
  historyMoves,
} from "./moves";

function event(
  seq: number,
  type: StoredEvent["type"],
  task: string | null,
  data: Record<string, unknown> = {},
  at = 1_000,
): EventEnvelope & DurableEvent {
  return {
    kind: "event",
    seq,
    type,
    actor: "fold",
    task,
    proposal: null,
    workspace: null,
    revision: null,
    data,
    created_at: at,
  };
}

describe("createMoveReader", () => {
  it("reads a column change from task.status_changed", () => {
    const read = createMoveReader();
    expect(
      read(
        event(7, "task.status_changed", "T-12", { from: "review", to: "done" }),
      ),
    ).toEqual({
      seq: 7,
      task: "T-12",
      from: "review",
      to: "done",
      at: 1_000,
    });
  });

  it("skips the first status of a task created in the same transaction", () => {
    const read = createMoveReader();
    expect(read(event(1, "task.created", "T-3"))).toBeNull();
    expect(
      read(
        event(2, "task.status_changed", "T-3", { from: "backlog", to: "todo" }),
      ),
    ).toBeNull();
    // A later transaction moves it again.
    expect(
      read(
        event(
          3,
          "task.status_changed",
          "T-3",
          { from: "todo", to: "in_progress" },
          2_000,
        ),
      ),
    ).toMatchObject({ seq: 3, to: "in_progress" });
  });

  it("still reads a move of another task in the creating transaction", () => {
    const read = createMoveReader();
    read(event(1, "task.created", "T-3"));
    expect(
      read(
        event(2, "task.status_changed", "T-1", { from: "todo", to: "backlog" }),
      ),
    ).toMatchObject({ task: "T-1" });
  });

  it("ignores other frames and malformed changes", () => {
    const read = createMoveReader();
    expect(read({ kind: "snapshot", seq: 1, workspaces: [] })).toBeNull();
    expect(read(event(1, "task.updated", "T-1"))).toBeNull();
    expect(
      read(event(2, "task.status_changed", null, { from: "todo", to: "done" })),
    ).toBeNull();
    expect(
      read(
        event(3, "task.status_changed", "T-1", { from: "todo", to: "later" }),
      ),
    ).toBeNull();
    expect(
      read(event(4, "task.status_changed", "T-1", { from: 1, to: "done" })),
    ).toBeNull();
    expect(
      read(
        event(5, "task.status_changed", "T-1", { from: "done", to: "done" }),
      ),
    ).toBeNull();
  });
});

describe("deletedTask", () => {
  it("names the task of task.deleted only", () => {
    expect(deletedTask(event(1, "task.deleted", "T-4"))).toBe("T-4");
    expect(deletedTask(event(1, "task.updated", "T-4"))).toBeNull();
    expect(deletedTask({ kind: "snapshot", seq: 1, workspaces: [] })).toBe(
      null,
    );
  });
});

describe("historyMoves", () => {
  const change = (seq: number, task: string, at: number) =>
    event(seq, "task.status_changed", task, { from: "todo", to: "done" }, at);

  it("reads the moves of newest-first pages, newest first", () => {
    const moves = historyMoves(
      [change(3, "T-2", 3_000), change(1, "T-1", 1_000)],
      true,
    );
    expect(moves.map((move) => move.seq)).toEqual([3, 1]);
  });

  it("skips an arrival and the moves of a task deleted later", () => {
    const moves = historyMoves(
      [
        event(5, "task.deleted", "T-1", {}, 5_000),
        change(4, "T-2", 4_000),
        change(3, "T-3", 2_000),
        event(2, "task.created", "T-3", {}, 2_000),
        change(1, "T-1", 1_000),
      ],
      true,
    );
    expect(moves.map((move) => move.task)).toEqual(["T-2"]);
  });

  it("leaves out the oldest transaction while older pages remain", () => {
    const events = [change(4, "T-2", 4_000), change(3, "T-3", 2_000)];
    expect(historyMoves(events, false).map((move) => move.task)).toEqual([
      "T-2",
    ]);
    expect(historyMoves([], false)).toEqual([]);
  });
});

describe("columnName", () => {
  it("names the board column", () => {
    expect(columnName("in_progress")).toBe("In progress");
    expect(columnName("todo")).toBe("To do");
  });
});

describe("cardTitle", () => {
  const board = {
    columns: [
      { status: "todo", cards: [{ task: "T-1", title: "Add the bell" }] },
      { status: "done", cards: ["broken", { task: "T-2", title: 3 }] },
      "broken",
      { status: "review" },
    ],
  };

  it("finds the card's title in the cached board", () => {
    expect(cardTitle(board, "T-1")).toBe("Add the bell");
  });

  it("is null without a board, a card or a title", () => {
    expect(cardTitle(undefined, "T-1")).toBeNull();
    expect(cardTitle({ columns: "x" }, "T-1")).toBeNull();
    expect(cardTitle(board, "T-2")).toBeNull();
    expect(cardTitle(board, "T-9")).toBeNull();
  });
});
