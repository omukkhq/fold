import { describe, expect, it, vi } from "vitest";
import type { DurableEvent } from "@/api/events";
import { FEED_LIMIT } from "./feed";
import { HISTORY_PAGES, loadMoves, type HistoryPage } from "./history";

function change(seq: number, task = `T-${String(seq)}`): DurableEvent {
  return {
    seq,
    type: "task.status_changed",
    actor: "fold",
    task,
    proposal: null,
    workspace: null,
    revision: null,
    data: { from: "todo", to: "done" },
    created_at: seq * 1_000,
  };
}

function other(seq: number): DurableEvent {
  return { ...change(seq), type: "checkpoint.recorded", data: {} };
}

/** Pages of `size` events, newest first, from `newest` down to 1. */
function pages(
  newest: number,
  size: number,
  event: (seq: number) => DurableEvent,
) {
  return vi.fn((before: number | null): Promise<HistoryPage> => {
    const top = before === null ? newest : before - 1;
    const events: DurableEvent[] = [];
    for (let seq = top; seq > Math.max(0, top - size); seq -= 1) {
      events.push(event(seq));
    }
    const last = events.at(-1)?.seq ?? 1;
    return Promise.resolve({ events, before: last > 1 ? last : null });
  });
}

describe("loadMoves", () => {
  it("stops at the first page that holds ten moves", async () => {
    const read = pages(100, 20, change);
    const moves = await loadMoves(read, () => true);
    expect(moves.map((move) => move.seq)).toEqual(
      Array.from({ length: FEED_LIMIT }, (_, i) => 100 - i),
    );
    expect(read).toHaveBeenCalledTimes(1);
  });

  it("reads older pages for the moves it keeps and stops at the first event", async () => {
    const read = pages(30, 10, (seq) =>
      seq % 3 === 0 ? change(seq) : other(seq),
    );
    const moves = await loadMoves(read, (move) => move.seq !== 30);
    expect(moves.map((move) => move.seq)).toEqual([
      27, 24, 21, 18, 15, 12, 9, 6, 3,
    ]);
    expect(read.mock.calls.map(([before]) => before)).toEqual([null, 21, 11]);
  });

  it("reads at most its page count", async () => {
    const read = pages(10_000, 10, (seq) =>
      seq === 9_995 ? change(seq) : other(seq),
    );
    const moves = await loadMoves(read, () => true);
    expect(moves.map((move) => move.seq)).toEqual([9_995]);
    expect(read).toHaveBeenCalledTimes(HISTORY_PAGES);
  });
});
