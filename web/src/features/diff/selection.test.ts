import { describe, expect, it } from "vitest";
import type { DiffRow } from "./layout";
import {
  EMPTY_STATE,
  isLastSelected,
  isSelected,
  keyCommand,
  pointKey,
  pointOf,
  pointsOf,
  reduceSelection,
  sameSelection,
  samePoint,
  tabStop,
  threadsCovering,
  threadsEndingAt,
  type DiffSelection,
  type LinePoint,
  type SelectionAction,
  type SelectionState,
} from "./selection";

const PATH = "src/a.ts";

function row(oldNo: number | null, newNo: number | null): DiffRow {
  const kind = oldNo === null ? "add" : newNo === null ? "remove" : "context";
  return {
    type: "line",
    key: `${String(oldNo)}-${String(newNo)}`,
    kind,
    oldNo,
    newNo,
    text: "",
  };
}

// Context 10/10, removed 11 and 12, added 11 and 12, context 13/13, then a gap and context 40/40, 41/41.
const ROWS: DiffRow[] = [
  { type: "hunk", key: "h0", header: "@@" },
  row(10, 10),
  row(11, null),
  row(12, null),
  row(null, 11),
  row(null, 12),
  row(13, 13),
  { type: "hunk", key: "h1", header: "@@" },
  row(40, 40),
  row(41, 41),
];
const POINTS = pointsOf(ROWS);

const n = (line: number): LinePoint => ({ side: "new", line });
const o = (line: number): LinePoint => ({ side: "old", line });
const range = (
  side: "old" | "new",
  start: number,
  end: number,
): DiffSelection => ({ path: PATH, side, start_line: start, end_line: end });

/** Runs actions in order, feeding each result's selection into the next, as the controlled viewer does. */
function run(
  actions: SelectionAction[],
  start: { state?: SelectionState; selection?: DiffSelection | null } = {},
) {
  let state = start.state ?? EMPTY_STATE;
  let selection = start.selection ?? null;
  for (const action of actions) {
    const result = reduceSelection(state, action, {
      path: PATH,
      points: POINTS,
      selection,
    });
    state = result.state;
    selection = result.selection;
  }
  return { state, selection };
}

const click = (point: LinePoint, shift = false): SelectionAction => ({
  type: "click",
  point,
  shift,
});
const key = (k: string, shift = false): SelectionAction => {
  const command = keyCommand(k, shift);
  if (command === null) throw new Error(`no command for ${k}`);
  return { type: "key", command };
};

describe("sides", () => {
  it("puts a context line on the new side, a removed line on the old side and an added line on the new side", () => {
    expect(pointOf(row(10, 10))).toEqual(n(10));
    expect(pointOf(row(11, null))).toEqual(o(11));
    expect(pointOf(row(null, 11))).toEqual(n(11));
  });

  it("puts a line with only an old number on the old side", () => {
    expect(
      pointOf({
        type: "line",
        key: "k",
        kind: "context",
        oldNo: 5,
        newNo: null,
        text: "",
      }),
    ).toEqual(o(5));
  });

  it("gives hunk headers, expanders and numberless lines no point", () => {
    expect(pointOf({ type: "hunk", key: "h", header: "@@" })).toBeNull();
    expect(pointOf({ type: "expander", key: "x", hidden: 3 })).toBeNull();
    expect(
      pointOf({
        type: "line",
        key: "k",
        kind: "context",
        oldNo: null,
        newNo: null,
        text: "",
      }),
    ).toBeNull();
  });

  it("lists the selectable lines in display order", () => {
    expect(POINTS).toEqual([
      n(10),
      o(11),
      o(12),
      n(11),
      n(12),
      n(13),
      n(40),
      n(41),
    ]);
  });
});

describe("click", () => {
  it("selects one line", () => {
    expect(run([click(n(12))]).selection).toEqual(range("new", 12, 12));
  });

  it("selects a removed line on the old side", () => {
    expect(run([click(o(11))]).selection).toEqual(range("old", 11, 11));
  });

  it("replaces the selection with a plain click", () => {
    expect(run([click(n(10)), click(n(13))]).selection).toEqual(
      range("new", 13, 13),
    );
  });
});

