import type { DiffRow } from "./layout";

export type DiffSide = "old" | "new";

/** A line of one side of the diff. */
export interface LinePoint {
  side: DiffSide;
  line: number;
}

/** The shape review comments store: one line or a run of consecutive lines of one side of one file, `start_line <= end_line`. */
export interface DiffSelection {
  path: string;
  side: DiffSide;
  start_line: number;
  end_line: number;
}

export interface SelectionState {
  /** Where a Shift extension starts. */
  anchor: LinePoint | null;
  /** The line that has keyboard focus. */
  cursor: LinePoint | null;
}

export type KeyCommand =
  | "select"
  | "extend"
  | "up"
  | "down"
  | "extend-up"
  | "extend-down"
  | "clear";

export type SelectionAction =
  | { type: "click"; point: LinePoint; shift: boolean }
  | { type: "focus"; point: LinePoint }
  | { type: "key"; command: KeyCommand };

export interface SelectionContext {
  path: string;
  /** The selectable lines in display order; hidden lines are not among them. */
  points: readonly LinePoint[];
  selection: DiffSelection | null;
}

export interface SelectionResult {
  state: SelectionState;
  selection: DiffSelection | null;
}

export const EMPTY_STATE: SelectionState = { anchor: null, cursor: null };

/** A context line carries both numbers and belongs to the new side; a line with no new number belongs to the old side. */
export function pointOf(row: DiffRow): LinePoint | null {
  if (row.type !== "line") return null;
  if (row.newNo !== null) return { side: "new", line: row.newNo };
  return row.oldNo === null ? null : { side: "old", line: row.oldNo };
}

/** Every line a row stands for: a context line is both its new and its old line, so a thread on either is drawn on it. */
export function threadPointsOf(row: DiffRow): LinePoint[] {
  const point = pointOf(row);
  if (!point) return [];
  if (row.type === "line" && row.newNo !== null && row.oldNo !== null) {
    return [point, { side: "old", line: row.oldNo }];
  }
  return [point];
}

export function pointsOf(rows: readonly DiffRow[]): LinePoint[] {
  return rows.flatMap((row) => {
    const point = pointOf(row);
    return point ? [point] : [];
  });
}

export function samePoint(a: LinePoint | null, b: LinePoint | null): boolean {
  return a !== null && b !== null && a.side === b.side && a.line === b.line;
}

export function keyCommand(key: string, shift: boolean): KeyCommand | null {
  switch (key) {
    case " ":
      return shift ? "extend" : "select";
    case "ArrowUp":
      return shift ? "extend-up" : "up";
    case "ArrowDown":
      return shift ? "extend-down" : "down";
    case "Escape":
      return "clear";
    default:
      return null;
  }
}

export function isSelected(
  selection: DiffSelection | null,
  path: string,
  point: LinePoint | null,
): boolean {
  return (
    selection !== null &&
    point !== null &&
    selection.path === path &&
    selection.side === point.side &&
    point.line >= selection.start_line &&
    point.line <= selection.end_line
  );
}

/** The composer goes under this line. */
export function isLastSelected(
  selection: DiffSelection | null,
  path: string,
  point: LinePoint | null,
): boolean {
  return (
    isSelected(selection, path, point) && point?.line === selection?.end_line
  );
}

/** The anchor kept from the last click when it is still an end of the selection; else the start of a selection set from outside. */
function effectiveAnchor(
  anchor: LinePoint | null,
  ctx: SelectionContext,
): LinePoint | null {
  const { selection, path } = ctx;
  if (selection === null || selection.path !== path) return null;
  if (
    anchor !== null &&
    anchor.side === selection.side &&
    (anchor.line === selection.start_line || anchor.line === selection.end_line)
  ) {
    return anchor;
  }
  return { side: selection.side, line: selection.start_line };
}

function select(point: LinePoint, path: string): SelectionResult {
  return {
    state: { anchor: point, cursor: point },
    selection: {
      path,
      side: point.side,
      start_line: point.line,
      end_line: point.line,
    },
  };
}

/** Every line between the two ends is on screen on that side. */
function consecutive(
  side: DiffSide,
  start: number,
  end: number,
  points: readonly LinePoint[],
): boolean {
  const shown = new Set(
    points.filter((p) => p.side === side).map((p) => p.line),
  );
  for (let line = start; line <= end; line++) {
    if (!shown.has(line)) return false;
  }
  return true;
}

