// @vitest-environment jsdom
import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ACTIVITY_TICK_MS, useActivityClock, useNow } from "./useNow";

beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(1000);
});

afterEach(() => {
  vi.useRealTimers();
});

describe("useNow", () => {
  it("ticks while enabled and stops when disabled", () => {
    const { result, rerender } = renderHook(
      ({ on }: { on: boolean }) => useNow(on, 1000),
      { initialProps: { on: true } },
    );
    expect(result.current).toBe(1000);
    act(() => {
      vi.advanceTimersByTime(2000);
    });
    expect(result.current).toBe(3000);
    rerender({ on: false });
    act(() => {
      vi.advanceTimersByTime(5000);
    });
    expect(result.current).toBe(3000);
  });

  it("reads the clock again when it is enabled", () => {
    const { result, rerender } = renderHook(
      ({ on }: { on: boolean }) => useNow(on),
      { initialProps: { on: false } },
    );
    vi.setSystemTime(9000);
    rerender({ on: true });
    act(() => {
      vi.advanceTimersByTime(0);
    });
    expect(result.current).toBe(9000);
  });
});

describe("useActivityClock", () => {
  it("keeps the screen's time while nothing ticks, and moves each minute while an activity shows", () => {
    const { result, rerender } = renderHook(
      ({ base, on }: { base: number; on: boolean }) =>
        useActivityClock(base, on),
      { initialProps: { base: 500, on: false } },
    );
    expect(result.current).toBe(500);
    rerender({ base: 500, on: true });
    act(() => {
      vi.advanceTimersByTime(0);
    });
    expect(result.current).toBe(1000);
    act(() => {
      vi.advanceTimersByTime(ACTIVITY_TICK_MS);
    });
    expect(result.current).toBe(1000 + ACTIVITY_TICK_MS);
    rerender({ base: 10 * ACTIVITY_TICK_MS, on: true });
    expect(result.current).toBe(10 * ACTIVITY_TICK_MS);
  });
});