describe("shift-click", () => {
  it("extends from the anchor to the clicked line", () => {
    expect(run([click(n(10)), click(n(13), true)]).selection).toEqual(
      range("new", 10, 13),
    );
  });

  it("extends upward and keeps start_line <= end_line", () => {
    expect(run([click(n(13)), click(n(11), true)]).selection).toEqual(
      range("new", 11, 13),
    );
  });

  it("keeps the anchor across extensions", () => {
    const { selection } = run([
      click(n(12)),
      click(n(13), true),
      click(n(10), true),
    ]);
    expect(selection).toEqual(range("new", 10, 12));
  });

  it("selects one line when nothing is selected", () => {
    expect(run([click(n(12), true)]).selection).toEqual(range("new", 12, 12));
  });

  it("extends a run of removed lines on the old side", () => {
    expect(run([click(o(11)), click(o(12), true)]).selection).toEqual(
      range("old", 11, 12),
    );
  });

  it("extends from the start of a selection set from outside", () => {
    const { selection } = run([click(n(13), true)], {
      selection: range("new", 11, 12),
    });
    expect(selection).toEqual(range("new", 11, 13));
  });

  it("starts fresh when the selection belongs to another file", () => {
    const { selection } = run([click(n(13), true)], {
      selection: { ...range("new", 10, 12), path: "other.ts" },
    });
    expect(selection).toEqual(range("new", 13, 13));
  });
});

describe("cross-side restart", () => {
  it("starts a new selection when a shift-click lands on the other side", () => {
    const { selection } = run([click(n(10)), click(o(12), true)]);
    expect(selection).toEqual(range("old", 12, 12));
  });

  it("never produces a mixed-side range, whatever order the clicks come in", () => {
    const all = POINTS;
    for (const a of all) {
      for (const b of all) {
        const { selection } = run([click(a), click(b, true)]);
        expect(selection?.side).toBe(b.side);
        if (a.side !== b.side) {
          expect(selection).toEqual(range(b.side, b.line, b.line));
        }
      }
    }
  });

  it("restarts with Shift and an arrow key onto the other side", () => {
    // 10 is on the new side; the next line down is removed line 11 on the old side.
    const { selection } = run([click(n(10)), key("ArrowDown", true)]);
    expect(selection).toEqual(range("old", 11, 11));
  });
});

describe("lines that are not on screen", () => {
  it("restarts when the range would take in lines between hunks", () => {
    const { selection } = run([click(n(13)), click(n(40), true)]);
    expect(selection).toEqual(range("new", 40, 40));
  });

  it("extends across hunks when the line numbers on that side run on", () => {
    const rows: DiffRow[] = [
      { type: "hunk", key: "h0", header: "@@" },
      row(1, 1),
      row(2, 2),
      { type: "hunk", key: "h1", header: "@@" },
      row(3, 3),
    ];
    const result = reduceSelection(
      { anchor: n(1), cursor: n(1) },
      click(n(3), true),
      { path: PATH, points: pointsOf(rows), selection: range("new", 1, 1) },
    );
    expect(result.selection).toEqual(range("new", 1, 3));
  });
});

describe("keyboard", () => {
  it("maps keys to commands", () => {
    expect(keyCommand(" ", false)).toBe("select");
    expect(keyCommand(" ", true)).toBe("extend");
    expect(keyCommand("ArrowUp", false)).toBe("up");
    expect(keyCommand("ArrowUp", true)).toBe("extend-up");
    expect(keyCommand("ArrowDown", false)).toBe("down");
    expect(keyCommand("ArrowDown", true)).toBe("extend-down");
    expect(keyCommand("Escape", false)).toBe("clear");
    expect(keyCommand("a", false)).toBeNull();
  });

  it("moves the cursor with the arrows and selects with Space", () => {
    const { state, selection } = run([
      { type: "focus", point: n(11) },
      key("ArrowDown"),
      key(" "),
    ]);
    expect(state.cursor).toEqual(n(12));
    expect(selection).toEqual(range("new", 12, 12));
  });

  it("does not change the selection while the cursor moves", () => {
    const { selection } = run([
      click(n(12)),
      key("ArrowDown"),
      key("ArrowDown"),
    ]);
    expect(selection).toEqual(range("new", 12, 12));
  });

  it("extends from the anchor with Shift and the arrows", () => {
    const { selection, state } = run([
      click(n(11)),
      key("ArrowDown", true),
      key("ArrowDown", true),
    ]);
    expect(selection).toEqual(range("new", 11, 13));
    expect(state.anchor).toEqual(n(11));
    expect(state.cursor).toEqual(n(13));
  });

  it("shrinks back toward the anchor with Shift and the other arrow", () => {
    const { selection } = run([
      click(n(11)),
      key("ArrowDown", true),
      key("ArrowDown", true),
      key("ArrowUp", true),
    ]);
    expect(selection).toEqual(range("new", 11, 12));
  });

  it("extends from the cursor when nothing is selected", () => {
    const { selection } = run([
      { type: "focus", point: n(11) },
      key("ArrowDown", true),
    ]);
    expect(selection).toEqual(range("new", 11, 12));
  });

  it("extends to the cursor with Shift and Space", () => {
    const { selection } = run([
      click(n(11)),
      key("ArrowDown"),
      key("ArrowDown"),
      key(" ", true),
    ]);
    expect(selection).toEqual(range("new", 11, 13));
  });

  it("starts at the first line when there is no cursor", () => {
    expect(run([key(" ")]).selection).toEqual(range("new", 10, 10));
    expect(run([key("ArrowDown")]).state.cursor).toEqual(n(10));
    expect(run([key("ArrowUp")]).state.cursor).toEqual(n(10));
    expect(run([key("ArrowDown", true)]).selection).toEqual(
      range("new", 10, 10),
    );
  });

  it("stays on the last and the first line at the ends", () => {
    expect(
      run([{ type: "focus", point: n(41) }, key("ArrowDown")]).state.cursor,
    ).toEqual(n(41));
    expect(
      run([{ type: "focus", point: n(10) }, key("ArrowUp")]).state.cursor,
    ).toEqual(n(10));
  });

  it("does nothing in a file with no lines", () => {
    const ctx = { path: PATH, points: [], selection: null };
    for (const command of ["select", "extend", "extend-down"] as const) {
      expect(
        reduceSelection(EMPTY_STATE, { type: "key", command }, ctx),
      ).toEqual({
        state: EMPTY_STATE,
        selection: null,
      });
    }
  });
});

