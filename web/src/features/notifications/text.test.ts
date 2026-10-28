import { describe, expect, it } from "vitest";
import type { TaskStatus } from "@/api/board";
import type { CardMove } from "./moves";
import {
  TOAST_TITLE,
  bellLabel,
  moveText,
  toastLine,
  toastParts,
  toastText,
} from "./text";

function move(
  seq: number,
  task: string,
  to: TaskStatus,
  from: TaskStatus = "in_progress",
  title: string | null = null,
): CardMove & { title: string | null } {
  return { seq, task, from, to, at: seq, title };
}

describe("toastText", () => {
  it("names the task of a single move, with its title cut when long", () => {
    expect(toastText([move(1, "T-12", "review")])).toBe("T-12 moved to Review");
    expect(
      toastText([move(1, "T-12", "review", "in_progress", "Add PIN sign-in")]),
    ).toBe("T-12 “Add PIN sign-in” moved to Review");
    const long = "a".repeat(TOAST_TITLE + 5);
    const line = toastLine({ task: "T-1", to: "done", title: long });
    expect(line).toBe(`T-1 “${"a".repeat(TOAST_TITLE - 1)}…” moved to Done`);
  });

  it("counts two or more moves per column", () => {
    expect(
      toastText([
        move(1, "T-1", "review", "in_progress", "One"),
        move(2, "T-2", "review"),
        move(3, "T-3", "review"),
        move(4, "T-4", "done", "review"),
      ]),
    ).toBe("3 cards moved to Review, 1 moved to Done");
    expect(
      toastText([move(1, "T-1", "done", "review"), move(2, "T-2", "done")]),
    ).toBe("2 cards moved to Done");
    expect(
      toastText([move(1, "T-1", "todo", "backlog"), move(2, "T-2", "done")]),
    ).toBe("1 card moved to Todo, 1 moved to Done");
  });

  it("keeps the board's column order, whatever the order of the moves", () => {
    expect(
      toastParts([
        move(1, "T-1", "done", "review"),
        move(2, "T-2", "review"),
        move(3, "T-3", "done", "review"),
        move(4, "T-4", "backlog", "todo"),
      ]),
    ).toEqual([
      "1 card moved to Backlog",
      "1 moved to Review",
      "2 moved to Done",
    ]);
  });

  it("counts a task once, with the column it reached last", () => {
    expect(
      toastText([
        move(1, "T-12", "review"),
        move(2, "T-14", "review"),
        move(3, "T-12", "done", "review"),
      ]),
    ).toBe("1 card moved to Review, 1 moved to Done");
    expect(
      toastText([
        move(3, "T-12", "done", "review"),
        move(1, "T-12", "review"),
        move(2, "T-14", "done"),
      ]),
    ).toBe("2 cards moved to Done");
  });

  it("leaves out a task that ended in the column it started from", () => {
    expect(
      toastText([
        move(1, "T-12", "todo", "backlog"),
        move(2, "T-14", "done", "review", "Ship it"),
        move(3, "T-12", "backlog", "todo"),
      ]),
    ).toBe("T-14 “Ship it” moved to Done");
    expect(
      toastParts([
        move(1, "T-12", "todo", "backlog"),
        move(2, "T-12", "backlog", "todo"),
      ]),
    ).toEqual([]);
    expect(toastText([])).toBe("");
  });
});

describe("labels", () => {
  it("words one move", () => {
    expect(moveText({ task: "T-7", to: "backlog" })).toBe(
      "T-7 moved to Backlog",
    );
  });

  it("gives the bell a name with the unread count", () => {
    expect(bellLabel(0)).toBe("Notifications");
    expect(bellLabel(3)).toBe("Notifications, 3 unread");
  });
});
