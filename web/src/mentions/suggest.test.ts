import { describe, expect, it } from "vitest";
import {
  activeMention,
  applyPick,
  groupRuns,
  itemSuggestions,
  pathQuery,
  pathSuggestions,
  stepIndex,
} from "./suggest";

describe("activeMention", () => {
  it("finds the mark, the query and the span around the caret", () => {
    const text = "data of #rep and more";
    expect(activeMention(text, 12)).toEqual({
      mark: "#",
      query: "rep",
      start: 8,
      end: 12,
    });
    expect(activeMention("see @web/sr", 11)).toEqual({
      mark: "@",
      query: "web/sr",
      start: 4,
      end: 11,
    });
  });

  it("covers the rest of the word when the caret sits inside it", () => {
    expect(activeMention("#T-12", 2)).toEqual({
      mark: "#",
      query: "T",
      start: 0,
      end: 5,
    });
  });

  it("finds nothing without a mark, inside a word or inside code", () => {
    expect(activeMention("plain text", 5)).toBeNull();
    expect(activeMention("a@b", 3)).toBeNull();
    expect(activeMention("page#T", 6)).toBeNull();
    expect(activeMention("`#T`", 3)).toBeNull();
    expect(activeMention("x # y", 4)).toBeNull();
  });

  it("opens on a bare mark", () => {
    expect(activeMention("go #", 4)).toEqual({
      mark: "#",
      query: "",
      start: 3,
      end: 4,
    });
  });
});

describe("applyPick", () => {
  const active = { mark: "#" as const, query: "rep", start: 4, end: 8 };

  it("writes the pick with a space after it", () => {
    expect(applyPick("see #rep", active, "T-9", false)).toEqual({
      text: "see #T-9 ",
      caret: 9,
    });
  });

  it("reuses the space that follows", () => {
    expect(applyPick("see #rep now", active, "T-9", false)).toEqual({
      text: "see #T-9 now",
      caret: 9,
    });
  });

  it("leaves a directory open for the next part", () => {
    const at = { mark: "@" as const, query: "we", start: 0, end: 3 };
    expect(applyPick("@we", at, "web/", true)).toEqual({
      text: "@web/",
      caret: 5,
    });
  });
});

describe("itemSuggestions", () => {
  const sources = {
    tasks: [
      { id: "T-9", title: "Reports page" },
      { id: "T-10", title: "Login" },
    ],
    proposals: [{ id: "P-3", title: "Report page layout" }],
    requests: [{ id: "RQ-4", title: "Report export" }],
  };

  it("matches the start of the id or any part of the title, grouped", () => {
    expect(itemSuggestions("rep", sources).map((s) => s.insert)).toEqual([
      "T-9",
      "P-3",
      "RQ-4",
    ]);
    expect(itemSuggestions("t-1", sources).map((s) => s.label)).toEqual([
      "T-10",
    ]);
  });

  it("lists at most five per group and skips a list the screen does not hold", () => {
    const many = Array.from({ length: 8 }, (_, i) => ({
      id: `T-${String(i + 1)}`,
      title: "x",
    }));
    const out = itemSuggestions("", { tasks: many });
    expect(out).toHaveLength(5);
    expect(out.every((s) => s.group === "tasks" && !s.open)).toBe(true);
  });
});

describe("pathQuery", () => {
  it("splits the directory from the name being typed", () => {
    expect(pathQuery("web/src/bo")).toEqual({
      workspace: null,
      dir: "web/src",
      leaf: "bo",
    });
    expect(pathQuery("we")).toEqual({ workspace: null, dir: "", leaf: "we" });
    expect(pathQuery("ws-9:src/")).toEqual({
      workspace: "ws-9",
      dir: "src",
      leaf: "",
    });
  });

  it("refuses a directory that climbs or doubles a slash", () => {
    expect(pathQuery("../x")).toBeNull();
    expect(pathQuery("a//b")).toBeNull();
  });
});

describe("pathSuggestions", () => {
  const entries = [
    { name: "board.ts", path: "web/src/board.ts", kind: "file" },
    { name: "boot", path: "web/src/boot", kind: "directory" },
    { name: "app.ts", path: "web/src/app.ts", kind: "file" },
  ];

  it("offers the entries that start with the typed name; a directory stays open", () => {
    const out = pathSuggestions(
      { workspace: null, dir: "web/src", leaf: "Bo" },
      entries,
    );
    expect(out.map((s) => [s.insert, s.label, s.open])).toEqual([
      ["web/src/board.ts", "board.ts", false],
      ["web/src/boot/", "boot/", true],
    ]);
  });

  it("keeps the workspace in what it writes", () => {
    const [first] = pathSuggestions(
      { workspace: "ws-9", dir: "web/src", leaf: "app" },
      entries,
    );
    expect(first?.insert).toBe("ws-9:web/src/app.ts");
  });
});

describe("stepIndex and groupRuns", () => {
  it("wraps at both ends", () => {
    expect(stepIndex(2, 3, 1)).toBe(0);
    expect(stepIndex(0, 3, -1)).toBe(2);
    expect(stepIndex(0, 0, 1)).toBe(0);
  });

  it("groups consecutive suggestions under one heading with their indexes", () => {
    const out = itemSuggestions("", {
      tasks: [
        { id: "T-1", title: "a" },
        { id: "T-2", title: "b" },
      ],
      revisions: [{ id: "r3", title: "" }],
    });
    expect(
      groupRuns(out).map((run) => [run.group, run.items.map((i) => i.index)]),
    ).toEqual([
      ["tasks", [0, 1]],
      ["revisions", [2]],
    ]);
  });
});
