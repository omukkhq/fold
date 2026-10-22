import { describe, expect, it } from "vitest";
import { comparedFile } from "@/test/diff";
import {
  readViewed,
  selectFile,
  showFile,
  shownPath,
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

describe("opening a file marks it viewed", () => {
  const none = { opened: null, viewed: new Set<string>() };

  it("marks the first file shown and each file the screen shows after it", () => {
    const a = showFile(none, "a.ts");
    expect(a).toEqual({ opened: "a.ts", viewed: new Set(["a.ts"]) });
    const b = showFile(a, "b.ts");
    expect(b).toEqual({ opened: "b.ts", viewed: new Set(["a.ts", "b.ts"]) });
    expect(showFile(none, null)).toBe(none);
  });

  it("keeps an unchecked file unviewed while it stays open, and marks it when it opens again", () => {
    const open = showFile(none, "a.ts");
    const unchecked = { ...open, viewed: toggleViewed(open.viewed, "a.ts") };
    expect(showFile(unchecked, "a.ts")).toBe(unchecked);
    const away = showFile(unchecked, "b.ts");
    expect(away.viewed).toEqual(new Set(["b.ts"]));
    expect(showFile(away, "a.ts").viewed).toEqual(new Set(["a.ts", "b.ts"]));
  });

  it("forgets the open file when the screen shows none, so the file marks when it comes back", () => {
    const open = showFile(none, "a.ts");
    const unchecked = { ...open, viewed: new Set<string>() };
    const gone = showFile(unchecked, null);
    expect(gone).toEqual({ opened: null, viewed: new Set() });
    expect(showFile(gone, "a.ts").viewed).toEqual(new Set(["a.ts"]));
  });

  it("keeps the marks of a file already viewed when it opens", () => {
    const marks = { opened: null, viewed: new Set(["a.ts"]) };
    const open = showFile(marks, "a.ts");
    expect(open.viewed).toBe(marks.viewed);
    expect(open.opened).toBe("a.ts");
  });

  it("marks a selected file again even when it is already open", () => {
    const open = showFile(none, "a.ts");
    expect(selectFile(open, "a.ts")).toBe(open);
    const unchecked = { ...open, viewed: new Set<string>() };
    expect(selectFile(unchecked, "a.ts")).toEqual(open);
    expect(selectFile(unchecked, "b.ts")).toEqual({
      opened: "b.ts",
      viewed: new Set(["b.ts"]),
    });
    expect(selectFile(unchecked, null)).toBe(unchecked);
  });
});

describe("the file whose diff the panel shows", () => {
  const a = comparedFile("src/a.ts", "modified");
  const b = comparedFile("src/b.ts", "added");

  it("is the selected file, or the first listed one", () => {
    const comparison = { files: [b, a], warnings: [] };
    expect(shownPath(comparison, "src/b.ts")).toBe("src/b.ts");
    expect(shownPath(comparison, undefined)).toBe("src/a.ts");
    expect(shownPath(comparison, "src/gone.ts")).toBe("src/a.ts");
  });

  it("is none when the panel shows a warning or nothing", () => {
    const unstable = { path: "src/a.ts", reason: "unstable" as const };
    expect(shownPath({ files: [a], warnings: [unstable] }, "src/a.ts")).toBe(
      null,
    );
    expect(shownPath({ files: [], warnings: [unstable] }, undefined)).toBe(
      null,
    );
    expect(shownPath({ files: [], warnings: [] }, undefined)).toBe(null);
  });
});
