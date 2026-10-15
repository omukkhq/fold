import { QueryClient } from "@tanstack/react-query";
import { describe, expect, it } from "vitest";
import type { Board } from "@/api/board";
import { keys } from "@/api/keys";
import type { TaskDetail } from "@/api/tasks";
import { boardCard, boardView, taskView } from "@/test/payloads";
import { applyEvent } from "./apply";
import { isEventEnvelope, type EventEnvelope } from "./envelope";

const slug = "demo";

/** Event frames exactly as the daemon writes them: `task.status_changed` carries `{from, to}` and `task.blocked_changed` carries `{blocked, blocked_by}`. */
const STATUS_CHANGED = {
  kind: "event",
  seq: 41,
  type: "task.status_changed",
  actor: "fold",
  task: "T-2",
  proposal: null,
  workspace: null,
  revision: null,
  data: { from: "todo", to: "in_progress" },
  created_at: 1_760_000_000_000,
};

const BLOCKED_CHANGED = {
  kind: "event",
  seq: 42,
  type: "task.blocked_changed",
  actor: "fold",
  task: "T-3",
  proposal: null,
  workspace: null,
  revision: null,
  data: { blocked: true, blocked_by: ["T-2"] },
  created_at: 1_760_000_000_100,
};

function frame(value: unknown): EventEnvelope {
  if (!isEventEnvelope(value)) {
    throw new Error("not a frame");
  }
  return value;
}

function seeded() {
  const client = new QueryClient();
  const board = boardView(
    [
      boardCard("T-2", { status: "todo" }),
      boardCard("T-3", { status: "todo", blocked_by: [] }),
    ],
    { done: 7 },
  );
  client.setQueryData(keys.board(slug), board);
  client.setQueryData(keys.task(slug, "T-2"), taskView("T-2"));
  client.setQueryData(keys.task(slug, "T-3"), taskView("T-3"));
  return client;
}

describe("recorded task.status_changed", () => {
  it("moves the card to the column named by `to`, keeps the totals, and patches the task", () => {
    const client = seeded();
    applyEvent(client, slug, frame(STATUS_CHANGED));
    const board = client.getQueryData<Board>(keys.board(slug));
    const column = (status: string) =>
      board?.columns.find((c) => c.status === status);
    expect(column("todo")?.cards.map((c) => c.task)).toEqual(["T-3"]);
    expect(column("in_progress")?.cards.map((c) => c.task)).toEqual(["T-2"]);
    expect(column("in_progress")?.cards[0]?.status).toBe("in_progress");
    expect(column("todo")?.total).toBe(1);
    expect(column("in_progress")?.total).toBe(1);
    expect(column("done")?.total).toBe(7);
    expect(
      client.getQueryData<TaskDetail>(keys.task(slug, "T-2"))?.status,
    ).toBe("in_progress");
  });
});

describe("recorded task.status_changed in another task read", () => {
  it("gives the new status to the edges and linked items that name the task", () => {
    const client = seeded();
    const edge = {
      task: "T-2",
      title: "Title of T-2",
      status: "todo" as const,
      direction: "needs" as const,
      origin: "checklist" as const,
    };
    const other = {
      ...edge,
      task: "T-4",
      direction: "needed_by" as const,
      origin: "direct" as const,
    };
    client.setQueryData(
      keys.task(slug, "T-3"),
      taskView("T-3", {
        dependencies: [edge, other],
        checklists: [
          {
            name: "Subtasks",
            items: [
              { id: "ci-1", kind: "step", text: "A step", done: false },
              {
                id: "ci-2",
                kind: "linked",
                task: "T-2",
                title: "Title of T-2",
                status: "todo",
                done: false,
              },
            ],
          },
          { name: "Notes", items: [] },
        ],
      }),
    );
    const untouched = taskView("T-5");
    client.setQueryData(keys.task(slug, "T-5"), untouched);
    applyEvent(client, slug, frame(STATUS_CHANGED));
    const read = client.getQueryData<TaskDetail>(keys.task(slug, "T-3"));
    expect(read?.dependencies.map((d) => [d.task, d.status])).toEqual([
      ["T-2", "in_progress"],
      ["T-4", "todo"],
    ]);
    expect(
      read?.checklists[0]?.items.map((i) => i.kind === "linked" && i.status),
    ).toEqual([false, "in_progress"]);
    expect(read?.checklists[1]).toEqual({ name: "Notes", items: [] });
    expect(client.getQueryData(keys.task(slug, "T-5"))).toBe(untouched);
    expect(client.getQueryState(keys.task(slug, "T-3"))?.isInvalidated).toBe(
      false,
    );
  });
});

describe("recorded task.blocked_changed", () => {
  it("sets the flag and the unfinished prerequisites on the card and the flag on the task", () => {
    const client = seeded();
    applyEvent(client, slug, frame(BLOCKED_CHANGED));
    const card = client
      .getQueryData<Board>(keys.board(slug))
      ?.columns.flatMap((c) => c.cards)
      .find((c) => c.task === "T-3");
    expect(card?.blocked).toBe(true);
    expect(card?.blocked_by).toEqual(["T-2"]);
    const task = client.getQueryData<TaskDetail>(keys.task(slug, "T-3"));
    expect(task?.blocked).toBe(true);
    expect(task).not.toHaveProperty("blocked_by");
  });

  it("refetches the board when the prerequisites are missing from the payload", () => {
    const client = seeded();
    applyEvent(
      client,
      slug,
      frame({ ...BLOCKED_CHANGED, data: { blocked: true } }),
    );
    expect(client.getQueryState(keys.board(slug))?.isInvalidated).toBe(true);
  });
});

describe("recorded label.created", () => {
  it("refreshes the label list and the board", () => {
    const client = seeded();
    client.setQueryData(keys.labels(slug), []);
    applyEvent(
      client,
      slug,
      frame({
        ...STATUS_CHANGED,
        type: "label.created",
        task: null,
        data: { name: "ui", color: "sage" },
      }),
    );
    expect(client.getQueryState(keys.labels(slug))?.isInvalidated).toBe(true);
  });
});