describe("clear", () => {
  it("clears the selection and the anchor with Escape and keeps the cursor", () => {
    const { selection, state } = run([
      click(n(11)),
      click(n(13), true),
      key("Escape"),
    ]);
    expect(selection).toBeNull();
    expect(state).toEqual({ anchor: null, cursor: n(13) });
  });

  it("starts a new selection after a clear instead of extending the old one", () => {
    const { selection } = run([
      click(n(10)),
      key("Escape"),
      click(n(13), true),
    ]);
    expect(selection).toEqual(range("new", 13, 13));
  });
});

describe("helpers", () => {
  it("tells whether a line is selected and whether it is the last selected line", () => {
    const sel = range("new", 11, 13);
    expect(isSelected(sel, PATH, n(12))).toBe(true);
    expect(isSelected(sel, PATH, o(12))).toBe(false);
    expect(isSelected(sel, "other.ts", n(12))).toBe(false);
    expect(isSelected(sel, PATH, n(14))).toBe(false);
    expect(isSelected(null, PATH, n(12))).toBe(false);
    expect(isSelected(sel, PATH, null)).toBe(false);
    expect(isLastSelected(sel, PATH, n(13))).toBe(true);
    expect(isLastSelected(sel, PATH, n(12))).toBe(false);
  });

  it("compares points and selections", () => {
    expect(samePoint(n(1), n(1))).toBe(true);
    expect(samePoint(n(1), o(1))).toBe(false);
    expect(samePoint(null, n(1))).toBe(false);
    expect(sameSelection(null, null)).toBe(true);
    expect(sameSelection(range("new", 1, 2), null)).toBe(false);
    expect(sameSelection(range("new", 1, 2), range("new", 1, 2))).toBe(true);
    expect(sameSelection(range("new", 1, 2), range("new", 1, 3))).toBe(false);
    expect(pointKey(o(4))).toBe("old:4");
  });

  it("puts the Tab stop on the cursor while it is on screen, else the first line", () => {
    expect(tabStop(n(12), POINTS)).toEqual(n(12));
    expect(tabStop(n(99), POINTS)).toEqual(n(10));
    expect(tabStop(null, [])).toBeNull();
  });

  it("finds the threads that cover a line and the threads that end on it", () => {
    const threads = [
      { id: "a", range: range("new", 11, 13) },
      { id: "b", range: range("old", 11, 11) },
    ];
    expect(threadsCovering(threads, PATH, n(12)).map((t) => t.id)).toEqual([
      "a",
    ]);
    expect(threadsEndingAt(threads, PATH, n(12))).toEqual([]);
    expect(threadsEndingAt(threads, PATH, n(13)).map((t) => t.id)).toEqual([
      "a",
    ]);
    expect(threadsEndingAt(threads, PATH, o(11)).map((t) => t.id)).toEqual([
      "b",
    ]);
  });
});
