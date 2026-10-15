// @vitest-environment jsdom
import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useNow } from "./useNow";

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
