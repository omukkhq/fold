import { describe, expect, it } from "vitest";
import type { Hunk, HunkLine } from "@/api/diff";
import { add, ctx, del, hunk } from "@/test/diff";
import {
  EXPAND_STEP,
  addSpan,
  expandPart,
  fileGaps,
  fileLineSet,
  gapText,
  hasGaps,
  hiddenParts,
  keptSpans,
  layoutWithGaps,
  splitLines,
  textFits,
  withGaps,
  type ExpandHow,
  type Gap,
  type GapState,
  type Span,
} from "./gaps";
import {
  collapseRuns,
  hunkRows,
  type DiffRow,
  type GapRow,
  type LineRow,
} from "./layout";

const file = (count: number): string[] =>
  Array.from({ length: count }, (_, i) => `l${String(i + 1)}\n`);

/** Context lines of `lines` from `from` (1-based) to `to`, as drafts. */
const same = (lines: readonly string[], from: number, to: number) =>
  lines.slice(from - 1, to).map((l) => ctx(l.trimEnd()));

const ready = (lines: readonly string[]) =>
  ({ status: "ready", lines }) as const;

function state(extra: Partial<GapState> = {}): GapState {
  return { text: { status: "pending" }, spans: [], whole: false, ...extra };
}

function laid(hunks: Hunk[], s: GapState): DiffRow[] {
  return withGaps(collapseRuns(hunkRows(hunks), new Set()), hunks, s);
}

const gapRows = (rows: readonly DiffRow[]) =>
  rows.filter((r): r is GapRow => r.type === "gap");
const lineRows = (rows: readonly DiffRow[]) =>
  rows.filter((r): r is LineRow => r.type === "line");

// A 40-line file with line 10 changed and line 30 added after line 29: hunks 7–13 and 27–32 of the new side.
const OLD40 = file(39);
const NEW40 = [
  ...OLD40.slice(0, 9),
  "L10\n",
  ...OLD40.slice(10, 29),
  "added\n",
  ...OLD40.slice(29),
];
const HUNKS40 = [
  hunk(7, 7, [
    ...same(OLD40, 7, 9),
    del("l10"),
    add("L10"),
    ...same(OLD40, 11, 13),
  ]),
  hunk(27, 27, [...same(OLD40, 27, 29), add("added"), ...same(OLD40, 30, 32)]),
];

describe("splitLines", () => {
  it("keeps each terminator, counts a last line without one, and splits on a lone carriage return", () => {
    expect(splitLines("")).toEqual([]);
    expect(splitLines("a\n")).toEqual(["a\n"]);
    expect(splitLines("a\nb")).toEqual(["a\n", "b"]);
    expect(splitLines("a\r\nb\rc\n\n")).toEqual(["a\r\n", "b\r", "c\n", "\n"]);
  });
});

describe("fileGaps", () => {
  it("finds the gap above, between and below the hunks with the shift to old numbers", () => {
    expect(fileGaps(HUNKS40, 40)).toEqual([
      { index: 0, first: 1, last: 6, shift: 0, tail: false },
      { index: 1, first: 14, last: 26, shift: 0, tail: false },
      { index: 2, first: 34, last: 40, shift: -1, tail: true },
    ]);
  });

  it("leaves out the gap below the last hunk until the line count is known", () => {
    expect(fileGaps(HUNKS40, null)?.map((g) => g.index)).toEqual([0, 1]);
  });

  it("reads the edges of a side with no lines from its start", () => {
    // Two lines added at the top of a file whose old side has none in the hunk.
    expect(fileGaps([hunk(0, 1, [add("a"), add("b")])], 5)).toEqual([
      { index: 0, first: 1, last: 0, shift: 0, tail: false },
      { index: 1, first: 3, last: 5, shift: -2, tail: true },
    ]);
    // The last line removed: the new side ends before the hunk.
    expect(fileGaps([hunk(5, 4, [del("x")])], 4)).toEqual([
      { index: 0, first: 1, last: 4, shift: 0, tail: false },
      { index: 1, first: 5, last: 4, shift: 1, tail: true },
    ]);
  });

  it("is null when the hunks do not fit a file", () => {
    expect(fileGaps(HUNKS40, 20)).toBeNull();
    expect(fileGaps([hunk(3, 5, [ctx("a")])], 10)).toBeNull();
    expect(
      fileGaps([hunk(1, 5, [ctx("a")]), hunk(1, 3, [ctx("b")])], null),
    ).toBeNull();
  });

  it("has no gaps without hunks", () => {
    expect(fileGaps([], 10)).toEqual([]);
    expect(hasGaps([], 10)).toBe(false);
  });
});

