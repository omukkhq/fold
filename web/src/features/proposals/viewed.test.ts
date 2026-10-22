import { describe, expect, it } from "vitest";
import {
  readViewed,
  toggleViewed,
  viewedKey,
  viewedText,
  writeViewed,
  type MarkStore,
} from "./viewed";

function memory(): MarkStore & { data: Map<string, string> } {
  const data = new Map<string, string>();
  return {
    data,
    getItem: (key) => data.get(key) ?? null,
    setItem: (key, value) => {
      data.set(key, value);
    },
  };
}

const refusing: MarkStore = {
  getItem: () => {
    throw new Error("blocked");
  },
  setItem: () => {
    throw new Error("blocked");
  },
};

describe("viewed marks", () => {
  it("names one change of one proposal", () => {
    expect(viewedKey("demo", "P-4", { version: 2, since: null })).toBe(
      "fold.viewed.demo.P-4.base..v2",
    );
    expect(viewedKey("demo", "P-4", { version: 2, since: 1 })).toBe(
      "fold.viewed.demo.P-4.v1..v2",
    );
  });

  it("writes and reads the viewed paths", () => {
    const store = memory();
    writeViewed(store, "k", new Set(["b.ts", "a.ts"]));
    expect(store.data.get("k")).toBe('["a.ts","b.ts"]');
    expect(readViewed(store, "k")).toEqual(new Set(["a.ts", "b.ts"]));
  });

  it("reads nothing from a missing, broken or refusing store", () => {
    const store = memory();
    expect(readViewed(store, "k")).toEqual(new Set());
    store.data.set("k", "{");
    expect(readViewed(store, "k")).toEqual(new Set());
    store.data.set("k", '{"a":1}');
    expect(readViewed(store, "k")).toEqual(new Set());
    store.data.set("k", '["a.ts", 3]');
    expect(readViewed(store, "k")).toEqual(new Set(["a.ts"]));
    expect(readViewed(null, "k")).toEqual(new Set());
    expect(readViewed(refusing, "k")).toEqual(new Set());
    expect(() => {
      writeViewed(refusing, "k", new Set(["a.ts"]));
    }).not.toThrow();
  });

  it("toggles a path and counts the listed files", () => {
    const viewed = toggleViewed(new Set(["a.ts"]), "b.ts");
    expect(viewed).toEqual(new Set(["a.ts", "b.ts"]));
    expect(toggleViewed(viewed, "a.ts")).toEqual(new Set(["b.ts"]));
    expect(viewedText(viewed, ["a.ts", "c.ts"])).toBe("1 of 2 viewed");
  });
});
