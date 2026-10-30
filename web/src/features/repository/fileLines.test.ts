import { describe, expect, it } from "vitest";
import { pointsOf } from "@/features/diff/selection";
import { fileRows, firstRowLimit, openedSelection } from "./fileLines";

describe("fileRows", () => {
  it("numbers each line from 1 on the new side", () => {
    const rows = fileRows("a\nb\r\nc\n");
    expect(rows.map((r) => [r.key, r.newNo, r.oldNo, r.text])).toEqual([
      ["l1", 1, null, "a"],
      ["l2", 2, null, "b"],
      ["l3", 3, null, "c"],
    ]);
    expect(rows.every((r) => r.kind === "context" && !r.noNewline)).toBe(true);
    expect(pointsOf(rows)).toEqual([
      { side: "new", line: 1 },
      { side: "new", line: 2 },
      { side: "new", line: 3 },
    ]);
  });

  it("marks a last line without a terminator, and an empty file has no rows", () => {
    const rows = fileRows("a\nb");
    expect(rows.map((r) => r.noNewline)).toEqual([false, true]);
    expect(fileRows("")).toEqual([]);
    expect(fileRows("\n").map((r) => r.text)).toEqual([""]);
  });

  it("ends a line at a lone carriage return too, as the daemon counts the lines of a request", () => {
    const rows = fileRows("a\rb\r\nc\r");
    expect(rows.map((r) => [r.newNo, r.text])).toEqual([
      [1, "a"],
      [2, "b"],
      [3, "c"],
    ]);
    expect(rows.at(-1)?.noNewline).toBe(false);
    expect(fileRows("a\r\rb").map((r) => r.text)).toEqual(["a", "", "b"]);
  });
});

describe("opening a file at a line", () => {
  it("shows as many pages of rows as reach the line", () => {
    expect(firstRowLimit(undefined)).toBe(1000);
    expect(firstRowLimit(1)).toBe(1000);
    expect(firstRowLimit(1000)).toBe(1000);
    expect(firstRowLimit(1001)).toBe(2000);
    expect(firstRowLimit(2500)).toBe(3000);
  });

  it("selects the line while the file holds it", () => {
    const rows = fileRows("a\nb\n");
    expect(openedSelection("a.txt", 2, rows)).toEqual({
      path: "a.txt",
      side: "new",
      start_line: 2,
      end_line: 2,
    });
    expect(openedSelection("a.txt", 3, rows)).toBeNull();
    expect(openedSelection("a.txt", undefined, rows)).toBeNull();
  });
});
