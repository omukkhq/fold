import { describe, expect, it } from "vitest";
import { keys } from "@/api/keys";
import { answeredKeys, answeredNote } from "./answered";

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
  it("says why nothing changed only when the daemon closed no ask", () => {
    expect(answeredNote(undefined)).toBeNull();
    expect(answeredNote(["m-3"])).toBeNull();
    expect(answeredNote([])).toContain("your reply");
  });
});
