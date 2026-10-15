import { describe, expect, it } from "vitest";
import type { BoardCard, TaskStatus } from "@/api/board";
import { INITIAL_STATUS } from "@/live/status";
import { groupColumns } from "./columns";
import { cardCount, needsYouLook, reconnectAsOf, summaryText } from "./view";

function make(task: string, status: TaskStatus): BoardCard {
  return {
    task,
    title: task,
    status,
    blocked: false,
    labels: [],
    checklist: {
      steps_done: 0,
      steps_total: 0,
      linked_done: 0,
      linked_total: 0,
    },
    needs: [],
    blocks: 0,
    worker: null,
    workspace: null,
    activity: null,
    proposal: null,
    human_review: null,
    attention: [],
  };
}

describe("summaryText", () => {
  it("counts the tasks and the ready ones", () => {
    const columns = groupColumns([
      make("T-1", "todo"),
      make("T-2", "todo"),
      make("T-3", "done"),
    ]);
    expect(cardCount(columns)).toBe(3);
    expect(summaryText(columns)).toBe("3 tasks · 2 ready");
  });

  it("says task for one", () => {
    expect(summaryText(groupColumns([make("T-1", "backlog")]))).toBe(
      "1 task · 0 ready",
    );
  });

  it("counts zero without a Todo column", () => {
    expect(summaryText([])).toBe("0 tasks");
  });

  it("says only 0 tasks for an empty board, as the empty frame does", () => {
    expect(summaryText(groupColumns([]))).toBe("0 tasks");
  });

  it("counts ready as zero when the Todo column is missing", () => {
    const columns = groupColumns([make("T-1", "done")]).filter(
      (column) => column.status !== "todo",
    );
    expect(summaryText(columns)).toBe("1 task · 0 ready");
  });
});

describe("reconnectAsOf", () => {
  const status = (
    lastEventAt: number | null,
    stateAsOf: number | null = null,
  ) => ({
    state: "reconnecting" as const,
    lastEventAt,
    stateAsOf,
    retryAt: null,
  });

  it("is null while the stream is connecting or live", () => {
    expect(reconnectAsOf(INITIAL_STATUS, 5)).toBeNull();
    expect(
      reconnectAsOf({ ...status(1, 1), state: "live" as const }, 5),
    ).toBeNull();
  });

  it("is the later of the last event and the last fetch, never the older snapshot", () => {
    expect(reconnectAsOf(status(9, 1), 5)).toBe(9);
    expect(reconnectAsOf(status(2, 1), 5)).toBe(5);
    expect(reconnectAsOf(status(null), 5)).toBe(5);
  });
});

describe("needsYouLook", () => {
  it("is ink with the count, ink with a dash while loading, and plain at zero", () => {
    expect(needsYouLook(3)).toEqual({ variant: "primary", pill: "3" });
    expect(needsYouLook(null)).toEqual({ variant: "primary", pill: "–" });
    expect(needsYouLook(0)).toEqual({ variant: "secondary", pill: null });
  });
});
