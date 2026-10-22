import { describe, expect, it } from "vitest";
import type { EventEnvelope, StoredEvent } from "@/live/envelope";
import { cardTitle, columnName, createMoveReader } from "./moves";

function event(
  seq: number,
  type: StoredEvent["type"],
  task: string | null,
  data: Record<string, unknown> = {},
  at = 1_000,
): EventEnvelope {
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
    ).toEqual({ seq: 7, task: "T-12", from: "review", to: "done", at: 1_000 });
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

describe("columnName", () => {
  it("names the board column", () => {
    expect(columnName("in_progress")).toBe("In Progress");
    expect(columnName("todo")).toBe("Todo");
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