describe("textFits and gapText", () => {
  const view = (text: string, extra: object = {}) =>
    ({
      status: "ready",
      file: { hash: "h", binary: false, too_large: false, text, ...extra },
    }) as const;

  it("takes a file whose lines are the hunks' new lines", () => {
    expect(textFits(HUNKS40, NEW40)).toBe(true);
    expect(gapText(HUNKS40, "h", view(NEW40.join("")))).toEqual(ready(NEW40));
  });

  it("refuses another file, a binary or too large one, and one with another hash", () => {
    const other = [...NEW40];
    other[8] = "changed\n";
    expect(textFits(HUNKS40, other)).toBe(false);
    expect(gapText(HUNKS40, "h", view(other.join("")))).toBeNull();
    expect(gapText(HUNKS40, "h", view(NEW40.slice(0, 20).join("")))).toBeNull();
    expect(gapText(HUNKS40, "other", view(NEW40.join("")))).toBeNull();
    expect(gapText(HUNKS40, "h", view("", { binary: true }))).toBeNull();
    expect(gapText(HUNKS40, "h", view("", { text: null }))).toBeNull();
    expect(
      gapText(HUNKS40, "h", {
        status: "ready",
        file: { hash: "h", binary: false, too_large: true },
      }),
    ).toBeNull();
  });

  it("passes on a pending or failed read", () => {
    expect(gapText(HUNKS40, "h", { status: "pending" })).toEqual({
      status: "pending",
    });
    expect(
      gapText(HUNKS40, "h", { status: "error", retry: () => undefined }),
    ).toEqual({
      status: "error",
    });
  });
});

describe("addSpan", () => {
  it("sorts the spans and joins overlapping and touching ones", () => {
    let spans: Span[] = [];
    spans = addSpan(spans, [10, 12]);
    spans = addSpan(spans, [1, 3]);
    expect(spans).toEqual([
      [1, 3],
      [10, 12],
    ]);
    expect(addSpan(spans, [4, 9])).toEqual([[1, 12]]);
    expect(addSpan(spans, [2, 11])).toEqual([[1, 12]]);
    expect(addSpan(spans, [5, 4])).toEqual(spans);
  });
});

describe("hiddenParts and expandPart", () => {
  const middle: Gap = { index: 1, first: 14, last: 60, shift: 0, tail: false };
  const top: Gap = { index: 0, first: 1, last: 50, shift: 0, tail: false };
  const tail: Gap = { index: 2, first: 33, last: 80, shift: 0, tail: true };

  it("offers up and down in a long middle gap, only up above the first hunk and only down below the last", () => {
    expect(hiddenParts(middle, [])).toEqual([
      { first: 14, last: 60, up: true, down: true },
    ]);
    expect(hiddenParts(top, [])).toEqual([
      { first: 1, last: 50, up: true, down: false },
    ]);
    expect(hiddenParts(tail, [])).toEqual([
      { first: 33, last: 80, up: false, down: true },
    ]);
  });

  it("offers only all for a run of one step or less", () => {
    const short: Gap = { ...middle, last: 14 + EXPAND_STEP - 1 };
    expect(hiddenParts(short, [])).toEqual([
      { first: 14, last: 33, up: false, down: false },
    ]);
  });

  it("splits a gap around shown spans", () => {
    expect(
      hiddenParts(middle, [
        [1, 5],
        [14, 20],
        [30, 31],
        [61, 70],
      ]),
    ).toEqual([
      { first: 21, last: 29, up: false, down: false },
      { first: 32, last: 60, up: true, down: true },
    ]);
    expect(hiddenParts(middle, [[10, 70]])).toEqual([]);
  });

  it("shows a step under the row above, a step over the row below, or the whole run", () => {
    const part = { first: 14, last: 60 };
    expect(expandPart([], part, "down")).toEqual({
      spans: [[14, 33]],
      focus: 14,
    });
    expect(expandPart([], part, "up")).toEqual({
      spans: [[41, 60]],
      focus: 41,
    });
    expect(expandPart([], part, "all")).toEqual({
      spans: [[14, 60]],
      focus: 14,
    });
    expect(expandPart([], { first: 14, last: 20 }, "up")).toEqual({
      spans: [[14, 20]],
      focus: 14,
    });
  });
});

