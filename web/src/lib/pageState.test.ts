// @vitest-environment jsdom
import { act, cleanup, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { useMediaQuery, usePageVisible } from "./pageState";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

function fakeMedia(initial: boolean) {
  const listeners = new Set<() => void>();
  const list = {
    matches: initial,
    addEventListener: (_: string, listener: () => void) => {
      listeners.add(listener);
    },
    removeEventListener: (_: string, listener: () => void) => {
      listeners.delete(listener);
    },
  };
  vi.stubGlobal(
    "matchMedia",
    vi.fn(() => list),
  );
  return {
    listeners,
    set(matches: boolean) {
      list.matches = matches;
      for (const listener of listeners) listener();
    },
  };
}

describe("useMediaQuery", () => {
  it("follows the query as the window changes and stops listening on unmount", () => {
    const media = fakeMedia(false);
    const { result, unmount } = renderHook(() =>
      useMediaQuery("(min-width: 64rem)"),
    );
    expect(result.current).toBe(false);
    act(() => {
      media.set(true);
    });
    expect(result.current).toBe(true);
    unmount();
    expect(media.listeners.size).toBe(0);
  });

  it("is false without matchMedia", () => {
    vi.stubGlobal("matchMedia", undefined);
    const { result } = renderHook(() => useMediaQuery("(min-width: 64rem)"));
    expect(result.current).toBe(false);
  });
});

describe("usePageVisible", () => {
  it("follows the tab's visibility", () => {
    const state = vi
      .spyOn(document, "visibilityState", "get")
      .mockReturnValue("hidden");
    const { result } = renderHook(() => usePageVisible());
    expect(result.current).toBe(false);
    state.mockReturnValue("visible");
    act(() => {
      document.dispatchEvent(new Event("visibilitychange"));
    });
    expect(result.current).toBe(true);
  });
});
