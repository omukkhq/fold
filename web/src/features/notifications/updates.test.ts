import { describe, expect, it } from "vitest";
import type { FeedEntry } from "./feed";
import { movesByTask, newText } from "./updates";

function entry(
  seq: number,
  task: string,
  from: FeedEntry["from"],
  to: FeedEntry["to"],
  extra: Partial<FeedEntry> = {},
): FeedEntry {
  return {
    seq,
    task,
    from,
    to,
    at: seq * 1_000,
    title: null,
    unread: false,
    ...extra,
  };
}

describe("movesByTask", () => {
  it("joins one task's moves into one path, oldest first", () => {
    const rows = movesByTask([
      entry(3, "T-5", "review", "done", { title: "Format counts" }),
      entry(2, "T-5", "in_progress", "review"),
      entry(1, "T-9", "todo", "in_progress", { unread: true }),
    ]);
    expect(rows).toEqual([
      {
        task: "T-5",
        title: "Format counts",
        path: ["in_progress", "review", "done"],
        at: 3_000,
        unread: false,
      },
      {
        task: "T-9",
        title: null,
        path: ["todo", "in_progress"],
        at: 1_000,
        unread: true,
      },
    ]);
  });

  it("puts the task whose newest move is newest first and keeps the newest title", () => {
    const rows = movesByTask([
      entry(4, "T-1", "todo", "in_progress", { title: "New name" }),
      entry(3, "T-2", "todo", "in_progress"),
      entry(1, "T-1", "backlog", "todo", { title: "Old name" }),
    ]);
    expect(rows.map((row) => row.task)).toEqual(["T-1", "T-2"]);
    expect(rows[0]?.title).toBe("New name");
  });

  it("orders moves of the same time by their seq", () => {
    const rows = movesByTask([
      entry(2, "T-3", "in_progress", "review", { at: 5 }),
      entry(1, "T-3", "todo", "in_progress", { at: 5 }),
    ]);
    expect(rows[0]?.path).toEqual(["todo", "in_progress", "review"]);
  });

  it("has no rows for an empty feed", () => {
    expect(movesByTask([])).toEqual([]);
  });
});

describe("newText", () => {
  it("names the new moves, and nothing when none is new", () => {
    expect(newText(3)).toBe("3 new");
    expect(newText(0)).toBeNull();
  });
});