describe("keptSpans", () => {
  const gaps: Gap[] = [
    { index: 0, first: 1, last: 6, shift: 0, tail: false },
    { index: 1, first: 14, last: 40, shift: 2, tail: false },
  ];

  it("shows a kept range in a gap with its edge lines, clipped to the gap", () => {
    expect(keptSpans(gaps, [{ side: "new", start: 25, end: 26 }])).toEqual([
      [22, 29],
    ]);
    expect(keptSpans(gaps, [{ side: "new", start: 2, end: 2 }])).toEqual([
      [1, 5],
    ]);
  });

  it("moves an old range to new numbers by the gap's shift", () => {
    expect(keptSpans(gaps, [{ side: "old", start: 27, end: 27 }])).toEqual([
      [22, 28],
    ]);
  });

  it("ignores a range in no gap", () => {
    expect(keptSpans(gaps, [{ side: "new", start: 9, end: 10 }])).toEqual([]);
  });
});

describe("withGaps", () => {
  it("draws each known gap as one row with the header of the hunk below while the file loads or failed", () => {
    for (const status of ["pending", "error"] as const) {
      const rows = laid(HUNKS40, state({ text: { status } }));
      expect(rows.map((r) => r.type)).toEqual([
        "gap",
        ...Array<string>(8).fill("line"),
        "gap",
        ...Array<string>(7).fill("line"),
      ]);
      expect(gapRows(rows)).toEqual([
        expect.objectContaining({
          first: 1,
          last: 6,
          hidden: 6,
          header: "@@ −7,7 +7,7 @@",
          status,
          up: false,
          down: false,
        }),
        expect.objectContaining({
          first: 14,
          last: 26,
          header: "@@ −27,6 +27,7 @@",
          status,
        }),
      ]);
    }
  });

  it("adds the gap below the last hunk once the file is read", () => {
    const rows = laid(HUNKS40, state({ text: ready(NEW40) }));
    expect(gapRows(rows).map((g) => [g.first, g.last, g.header])).toEqual([
      [1, 6, "@@ −7,7 +7,7 @@"],
      [14, 26, "@@ −27,6 +27,7 @@"],
      [34, 40, null],
    ]);
  });

  it("drops a hunk's header once a line of the gap above it shows", () => {
    const rows = laid(
      HUNKS40,
      state({ text: ready(NEW40), spans: [[24, 26]] }),
    );
    expect(rows.some((r) => r.type === "hunk")).toBe(false);
    const gap = gapRows(rows)[1];
    expect(gap).toMatchObject({ first: 14, last: 23, header: null });
    const shown = lineRows(rows).filter((r) => r.key.startsWith("g"));
    expect(shown.map((r) => [r.oldNo, r.newNo, r.text])).toEqual([
      [24, 24, "l24"],
      [25, 25, "l25"],
      [26, 26, "l26"],
    ]);
  });

  it("keeps the first hunk's header when no line sits above it", () => {
    const lines = ["A\n", "b\n", "c\n", "d\n", "e\n", "f\n"];
    const hunks = [
      hunk(1, 1, [del("a"), add("A"), ctx("b"), ctx("c"), ctx("d")]),
    ];
    const rows = laid(hunks, state({ text: ready(lines) }));
    expect(rows[0]?.type).toBe("hunk");
    expect(gapRows(rows)).toEqual([
      expect.objectContaining({ first: 5, last: 6, up: false, down: false }),
    ]);
    const whole = laid(hunks, state({ text: ready(lines), whole: true }));
    expect(whole.some((r) => r.type === "hunk" || r.type === "gap")).toBe(
      false,
    );
  });

  it("marks the last line of a file without a terminator", () => {
    const lines = [...NEW40.slice(0, 39), "l40"];
    const rows = laid(HUNKS40, state({ text: ready(lines), whole: true }));
    const last = lineRows(rows).at(-1);
    expect(last).toMatchObject({
      newNo: 40,
      oldNo: 39,
      text: "l40",
      noNewline: true,
    });
    expect(lineRows(rows).filter((r) => r.noNewline)).toHaveLength(1);
  });

  it("opens the lines around a kept range", () => {
    const rows = laid(
      HUNKS40,
      state({
        text: ready(NEW40),
        kept: [{ side: "new", start: 20, end: 20 }],
      }),
    );
    expect(
      lineRows(rows)
        .filter((r) => r.key.startsWith("g"))
        .map((r) => r.newNo),
    ).toEqual([17, 18, 19, 20, 21, 22, 23]);
    expect(gapRows(rows).map((g) => [g.first, g.last])).toEqual([
      [1, 6],
      [14, 16],
      [24, 26],
      [34, 40],
    ]);
  });

  it("leaves the rows alone when the hunks make no gaps", () => {
    const hunks = [hunk(3, 5, [ctx("a")])];
    const rows = collapseRuns(hunkRows(hunks), new Set());
    expect(withGaps(rows, hunks, state({ text: ready(file(9)) }))).toEqual(
      rows,
    );
    expect(withGaps([], [], state())).toEqual([]);
  });
});

