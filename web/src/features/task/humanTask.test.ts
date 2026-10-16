import { describe, expect, it } from "vitest";
import type { TaskEdge } from "@/api/tasks";
import { segmentsText } from "@/people/segments";
import { taskView } from "@/test/payloads";
import {
  blockerControls,
  blockerView,
  humanWork,
  KIND_OPTIONS,
} from "./humanTask";

function edge(task: string, over: Partial<TaskEdge> = {}): TaskEdge {
  return {
    task,
    title: `Title of ${task}`,
    status: "todo",
    direction: "needs",
    origin: "direct",
    ...over,
  };
}

const HOLD = {
  reason: "Wait for the logo",
  set_by: "web-lead",
  set_at: 1_000,
};

describe("humanWork", () => {
  it("is null for a task an agent works on", () => {
    expect(humanWork(taskView("T-1"))).toBeNull();
    expect(humanWork(taskView("T-1", { kind: "no_proposal" }))).toBeNull();
  });

  it("lets a named human mark open work Done", () => {
    const human = taskView("T-2", { kind: "human", can_mark_done: true });
    expect(humanWork(human)).toMatchObject({
      state: "open",
      waitsFor: [],
      canMarkDone: true,
    });
    expect(humanWork({ ...human, can_mark_done: false })?.canMarkDone).toBe(
      false,
    );
  });

  it("waits for the unfinished prerequisites and names them", () => {
    const one = taskView("T-2", {
      kind: "human",
      can_mark_done: true,
      dependencies: [
        edge("T-3"),
        edge("T-4", { status: "done" }),
        edge("T-5", { direction: "needed_by" }),
      ],
    });
    const work = humanWork(one);
    expect(work).toMatchObject({
      state: "waits",
      waitsFor: ["T-3"],
      canMarkDone: false,
    });
    expect(work?.note).toContain("once T-3 is Done");
    const many = {
      ...one,
      dependencies: [edge("T-3"), edge("T-6"), edge("T-7")],
    };
    expect(humanWork(many)?.note).toContain("once T-3, T-6 and T-7 are Done");
  });

  it("shows Done and archived work without the control", () => {
    const done = taskView("T-2", {
      kind: "human",
      status: "done",
      can_mark_done: true,
    });
    expect(humanWork(done)).toMatchObject({
      state: "done",
      canMarkDone: false,
    });
    expect(humanWork({ ...done, archived_at: 5 })).toMatchObject({
      state: "archived",
      canMarkDone: false,
    });
  });
});

describe("blockerView", () => {
  it("shows the reason and who set it", () => {
    expect(blockerView(taskView("T-1"), 0)).toBeNull();
    const view = blockerView(taskView("T-1", { blocker: HOLD }), 61_000);
    expect(view?.reason).toBe("Wait for the logo");
    expect(segmentsText(view?.setBy ?? [])).toBe(
      "Set by @web-lead · 1 min ago",
    );
  });
});

describe("blockerControls", () => {
  it("offers what the daemon allows", () => {
    const task = taskView("T-1");
    expect(blockerControls(task)).toEqual({ canSet: false, canClear: false });
    expect(
      blockerControls({
        ...task,
        can_set_blocker: true,
        can_clear_blocker: true,
      }),
    ).toEqual({ canSet: true, canClear: true });
  });
});

describe("KIND_OPTIONS", () => {
  it("offers every kind once, the default first", () => {
    expect(KIND_OPTIONS.map((option) => option.value)).toEqual([
      "proposal",
      "no_proposal",
      "human",
    ]);
  });
});
