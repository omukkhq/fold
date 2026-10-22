import { describe, expect, it } from "vitest";
import type { TaskStatus } from "@/api/board";
import type { CardMove } from "./moves";
import { bellLabel, moveText, toastText } from "./text";

function move(seq: number, task: string, to: TaskStatus): CardMove {
  return { seq, task, from: "in_progress", to, at: seq };
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
        move(2, "T-14", "in_progress"),
        move(3, "T-12", "done"),
      ]),
    ).toBe("T-14 moved to In Progress · T-12 moved to Done");
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
