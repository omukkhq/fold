import { describe, expect, it } from "vitest";
import { ApiError } from "@/api/errors";
import type { TaskEdge } from "@/api/tasks";
import {
  canEditDependencies,
  cyclePath,
  cycleText,
  groupDependencies,
  needsTitle,
} from "./dependencies";

const edge = (
  task: string,
  direction: TaskEdge["direction"],
  status: TaskEdge["status"] = "todo",
  origin: TaskEdge["origin"] = "direct",
): TaskEdge => ({ task, title: task, status, direction, origin });

describe("groupDependencies", () => {
  it("puts prerequisites and dependents apart, in task-number order", () => {
    const groups = groupDependencies([
      edge("T-12", "needs"),
      edge("T-3", "needed_by"),
      edge("T-9", "needs"),
      edge("T-10", "needed_by"),
    ]);
    expect(groups.needs.map((r) => r.task)).toEqual(["T-9", "T-12"]);
    expect(groups.neededBy.map((r) => r.task)).toEqual(["T-3", "T-10"]);
  });

  it("flags the unfinished tasks and counts the unfinished prerequisites", () => {
    const groups = groupDependencies([
      edge("T-1", "needs", "done"),
      edge("T-2", "needs", "review"),
      edge("T-3", "needed_by", "backlog"),
    ]);
    expect(groups.needs.map((r) => r.unfinished)).toEqual([false, true]);
    expect(groups.neededBy[0]?.unfinished).toBe(true);
    expect(groups.waitingFor).toBe(1);
  });

  it("lets only a direct prerequisite edge be removed", () => {
    const groups = groupDependencies([
      edge("T-1", "needs", "todo", "direct"),
      edge("T-2", "needs", "todo", "checklist"),
      edge("T-3", "needed_by", "todo", "direct"),
    ]);
    expect(groups.needs.map((r) => r.removable)).toEqual([true, false]);
    expect(groups.neededBy[0]?.removable).toBe(false);
  });

  it("is empty for a task without edges", () => {
    expect(groupDependencies([])).toEqual({
      needs: [],
      neededBy: [],
      waitingFor: 0,
    });
  });
});

describe("needsTitle", () => {
  it("names the unfinished prerequisites", () => {
    expect(needsTitle(groupDependencies([edge("T-1", "needs", "done")]))).toBe(
      "Needs",
    );
    expect(needsTitle(groupDependencies([edge("T-1", "needs")]))).toBe(
      "Needs · 1 unfinished",
    );
  });
});

describe("canEditDependencies", () => {
  it("shows no editor on a Done task", () => {
    expect(canEditDependencies("done")).toBe(false);
    for (const status of [
      "backlog",
      "todo",
      "in_progress",
      "review",
    ] as const) {
      expect(canEditDependencies(status)).toBe(true);
    }
  });
});

describe("cyclePath", () => {
  it("reads the task ids of a dependency_cycle refusal", () => {
    const error = new ApiError("dependency_cycle", "cycle", {
      cycle: ["T-2", "T-5", "T-2"],
    });
    expect(cyclePath(error)).toEqual(["T-2", "T-5", "T-2"]);
    expect(cycleText(["T-2", "T-5", "T-2"])).toBe(
      "The edge would close a cycle: T-2 → T-5 → T-2.",
    );
  });

  it("is null for another code, a malformed path or a plain error", () => {
    expect(cyclePath(new ApiError("task_not_found", "x"))).toBeNull();
    expect(
      cyclePath(new ApiError("dependency_cycle", "x", { cycle: [] })),
    ).toBeNull();
    expect(
      cyclePath(new ApiError("dependency_cycle", "x", { cycle: [1, 2] })),
    ).toBeNull();
    expect(cyclePath(new ApiError("dependency_cycle", "x"))).toBeNull();
    expect(cyclePath(new Error("x"))).toBeNull();
    expect(
      cyclePath({ error: { code: "dependency_cycle", cycle: ["T-1"] } }),
    ).toBeNull();
  });
});
