// @vitest-environment jsdom
import { act, cleanup, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { useChangedFiles } from "./changedFilesState";

const KEY = "fold.viewed.demo.P-3.base..v1";

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  window.localStorage.clear();
});

describe("the reader's state of a change's file tree", () => {
  it("keeps the viewed marks in this browser and starts with every folder open", () => {
    window.localStorage.setItem(KEY, JSON.stringify(["src/a.ts"]));
    const { result } = renderHook(() => useChangedFiles(KEY, null));
    expect([...result.current.viewed]).toEqual(["src/a.ts"]);
    expect(result.current.closed.size).toBe(0);
    expect(result.current.filter).toBe("");

    act(() => {
      result.current.toggleViewed("src/b.ts");
      result.current.setFilter("b");
      result.current.setClosed(() => new Set(["src"]));
    });

    expect([...result.current.viewed].sort()).toEqual(["src/a.ts", "src/b.ts"]);
    expect(window.localStorage.getItem(KEY)).toBe('["src/a.ts","src/b.ts"]');
    expect(result.current.filter).toBe("b");
    expect([...result.current.closed]).toEqual(["src"]);
  });

  it("keeps the marks on screen when the browser gives no storage", () => {
    vi.spyOn(window, "localStorage", "get").mockImplementation(() => {
      throw new Error("blocked");
    });
    const { result } = renderHook(() => useChangedFiles(KEY, null));
    expect(result.current.viewed.size).toBe(0);

    act(() => {
      result.current.toggleViewed("src/a.ts");
    });

    expect([...result.current.viewed]).toEqual(["src/a.ts"]);
  });

  it("marks the file the screen opens, and keeps a file unchecked while it stays open", () => {
    const { result, rerender } = renderHook(
      ({ shown }: { shown: string | null }) => useChangedFiles(KEY, shown),
      { initialProps: { shown: "src/a.ts" } },
    );
    expect([...result.current.viewed]).toEqual(["src/a.ts"]);
    expect(window.localStorage.getItem(KEY)).toBe('["src/a.ts"]');

    act(() => {
      result.current.toggleViewed("src/a.ts");
    });
    rerender({ shown: "src/a.ts" });
    expect(result.current.viewed.size).toBe(0);
    expect(window.localStorage.getItem(KEY)).toBe("[]");

    rerender({ shown: "src/b.ts" });
    expect([...result.current.viewed]).toEqual(["src/b.ts"]);
    rerender({ shown: "src/a.ts" });
    expect([...result.current.viewed].sort()).toEqual(["src/a.ts", "src/b.ts"]);
    expect(window.localStorage.getItem(KEY)).toBe('["src/a.ts","src/b.ts"]');
  });

  it("marks the open file again when the reader selects it", () => {
    const { result } = renderHook(() => useChangedFiles(KEY, "src/a.ts"));
    act(() => {
      result.current.toggleViewed("src/a.ts");
    });
    expect(result.current.viewed.size).toBe(0);
    act(() => {
      result.current.selectFile("src/a.ts");
    });
    expect([...result.current.viewed]).toEqual(["src/a.ts"]);
  });
});
