import { describe, expect, it } from "vitest";
import { heldTasks } from "./held";

describe("heldTasks", () => {
  it("sorts by task number and drops repeats", () => {
    expect(heldTasks({ held_by: ["T-10", "T-9", "T-10"] })).toEqual([
      "T-9",
      "T-10",
    ]);
  });

  it("is empty when the daemon names none", () => {
    expect(heldTasks({})).toEqual([]);
    expect(heldTasks({ held_by: [] })).toEqual([]);
  });
});
