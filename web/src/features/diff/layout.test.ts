import { describe, expect, it } from "vitest";
import { add, comparedFile as file, ctx, del, hunk } from "@/test/diff";
import {
  collapseRuns,
  currentFile,
  lineText,
  shownEntry,
  fileBody,
  fileTree,
  hunkHeader,
  hunkRows,
  layoutFile,
  listedFiles,
  pairRenames,
  threadEnds,
  type DiffRow,
  type ExpanderRow,
  type LineRow,
} from "./layout";

const lines = (rows: DiffRow[]) =>
  rows.filter((r): r is LineRow => r.type === "line");

const context = (n: number, from = 1) =>
  Array.from({ length: n }, (_, i) => ctx(`c${String(from + i)}`));

describe("hunkHeader", () => {
  it("writes both ranges with U+2212", () => {
    expect(hunkHeader(hunk(12, 12, [ctx("a"), add("b")]))).toBe(
      "@@ −12,1 +12,2 @@",
    );
  });
});

describe("lineText", () => {
  it("drops the line terminator the daemon keeps, and only that", () => {
    expect(lineText("a\n")).toBe("a");
    expect(lineText("a\r\n")).toBe("a");
    expect(lineText("a")).toBe("a");
    expect(lineText("  \n\n")).toBe("  \n");
  });
});

describe("line rows", () => {
  it("take the numbers the daemon sent, not a count of their own", () => {
    const [row] = hunkRows([
      {
        old_start: 1,
        old_lines: 1,
        new_start: 1,
        new_lines: 1,
        lines: [
          {
            kind: "context",
            old_no: 7,
            new_no: 9,
            text: "x\n",
            no_newline: false,
          },
        ],
      },
    ]).filter((r): r is LineRow => r.type === "line");
    expect(row).toMatchObject({ oldNo: 7, newNo: 9, text: "x" });
  });

  it("carry the last line's missing terminator", () => {
    const h = hunk(1, 1, [ctx("a"), add("b")]);
    const last = h.lines[1];
    if (last) {
      last.text = "b";
      last.no_newline = true;
    }
    expect(lines(hunkRows([h])).map((r) => [r.text, r.noNewline])).toEqual([
      ["a", false],
      ["b", true],
    ]);
  });
});

describe("line numbers", () => {
  it("counts each side from the hunk's start and skips the side a line is not on", () => {
    const rows = hunkRows([
      hunk(12, 12, [ctx("a"), del("b"), del("c"), add("d"), ctx("e")]),
    ]);
    expect(rows[0]).toMatchObject({ type: "hunk" });
    expect(lines(rows).map((r) => [r.kind, r.oldNo, r.newNo])).toEqual([
      ["context", 12, 12],
      ["removed", 13, null],
      ["removed", 14, null],
      ["added", null, 13],
      ["context", 15, 14],
    ]);
  });

  it("starts each hunk again from its own start", () => {
    const rows = hunkRows([hunk(1, 1, [add("a")]), hunk(40, 41, [ctx("b")])]);
    expect(rows.map((r) => r.type)).toEqual(["hunk", "line", "hunk", "line"]);
    expect(lines(rows)[1]).toMatchObject({ oldNo: 40, newNo: 41 });
  });

  it("numbers an added file from 1 on the new side only", () => {
    const rows = lines(hunkRows([hunk(0, 1, [add("a"), add("b")])]));
    expect(rows.map((r) => [r.oldNo, r.newNo])).toEqual([
      [null, 1],
      [null, 2],
    ]);
  });

  it("gives every row a unique key", () => {
    const rows = hunkRows([
      hunk(1, 1, [ctx("a"), ctx("b")]),
      hunk(9, 9, [ctx("c")]),
    ]);
    expect(new Set(rows.map((r) => r.key)).size).toBe(rows.length);
  });
});

