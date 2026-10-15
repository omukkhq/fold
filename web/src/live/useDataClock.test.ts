// @vitest-environment jsdom
import { renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { dataTime, useDataClock } from "./useDataClock";

beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(1000);
});

afterEach(() => {
  vi.useRealTimers();
});

describe("dataTime", () => {
  it("takes the later of the current time and the data", () => {
    expect(dataTime([500, null, undefined], 1000)).toBe(1000);
    expect(dataTime([500, 2500], 1000)).toBe(2500);
    expect(dataTime([])).toBe(1000);
  });
});

describe("useDataClock", () => {
  it("reads the time only when the data or a renewal changes", () => {
    const { result, rerender } = renderHook(
      ({ at, retry }: { at: number; retry: number | null }) =>
        useDataClock([at], [retry]),
      { initialProps: { at: 400, retry: null as number | null } },
    );
    expect(result.current).toBe(1000);
    vi.setSystemTime(5000);
    rerender({ at: 400, retry: null });
    expect(result.current).toBe(1000);
    rerender({ at: 4000, retry: null });
    expect(result.current).toBe(5000);
    vi.setSystemTime(6000);
    rerender({ at: 4000, retry: 90_000 });
    expect(result.current).toBe(6000);
  });

  it("never reads earlier than the newest data", () => {
    const { result, rerender } = renderHook(
      ({ at }: { at: number }) => useDataClock([at]),
      { initialProps: { at: 400 } },
    );
    rerender({ at: 9000 });
    expect(result.current).toBe(9000);
  });
});
