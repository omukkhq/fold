import { describe, expect, it } from "vitest";
import type { TaskStatus } from "@/api/board";
import type { CardMove } from "./moves";
import { TOAST_TITLE, bellLabel, moveText, toastLine, toastText } from "./text";

function move(
  seq: number,
  task: string,
  to: TaskStatus,
  from: TaskStatus = "in_progress",
  title: string | null = null,
): CardMove & { title: string | null } {
  return { seq, task, from, to, at: seq, by: null, title };
}

describe("toastText", () => {
  it("lists the moves of a batch in the order they happened", () => {
    expect(
      toastText([move(2, "T-14", "done"), move(1, "T-12", "review")]),
    ).toBe("T-12 moved to Review · T-14 moved to Done");
  });

  it("names a task once, with the column it reached last", () => {
    expect(
      toastText([
        move(1, "T-12", "review"),
        move(2, "T-14", "review"),
        move(3, "T-12", "done", "review"),
      ]),
    ).toBe("T-14 moved to Review · T-12 moved to Done");
  });

  it("leaves out a task that ended in the column it started from", () => {
    expect(
      toastText([
        move(1, "T-12", "todo", "backlog"),
        move(2, "T-14", "done", "review"),
        move(3, "T-12", "backlog", "todo"),
      ]),
    ).toBe("T-14 moved to Done");
    expect(
      toastText([
        move(1, "T-12", "todo", "backlog"),
        move(2, "T-12", "backlog", "todo"),
      ]),
    ).toBe("");
  });

  it("counts the tasks past the third", () => {
    expect(
      toastText([
        move(1, "T-1", "todo"),
        move(2, "T-2", "todo"),
        move(3, "T-3", "todo"),
        move(4, "T-4", "todo"),
        move(5, "T-5", "backlog"),
      ]),
    ).toBe(
      "T-1 moved to Todo · T-2 moved to Todo · T-3 moved to Todo · 2 more",
    );
  });

  it("names the task's title and cuts a long one", () => {
    expect(
      toastText([move(1, "T-12", "review", "in_progress", "Add the bell")]),
    ).toBe("T-12 “Add the bell” moved to Review");
    const long = "a".repeat(TOAST_TITLE + 5);
    const line = toastLine({ task: "T-1", to: "done", title: long });
    expect(line).toBe(`T-1 “${"a".repeat(TOAST_TITLE - 1)}…” moved to Done`);
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