describe("collapseRuns", () => {
  const options = { threshold: 4, edge: 1 };

  it("keeps a run at the threshold", () => {
    const rows = hunkRows([hunk(1, 1, [add("x"), ...context(4), add("y")])]);
    expect(collapseRuns(rows, new Set(), options)).toEqual(rows);
  });

  it("hides the middle of a run between changes and keeps the edge on each side", () => {
    const rows = hunkRows([hunk(1, 1, [add("x"), ...context(6), add("y")])]);
    const out = collapseRuns(rows, new Set(), options);
    expect(out.map((r) => (r.type === "line" ? r.text : r.type))).toEqual([
      "hunk",
      "x",
      "c1",
      "expander",
      "c6",
      "y",
    ]);
    expect(out[3]).toMatchObject({ type: "expander", hidden: 4 });
  });

  it("keeps lines only on the changed side of a run at the start or the end of a hunk", () => {
    const rows = hunkRows([
      hunk(1, 1, [...context(6), add("x"), ...context(6, 7)]),
    ]);
    const out = collapseRuns(rows, new Set(), options);
    expect(out.map((r) => (r.type === "line" ? r.text : r.type))).toEqual([
      "hunk",
      "expander",
      "c6",
      "x",
      "c7",
      "expander",
    ]);
    expect(
      out
        .filter((r): r is ExpanderRow => r.type === "expander")
        .map((r) => r.hidden),
    ).toEqual([5, 5]);
  });

  it("collapses a hunk that is all context into one expander", () => {
    const rows = hunkRows([hunk(1, 1, context(6))]);
    const out = collapseRuns(rows, new Set(), options);
    expect(out.map((r) => r.type)).toEqual(["hunk", "expander"]);
  });

  it("shows every line of an expander the reader opened", () => {
    const rows = hunkRows([hunk(1, 1, [add("x"), ...context(6), add("y")])]);
    const collapsed = collapseRuns(rows, new Set(), options);
    const expander = collapsed.find((r) => r.type === "expander");
    expect(expander).toBeDefined();
    const opened = collapseRuns(rows, new Set([expander?.key ?? ""]), options);
    expect(opened).toEqual(rows);
  });

  it("keeps open a run that holds the end of a thread on either side", () => {
    const rows = hunkRows([hunk(1, 1, [add("x"), ...context(6), add("y")])]);
    // c3 is old line 3 and new line 4.
    for (const end of ["new:4", "old:3"]) {
      expect(
        collapseRuns(rows, new Set(), { ...options, keep: new Set([end]) }),
      ).toEqual(rows);
    }
    expect(
      collapseRuns(rows, new Set(), {
        ...options,
        keep: new Set(["old:4"]),
      }).some((r) => r.type === "expander"),
    ).toBe(false);
    expect(
      collapseRuns(rows, new Set(), {
        ...options,
        keep: new Set(["new:9"]),
      }).some((r) => r.type === "expander"),
    ).toBe(true);
  });

  it("uses the default threshold of eight and edge of three", () => {
    const rows = hunkRows([hunk(1, 1, [add("x"), ...context(8), add("y")])]);
    expect(collapseRuns(rows, new Set())).toEqual(rows);
    const longer = hunkRows([hunk(1, 1, [add("x"), ...context(9), add("y")])]);
    const out = collapseRuns(longer, new Set());
    expect(out.find((r) => r.type === "expander")).toMatchObject({ hidden: 3 });
  });
});

describe("threadEnds", () => {
  it("names the end line and side of each thread on the file", () => {
    const range = (path: string, side: string, end_line: number) => ({
      range: { path, side, end_line },
    });
    expect(
      threadEnds(
        [
          range("a.rs", "new", 4),
          range("a.rs", "old", 7),
          range("b.rs", "new", 1),
        ],
        "a.rs",
      ),
    ).toEqual(new Set(["new:4", "old:7"]));
  });
});

describe("layoutFile", () => {
  const hunks = [
    hunk(
      0,
      1,
      Array.from({ length: 10 }, (_, i) => add(String(i))),
    ),
  ];

  it("shows every row under the limit", () => {
    const layout = layoutFile(hunks, new Set(), 20);
    expect(layout.rows).toHaveLength(11);
    expect(layout.remaining).toBe(0);
  });

  it("shows the first rows of a file over the limit and counts the rest", () => {
    const layout = layoutFile(hunks, new Set(), 4);
    expect(layout.rows).toHaveLength(4);
    expect(layout.remaining).toBe(7);
  });

  it("limits a large file at the default row limit", () => {
    const big = [
      hunk(
        0,
        1,
        Array.from({ length: 1200 }, (_, i) => add(String(i))),
      ),
    ];
    expect(layoutFile(big, new Set()).remaining).toBe(201);
  });
});

