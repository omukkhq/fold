import { QueryClient } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { boardCards, groupColumns } from "../features/board/columns";
import { fixtureEventTransport } from "../../fixtures/events";
import { applyEvent } from "../live/apply";
import { connectProject } from "../live/connect";
import type { StoredEvent } from "../live/envelope";
import { createLiveStatusStore } from "../live/status";
import { boardQuery, getBoard, type Board } from "./board";
import { keys } from "./keys";

beforeEach(() => {
  vi.useFakeTimers();
});
afterEach(() => {
  vi.useRealTimers();
});

async function demoBoard(): Promise<Board> {
  const board = getBoard("demo");
  await vi.runAllTimersAsync();
  return board;
}

describe("the board fixture", () => {
  it("fills the five columns of the demo project", async () => {
    const board = await demoBoard();
    const columns = groupColumns(boardCards(board));
    expect(columns.map((c) => c.status)).toEqual([
      "backlog",
      "todo",
      "in_progress",
      "review",
      "done",
    ]);
    expect(columns.every((c) => c.cards.length > 0)).toBe(true);
  });

  it("holds a blocked card, every attention kind, both proposal problems and a human-review mark", async () => {
    const cards = boardCards(await demoBoard());
    const kinds = new Set(cards.flatMap((c) => c.attention.map((a) => a.kind)));
    expect([...kinds].sort()).toEqual([
      "human_review",
      "question",
      "stale_claim",
      "stalled",
    ]);
    expect(cards.some((c) => c.blocked && c.needs.length > 0)).toBe(true);
    const states = cards.map((c) => c.proposal?.state);
    expect(states).toContain("changes_requested");
    expect(states).toContain("conflict");
    expect(cards.some((c) => c.human_review !== null)).toBe(true);
  });

  it("is empty for every other project", async () => {
    const board = getBoard("ledger");
    await vi.runAllTimersAsync();
    expect(await board).toEqual({ columns: [] });
  });
});

describe("a production build", () => {
  afterEach(() => {
    vi.unstubAllEnvs();
    vi.resetModules();
  });

  it("answers with an empty board", async () => {
    vi.stubEnv("DEV", false);
    vi.resetModules();
    const board = await import("./board");
    await expect(board.getBoard("demo")).resolves.toEqual({ columns: [] });
  });
});

describe("board queries", () => {
  it("cache the board under the board key", async () => {
    const client = new QueryClient();
    const loading = client.query(boardQuery("demo"));
    await vi.runAllTimersAsync();
    const board = await loading;
    expect(client.getQueryData(keys.board("demo"))).toEqual(board);
  });

  it("move a card to its new column when a status event arrives", async () => {
    const client = new QueryClient();
    client.setQueryData(keys.board("demo"), await demoBoard());
    const event: StoredEvent = {
      kind: "event",
      seq: 1,
      type: "task.status_changed",
      actor: "frontend/ui",
      task: "T-1",
      proposal: null,
      workspace: null,
      revision: null,
      data: { status: "in_progress" },
      created_at: 0,
    };
    applyEvent(client, "demo", event);
    const board = client.getQueryData<Board>(keys.board("demo"));
    const columns = groupColumns(board ? boardCards(board) : []);
    const tasks = (status: string) =>
      columns.find((c) => c.status === status)?.cards.map((c) => c.task);
    expect(tasks("todo")).not.toContain("T-1");
    expect(tasks("in_progress")?.[0]).toBe("T-1");
    expect(client.getQueryState(keys.board("demo"))?.isInvalidated).toBe(false);
  });
});

describe("the fixture event stream", () => {
  it("moves T-1 from Todo through In Progress to Review with no refetch", async () => {
    const client = new QueryClient();
    client.setQueryData(keys.board("demo"), await demoBoard());
    const column = () => {
      const board = client.getQueryData<Board>(keys.board("demo"));
      return board
        ? groupColumns(boardCards(board)).find((c) =>
            c.cards.some((card) => card.task === "T-1"),
          )?.status
        : undefined;
    };
    expect(column()).toBe("todo");
    const handle = connectProject(
      client,
      "demo",
      fixtureEventTransport("demo", 10, 60_000),
      createLiveStatusStore(),
    );
    // Each scripted frame follows a 10 ms step; the status events are the second and the fourth frame.
    await vi.advanceTimersByTimeAsync(25);
    expect(column()).toBe("in_progress");
    await vi.advanceTimersByTimeAsync(20);
    expect(column()).toBe("review");
    expect(client.getQueryState(keys.board("demo"))?.isInvalidated).toBe(false);
    handle.stop();
  });
});