/** A range that would cross sides or a line that is not on screen starts a new selection at the target instead. */
function extend(
  anchor: LinePoint | null,
  target: LinePoint,
  ctx: SelectionContext,
): SelectionResult {
  if (anchor === null || anchor.side !== target.side) {
    return select(target, ctx.path);
  }
  const start = Math.min(anchor.line, target.line);
  const end = Math.max(anchor.line, target.line);
  if (!consecutive(target.side, start, end, ctx.points)) {
    return select(target, ctx.path);
  }
  return {
    state: { anchor, cursor: target },
    selection: {
      path: ctx.path,
      side: target.side,
      start_line: start,
      end_line: end,
    },
  };
}

function step(
  cursor: LinePoint | null,
  points: readonly LinePoint[],
  delta: 1 | -1,
): LinePoint | null {
  const index = points.findIndex((p) => samePoint(p, cursor));
  if (index < 0) return points[0] ?? null;
  return points[index + delta] ?? points[index] ?? null;
}

export function reduceSelection(
  state: SelectionState,
  action: SelectionAction,
  ctx: SelectionContext,
): SelectionResult {
  const kept = { state, selection: ctx.selection };
  switch (action.type) {
    case "click":
      return action.shift
        ? extend(effectiveAnchor(state.anchor, ctx), action.point, ctx)
        : select(action.point, ctx.path);
    case "focus":
      return { ...kept, state: { ...state, cursor: action.point } };
    case "key":
      break;
  }
  const { command } = action;
  if (command === "clear") {
    return { state: { anchor: null, cursor: state.cursor }, selection: null };
  }
  if (command === "up" || command === "down") {
    const cursor = step(state.cursor, ctx.points, command === "up" ? -1 : 1);
    return { ...kept, state: { ...state, cursor } };
  }
  if (command === "select" || command === "extend") {
    const target = state.cursor ?? ctx.points[0] ?? null;
    if (target === null) return kept;
    return command === "select"
      ? select(target, ctx.path)
      : extend(effectiveAnchor(state.anchor, ctx), target, ctx);
  }
  const target = step(
    state.cursor,
    ctx.points,
    command === "extend-up" ? -1 : 1,
  );
  if (target === null) return kept;
  const anchor = effectiveAnchor(state.anchor, ctx) ?? state.cursor;
  return extend(anchor, target, ctx);
}

/** A review thread anchored to a range; the proposal screens supply its content. */
export interface DiffThread {
  id: string;
  range: DiffSelection;
}

/** The threads whose range holds the line; the row shows a comment marker. */
export function threadsCovering(
  threads: readonly DiffThread[],
  path: string,
  point: LinePoint | null,
): DiffThread[] {
  return threads.filter((t) => isSelected(t.range, path, point));
}

/** The marks whose range holds one of a row's lines, each once, and whether the row holds the first line of its range. */
export function marksAt(
  marks: readonly DiffThread[],
  path: string,
  points: readonly LinePoint[],
): { mark: DiffThread; first: boolean }[] {
  return marks
    .filter((m) => points.some((p) => isSelected(m.range, path, p)))
    .map((mark) => ({
      mark,
      first: points.some(
        (p) => p.side === mark.range.side && p.line === mark.range.start_line,
      ),
    }));
}

/** The threads that end on the line; they show under it. */
export function threadsEndingAt(
  threads: readonly DiffThread[],
  path: string,
  point: LinePoint | null,
): DiffThread[] {
  return threads.filter((t) => isLastSelected(t.range, path, point));
}

export function pointKey(point: LinePoint): string {
  return `${point.side}:${String(point.line)}`;
}

/** The line that takes Tab focus in the diff: the cursor while it is on screen, else the first line. */
export function tabStop(
  cursor: LinePoint | null,
  points: readonly LinePoint[],
): LinePoint | null {
  return points.find((p) => samePoint(p, cursor)) ?? points[0] ?? null;
}

export function sameSelection(
  a: DiffSelection | null,
  b: DiffSelection | null,
): boolean {
  if (a === null || b === null) return a === b;
  return (
    a.path === b.path &&
    a.side === b.side &&
    a.start_line === b.start_line &&
    a.end_line === b.end_line
  );
}
