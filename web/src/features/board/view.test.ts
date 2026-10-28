import { describe, expect, it } from "vitest";
import type { TaskStatus } from "@/api/board";
import { boardCard } from "@/test/payloads";
import { INITIAL_STATUS } from "@/live/status";
import { groupColumns } from "./columns";
import {
  cardCount,
  needsYouPill,
  phoneColumn,
  phoneHint,
  reconnectAsOf,
  summaryText,
  taskCountText,
} from "./view";

const make = (task: string, status: TaskStatus) => boardCard(task, { status });

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

  it("counts zero without a To do column", () => {
    expect(summaryText([])).toBe("0 tasks");
  });

  it("says only 0 tasks for an empty board", () => {
    expect(summaryText(groupColumns([]))).toBe("0 tasks");
  });

  it("counts ready as zero when the To do column is missing", () => {
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

describe("needsYouPill", () => {
  it("is the count, a dash while loading, and nothing at zero", () => {
    expect(needsYouPill(3)).toBe("3");
    expect(needsYouPill(null)).toBe("–");
    expect(needsYouPill(0)).toBeNull();
  });
});

describe("phoneColumn", () => {
  it("opens on the first column with a card that needs a person", () => {
    const columns = groupColumns([
      make("T-1", "todo"),
      boardCard("T-2", {
        status: "review",
        attention: [{ kind: "question", closable: true }],
      }),
    ]);
    expect(phoneColumn(columns)).toBe("review");
  });

  it("falls back to the first column with a card, then To do", () => {
    expect(phoneColumn(groupColumns([make("T-1", "in_progress")]))).toBe(
      "in_progress",
    );
    expect(phoneColumn(groupColumns([]))).toBe("todo");
  });
});

describe("phoneHint", () => {
  it("names the column and its hint", () => {
    expect(phoneHint("Backlog", "Waiting on prerequisites")).toBe(
      "Backlog: waiting on prerequisites",
    );
    expect(taskCountText(1)).toBe("1 task");
    expect(taskCountText(4)).toBe("4 tasks");
  });
});
