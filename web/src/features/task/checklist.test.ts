import { describe, expect, it } from "vitest";
import type { LinkedItem, StepItem } from "@/api/tasks";
import {
  isTickable,
  linkAddText,
  linkedStateText,
  linkRemoveText,
  moveTargets,
  progress,
} from "./checklist";

const step = (done: boolean, id = "s"): StepItem => ({
  id,
  kind: "step",
  text: "a step",
  done,
});
const linked = (done: boolean): LinkedItem => ({
  id: "l",
  kind: "linked",
  task: "T-13",
  title: "Form field components",
  status: done ? "done" : "in_progress",
  done,
});

describe("progress", () => {
  it("counts plain steps and linked items together", () => {
    const p = progress({
      items: [step(true), step(true), step(true), step(false), linked(true)],
    });
    expect(p).toEqual({ done: 4, total: 5, percent: 80, text: "4/5" });
  });

  it("rounds down, so the bar is never full before the last item", () => {
    const items = [
      ...Array.from({ length: 199 }, () => step(true)),
      step(false),
    ];
    expect(progress({ items }).percent).toBe(99);
  });

  it("is zero for an empty checklist", () => {
    expect(progress({ items: [] })).toEqual({
      done: 0,
      total: 0,
      percent: 0,
      text: "0/0",
    });
  });
});

describe("isTickable", () => {
  it("takes a hand tick on a plain step only", () => {
    expect(isTickable(step(false))).toBe(true);
    expect(isTickable(linked(false))).toBe(false);
    expect(isTickable(linked(true))).toBe(false);
  });
});

describe("moveTargets", () => {
  it("moves one place up or down, never past either end", () => {
    expect(moveTargets(0, 3)).toEqual({ up: null, down: 1 });
    expect(moveTargets(1, 3)).toEqual({ up: 0, down: 2 });
    expect(moveTargets(2, 3)).toEqual({ up: 1, down: null });
    expect(moveTargets(0, 1)).toEqual({ up: null, down: null });
  });
});

describe("the link texts", () => {
  it("say that adding a link adds the dependency", () => {
    expect(linkAddText("T-2")).toContain("also adds the dependency");
    expect(linkAddText("T-2")).toContain("T-2 will need it");
  });

  it("say that removing a link removes the dependency", () => {
    expect(linkRemoveText("T-2", "T-13")).toBe(
      "Removing the link also removes the dependency: T-2 will no longer need T-13.",
    );
  });

  it("say fold ticked a Done child", () => {
    expect(linkedStateText(linked(true), "Done")).toBe("Done · ticked by fold");
    expect(linkedStateText(linked(false), "In progress")).toBe("In progress");
  });
});
