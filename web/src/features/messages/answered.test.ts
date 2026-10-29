import { describe, expect, it } from "vitest";
import { keys } from "@/api/keys";
import {
  answeredKeys,
  answeredNote,
  markResetDue,
  taskCanMarkAnswered,
  threadCanMarkAnswered,
} from "./answered";

describe("answeredKeys", () => {
  it("refreshes every read that shows the ask, and the task read for a task", () => {
    expect(answeredKeys("demo", { thread: "m-3" })).toEqual([
      keys.messages("demo"),
      keys.board("demo"),
      keys.needsYou("demo"),
    ]);
    expect(answeredKeys("demo", { task: "T-2" })).toContainEqual(
      keys.task("demo", "T-2"),
    );
  });
});

describe("answeredNote", () => {
  it("says the ask was already answered only when the daemon closed none", () => {
    expect(answeredNote(undefined)).toBeNull();
    expect(answeredNote(["m-3"])).toBeNull();
    expect(answeredNote([])).toBe("It was already answered.");
  });
});

describe("taskCanMarkAnswered", () => {
  it("offers the control only for a question a human can close", () => {
    expect(taskCanMarkAnswered([{ kind: "question", closable: true }])).toBe(
      true,
    );
    expect(taskCanMarkAnswered([{ kind: "question", closable: false }])).toBe(
      false,
    );
    expect(
      taskCanMarkAnswered([
        { kind: "human_review", closable: false },
        { kind: "stalled", closable: false },
      ]),
    ).toBe(false);
    expect(taskCanMarkAnswered([])).toBe(false);
  });
});

describe("threadCanMarkAnswered", () => {
  it("offers the control only while the thread waits on an ask a human can close", () => {
    expect(
      threadCanMarkAnswered({ awaiting_human: true, closable: true }),
    ).toBe(true);
    expect(
      threadCanMarkAnswered({ awaiting_human: true, closable: false }),
    ).toBe(false);
    expect(
      threadCanMarkAnswered({ awaiting_human: false, closable: false }),
    ).toBe(false);
  });
});

describe("markResetDue", () => {
  it("forgets the last call only when the control moves to another thread", () => {
    expect(markResetDue("m-1", "m-5")).toBe(true);
    expect(markResetDue("m-1", "m-1")).toBe(false);
    expect(markResetDue("m-1", null)).toBe(false);
    expect(markResetDue(null, "m-5")).toBe(false);
  });
});