describe("layoutWithGaps", () => {
  it("lays out a file without a text source as before", () => {
    const rows = collapseRuns(hunkRows(HUNKS40), new Set());
    expect(layoutWithGaps(HUNKS40, new Set(), null).rows).toEqual(rows);
  });

  it("limits the rows with the gaps in them", () => {
    const layout = layoutWithGaps(
      HUNKS40,
      new Set(),
      state({ text: ready(NEW40), whole: true }),
      10,
    );
    expect(layout.rows).toHaveLength(10);
    expect(layout.remaining).toBe(31);
  });
});

describe("fileLineSet and hasGaps", () => {
  it("holds every line of both sides", () => {
    const set = fileLineSet(HUNKS40, 40);
    expect(set.has("new:40")).toBe(true);
    expect(set.has("new:41")).toBe(false);
    expect(set.has("old:39")).toBe(true);
    expect(set.has("old:40")).toBe(false);
    expect(set.has("old:0")).toBe(false);
    expect(set.has("side:3")).toBe(false);
  });

  it("finds a gap only when a line is outside the hunks", () => {
    expect(hasGaps(HUNKS40, 40)).toBe(true);
    const all = [hunk(1, 1, [del("a"), add("b")])];
    expect(hasGaps(all, 1)).toBe(false);
    expect(hasGaps(all, 0)).toBe(false);
  });
});

// Property checks over random files and edits, with a fixed seed so a failure repeats.

function random(seed: number): () => number {
  let s = seed;
  return () => {
    s = (s * 1103515245 + 12345) % 2147483648;
    return s / 2147483648;
  };
}

interface Case {
  oldLines: string[];
  newLines: string[];
  hunks: Hunk[];
}

/** A random edit of a random file, grouped into hunks with three context lines as the daemon groups them. */
function randomCase(next: () => number): Case {
  const entries: { kind: HunkLine["kind"]; text: string }[] = [];
  const count = Math.floor(next() * 120);
  for (let i = 0; i < count; i++) {
    const roll = next();
    const text = `line ${String(i)}${roll < 0.05 ? "\r\n" : "\n"}`;
    if (roll < 0.9) entries.push({ kind: "context", text });
    else if (roll < 0.95) entries.push({ kind: "removed", text });
    else entries.push({ kind: "added", text: `new ${text}` });
  }
  const oldLines: string[] = [];
  const newLines: string[] = [];
  const numbered = entries.map((e) => {
    const line: HunkLine = {
      kind: e.kind,
      text: e.text,
      old_no: e.kind === "added" ? null : oldLines.push(e.text),
      new_no: e.kind === "removed" ? null : newLines.push(e.text),
      no_newline: false,
    };
    return line;
  });
  const changed = numbered.flatMap((l, i) => (l.kind === "context" ? [] : [i]));
  const inHunk = numbered.map((_, i) =>
    changed.some((c) => Math.abs(c - i) <= 3),
  );
  const hunks: Hunk[] = [];
  let current: HunkLine[] = [];
  const close = () => {
    if (current.length === 0) return;
    const olds = current.flatMap((l) => (l.old_no === null ? [] : [l.old_no]));
    const news = current.flatMap((l) => (l.new_no === null ? [] : [l.new_no]));
    const before = numbered.indexOf(current[0] as HunkLine);
    const oldBefore = numbered
      .slice(0, before)
      .filter((l) => l.old_no !== null).length;
    const newBefore = numbered
      .slice(0, before)
      .filter((l) => l.new_no !== null).length;
    hunks.push({
      old_start: olds[0] ?? oldBefore,
      old_lines: olds.length,
      new_start: news[0] ?? newBefore,
      new_lines: news.length,
      lines: current,
    });
    current = [];
  };
  numbered.forEach((line, i) => {
    if (inHunk[i]) current.push(line);
    else close();
  });
  close();
  return { oldLines, newLines, hunks };
}