describe("pairRenames", () => {
  it("pairs a deleted and an added file with the same hash into one rename in the added file's place", () => {
    const files = [
      file("docs/old.md", "deleted", { old_hash: "h1" }),
      file("src/a.ts", "modified"),
      file("docs/new.md", "added", { new_hash: "h1" }),
    ];
    expect(pairRenames(files)).toEqual([
      files[1],
      {
        ...files[2],
        change: "renamed",
        previous_path: "docs/old.md",
        old_hash: "h1",
        added_lines: 0,
        removed_lines: 0,
      },
    ]);
  });

  it("leaves files with different hashes apart", () => {
    const files = [
      file("a.md", "deleted", { old_hash: "h1" }),
      file("b.md", "added", { new_hash: "h2" }),
    ];
    expect(pairRenames(files)).toEqual(files);
  });

  it("pairs each deleted file with one added file only", () => {
    const files = [
      file("a.md", "deleted", { old_hash: "h1" }),
      file("b.md", "added", { new_hash: "h1" }),
      file("c.md", "added", { new_hash: "h1" }),
    ];
    const out = pairRenames(files);
    expect(out.map((f) => [f.path, f.change])).toEqual([
      ["b.md", "renamed"],
      ["c.md", "added"],
    ]);
  });

  it("keeps a rename the daemon already reported", () => {
    const renamed = file("b.ts", "renamed", { previous_path: "a.ts" });
    expect(pairRenames([renamed])).toEqual([renamed]);
  });
});

describe("fileTree", () => {
  it("puts directories before files, each by name, and joins a directory chain into one entry", () => {
    const tree = fileTree([
      file("src/routes/login.tsx", "modified"),
      file("README.md", "modified"),
      file("src/routes/a.tsx", "added"),
      file("docs/guide/deep/x.md", "added"),
      file("src/api.ts", "modified"),
    ]);
    const shape = (nodes: typeof tree): unknown[] =>
      nodes.map((n) =>
        n.type === "dir" ? [n.path, shape(n.children)] : n.name,
      );
    expect(shape(tree)).toEqual([
      ["docs/guide/deep", ["x.md"]],
      ["src", [["src/routes", ["a.tsx", "login.tsx"]], "api.ts"]],
      "README.md",
    ]);
  });

  it("lists the files in tree order with renames paired", () => {
    const listed = listedFiles([
      file("src/b.ts", "modified"),
      file("old/x.md", "deleted", { old_hash: "h" }),
      file("src/a/z.ts", "modified"),
      file("new/x.md", "added", { new_hash: "h" }),
    ]);
    expect(listed.map((f) => [f.path, f.change])).toEqual([
      ["new/x.md", "renamed"],
      ["src/a/z.ts", "modified"],
      ["src/b.ts", "modified"],
    ]);
  });
});

describe("currentFile", () => {
  const files = [file("a.ts", "modified"), file("b.ts", "modified")];

  it("takes the file the address names", () => {
    expect(currentFile(files, "b.ts")?.path).toBe("b.ts");
  });

  it("falls back to the first file", () => {
    expect(currentFile(files, undefined)?.path).toBe("a.ts");
    expect(currentFile(files, "gone.ts")?.path).toBe("a.ts");
    expect(currentFile([], undefined)).toBeNull();
  });
});

describe("fileBody", () => {
  it("shows hashes for a binary file", () => {
    expect(fileBody(file("a.png", "modified", { binary: true }))).toBe(
      "binary",
    );
  });

  it("shows a note for a rename with the same content", () => {
    expect(
      fileBody(file("b.ts", "renamed", { old_hash: "h", new_hash: "h" })),
    ).toBe("unchanged");
  });

  it("shows a note for a mode change with the same content", () => {
    expect(
      fileBody(
        file("run.sh", "mode_changed", { old_hash: "h", new_hash: "h" }),
      ),
    ).toBe("unchanged");
  });

  it("shows hunks for a rename with changes and for every other text file", () => {
    expect(fileBody(file("b.ts", "renamed"))).toBe("text");
    expect(fileBody(file("b.ts", "deleted"))).toBe("text");
  });
});

describe("shownEntry", () => {
  const files = [file("a.ts", "modified"), file("b.ts", "added")];
  const unstable = { path: "noisy.log", reason: "unstable" as const };

  it("shows the file the address names, else the first", () => {
    expect(shownEntry(files, [], "b.ts")).toEqual({
      kind: "file",
      file: files[1],
    });
    expect(shownEntry(files, [unstable], undefined)).toEqual({
      kind: "file",
      file: files[0],
    });
  });

  it("shows the warning of a warned path the address names", () => {
    expect(shownEntry(files, [unstable], "noisy.log")).toEqual({
      kind: "warning",
      warning: unstable,
    });
  });

  it("shows a warned file as its warning, never as an empty diff", () => {
    const warned = { path: "a.ts", reason: "unstable" as const };
    expect(shownEntry(files, [warned], "a.ts")).toEqual({
      kind: "warning",
      warning: warned,
    });
  });

  it("shows the first warning when no file changed, and nothing when nothing is listed", () => {
    expect(shownEntry([], [unstable], undefined)).toEqual({
      kind: "warning",
      warning: unstable,
    });
    expect(shownEntry([], [], undefined)).toBeNull();
  });
});
