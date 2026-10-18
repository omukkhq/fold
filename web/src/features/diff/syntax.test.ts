import { describe, expect, it } from "vitest";
import type { SyntaxLine } from "@/syntax/highlight";
import { add, ctx, del, hunk } from "@/test/diff";
import { collapseRuns, hunkRows, type DiffRow } from "./layout";
import { diffSyntax, rowTokens } from "./syntax";

const ROWS: DiffRow[] = hunkRows([
  hunk(1, 1, [ctx("a"), del("b"), add("B"), ctx("c")]),
  hunk(10, 10, [add("x")]),
]);

describe("diffSyntax", () => {
  it("splits each hunk into its old side and its new side", () => {
    const { request } = diffSyntax(ROWS, "file:a.ts");
    expect(request.language).toBe("file:a.ts");
    expect(request.segments).toEqual([
      ["a", "b", "c"],
      ["a", "B", "c"],
      [],
      ["x"],
    ]);
  });

  it("places a removed line on the old side and every other line on the new side", () => {
    const { at } = diffSyntax(ROWS, "file:a.ts");
    expect(Object.fromEntries(at)).toEqual({
      "h0-0": [1, 0],
      "h0-1": [0, 1],
      "h0-2": [1, 1],
      "h0-3": [1, 2],
      "h1-0": [3, 0],
    });
  });

  it("takes only the rows on screen", () => {
    const lines = ["1", "2", "3", "4", "5", "6", "7", "8", "9", "10"];
    const rows = collapseRuns(
      hunkRows([hunk(1, 1, [add("new"), ...lines.map(ctx)])]),
      new Set(),
    );
    const { request } = diffSyntax(rows, "file:main.rs");
    expect(request.segments[1]).toEqual(["new", "1", "2", "3"]);
  });

  it("keys the request by the shown code", () => {
    const again = hunkRows([
      hunk(1, 1, [ctx("a"), del("b"), add("B"), ctx("c")]),
      hunk(10, 10, [add("x")]),
    ]);
    const other = hunkRows([hunk(1, 1, [ctx("a")])]);
    const key = diffSyntax(ROWS, "file:main.go").request.key;
    expect(diffSyntax(again, "file:main.go").request.key).toBe(key);
    expect(diffSyntax(other, "file:main.go").request.key).not.toBe(key);
  });
});

describe("rowTokens", () => {
  const syntax = diffSyntax(ROWS, "file:a.ts");
  const line = (content: string): SyntaxLine => [{ content, color: null }];
  const tokens = [
    [line("a"), line("b"), line("c")],
    [line("a"), line("B"), line("c")],
    [],
    [line("x")],
  ];

  it("finds a row's line in its side", () => {
    expect(rowTokens(syntax, tokens, "h0-1")).toEqual(line("b"));
    expect(rowTokens(syntax, tokens, "h0-2")).toEqual(line("B"));
    expect(rowTokens(syntax, tokens, "h1-0")).toEqual(line("x"));
  });

  it("is null while the diff is plain, for an unknown row and for a missing line", () => {
    expect(rowTokens(syntax, null, "h0-1")).toBeNull();
    expect(rowTokens(syntax, tokens, "h9-9")).toBeNull();
    expect(rowTokens(syntax, [[], [], [], []], "h1-0")).toBeNull();
  });
});