function check(c: Case, rows: readonly DiffRow[]) {
  const lines = lineRows(rows);
  const news = lines.flatMap((r) => (r.newNo === null ? [] : [r.newNo]));
  const olds = lines.flatMap((r) => (r.oldNo === null ? [] : [r.oldNo]));
  expect(news).toEqual([...news].sort((a, b) => a - b));
  expect(new Set(news).size).toBe(news.length);
  expect(olds).toEqual([...olds].sort((a, b) => a - b));
  for (const row of lines) {
    if (row.newNo !== null) {
      expect(row.text).toBe(c.newLines[row.newNo - 1]?.replace(/\r?\n$/, ""));
    }
    if (row.oldNo !== null) {
      expect(row.text).toBe(c.oldLines[row.oldNo - 1]?.replace(/\r?\n$/, ""));
    }
  }
  const hidden = gapRows(rows).reduce((n, g) => n + g.hidden, 0);
  expect(hidden + news.length).toBe(c.newLines.length);
}

describe("gap properties", () => {
  it("shows every line of both sides once, in order, with matching text, however the gaps are opened", () => {
    const next = random(7);
    let used = 0;
    for (let n = 0; n < 150; n++) {
      const c = randomCase(next);
      expect(splitLines(c.newLines.join(""))).toEqual(c.newLines);
      expect(textFits(c.hunks, c.newLines)).toBe(true);
      if (c.hunks.length === 0) continue;
      const text = ready(c.newLines);
      const whole = laid(c.hunks, state({ text, whole: true }));
      check(c, whole);
      expect(lineRows(whole).filter((r) => r.newNo !== null)).toHaveLength(
        c.newLines.length,
      );
      expect(lineRows(whole).filter((r) => r.oldNo !== null)).toHaveLength(
        c.oldLines.length,
      );
      let spans: Span[] = [];
      for (let step = 0; step < 12; step++) {
        const rows = laid(c.hunks, state({ text, spans }));
        check(c, rows);
        const gaps = gapRows(rows);
        const gap = gaps[Math.floor(next() * gaps.length)];
        if (!gap) break;
        const options: ExpandHow[] = [
          "all",
          ...(gap.up ? (["up"] as const) : []),
          ...(gap.down ? (["down"] as const) : []),
        ];
        const how = options[Math.floor(next() * options.length)] ?? "all";
        used++;
        const result = expandPart(spans, gap, how);
        expect(result.focus).toBeGreaterThanOrEqual(gap.first);
        expect(result.focus).toBeLessThanOrEqual(gap.last);
        spans = result.spans;
      }
    }
    expect(used).toBeGreaterThan(300);
  });

  it("ends with the whole file's lines once every gap is opened in full", () => {
    const next = random(11);
    for (let n = 0; n < 80; n++) {
      const c = randomCase(next);
      if (c.hunks.length === 0) continue;
      const text = ready(c.newLines);
      let spans: Span[] = [];
      for (;;) {
        const gap = gapRows(laid(c.hunks, state({ text, spans })))[0];
        if (!gap) break;
        spans = expandPart(spans, gap, "all").spans;
      }
      const opened = lineRows(laid(c.hunks, state({ text, spans })));
      const whole = lineRows(laid(c.hunks, state({ text, whole: true })));
      expect(opened.map((r) => [r.oldNo, r.newNo, r.text])).toEqual(
        whole.map((r) => [r.oldNo, r.newNo, r.text]),
      );
    }
  });
});
