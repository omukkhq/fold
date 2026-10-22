import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { BATCH_WINDOW_MS, createBatcher } from "./batch";

beforeEach(() => {
  vi.useFakeTimers();
});
afterEach(() => {
  vi.useRealTimers();
});

describe("createBatcher", () => {
  it("flushes every item of one window at once", () => {
    const flush = vi.fn();
    const batch = createBatcher<number>(flush);
    batch.add(1);
    vi.advanceTimersByTime(500);
    batch.add(2);
    batch.add(3);
    vi.advanceTimersByTime(BATCH_WINDOW_MS - 501);
    expect(flush).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(flush).toHaveBeenCalledExactlyOnceWith([1, 2, 3]);
  });

  it("does not stretch the window for a steady flow", () => {
    const flush = vi.fn();
    const batch = createBatcher<number>(flush, 1_000);
    for (let i = 0; i < 6; i += 1) {
      batch.add(i);
      vi.advanceTimersByTime(400);
    }
    expect(flush.mock.calls).toEqual([[[0, 1, 2]], [[3, 4, 5]]]);
  });

  it("drops the waiting items on cancel", () => {
    const flush = vi.fn();
    const batch = createBatcher<number>(flush);
    batch.cancel();
    batch.add(1);
    batch.cancel();
    vi.advanceTimersByTime(BATCH_WINDOW_MS * 2);
    expect(flush).not.toHaveBeenCalled();
    batch.add(2);
    vi.advanceTimersByTime(BATCH_WINDOW_MS);
    expect(flush).toHaveBeenCalledExactlyOnceWith([2]);
  });
});
