import type { Hunk } from "@/api/diff";
import {
  ROW_LIMIT,
  collapseRuns,
  hunkHeader,
  hunkRows,
  lineText,
  type CollapseOptions,
  type DiffRow,
  type FileLayout,
  type GapRow,
  type LineRow,
} from "./layout";
import type { DiffSide } from "./selection";

/** The unchanged lines one "up" or "down" control shows. */
export const EXPAND_STEP = 20;
/** The unchanged lines kept on each side of a kept range inside a gap, so a thread under a hidden line reads with its neighbours. */
export const KEEP_EDGE = 3;

/**
 * The lines of a text as the daemon's line diff counts them: each keeps its terminator (`\n`, `\r\n` or a lone `\r`), and a last line without one
 * still counts, so the hunks' line numbers index into this list.
 */
export function splitLines(text: string): string[] {
  const lines: string[] = [];
  let start = 0;
  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    if (c === "\r" && text[i + 1] === "\n") i++;
    if (c === "\r" || c === "\n") {
      lines.push(text.slice(start, i + 1));
      start = i + 1;
    }
  }
  if (start < text.length) lines.push(text.slice(start));
  return lines;
}

/** The unchanged lines between two hunks, above the first or below the last, by their new numbers; `last < first` when there are none. */
export interface Gap {
  /** `i` is the gap above hunk `i`; the hunk count names the gap below the last hunk. */
  index: number;
  first: number;
  last: number;
  /** A line's old number is its new number plus `shift`: the lines of a gap are the same on both sides. */
  shift: number;
  /** The gap below the last hunk: nothing follows it. */
  tail: boolean;
}

interface Edges {
  firstNew: number;
  lastNew: number;
  firstOld: number;
  lastOld: number;
}

/**
 * A start names the line before the hunk when its side has no lines in it, so the edges come from the numbered lines. They are read in one pass,
 * never spread into one call: a hunk can hold more lines than a browser takes as the arguments of a call.
 */
function edges(hunk: Hunk): Edges {
  const found = {
    firstNew: Infinity,
    lastNew: -Infinity,
    firstOld: Infinity,
    lastOld: -Infinity,
  };
  for (const line of hunk.lines) {
    if (line.new_no !== null) {
      found.firstNew = Math.min(found.firstNew, line.new_no);
      found.lastNew = Math.max(found.lastNew, line.new_no);
    }
    if (line.old_no !== null) {
      found.firstOld = Math.min(found.firstOld, line.old_no);
      found.lastOld = Math.max(found.lastOld, line.old_no);
    }
  }
  const news = found.lastNew >= found.firstNew;
  const olds = found.lastOld >= found.firstOld;
  return {
    firstNew: news ? found.firstNew : hunk.new_start + 1,
    lastNew: news ? found.lastNew : hunk.new_start,
    firstOld: olds ? found.firstOld : hunk.old_start + 1,
    lastOld: olds ? found.lastOld : hunk.old_start,
  };
}

/**
 * The gaps around `hunks` in a new file of `lineCount` lines; without the count the gap below the last hunk is left out. Null when the hunks
 * do not fit such a file: a gap whose two sides differ in length, or a file shorter than its hunks.
 */
export function fileGaps(
  hunks: readonly Hunk[],
  lineCount: number | null,
): Gap[] | null {
  const gaps: Gap[] = [];
  let lastNew = 0;
  let lastOld = 0;
  for (const [index, hunk] of hunks.entries()) {
    const e = edges(hunk);
    if (e.firstNew - lastNew !== e.firstOld - lastOld) return null;
    if (e.firstNew <= lastNew) return null;
    gaps.push({
      index,
      first: lastNew + 1,
      last: e.firstNew - 1,
      shift: lastOld - lastNew,
      tail: false,
    });
    lastNew = e.lastNew;
    lastOld = e.lastOld;
  }
  if (lineCount === null || hunks.length === 0) return gaps;
  if (lineCount < lastNew) return null;
  gaps.push({
    index: hunks.length,
    first: lastNew + 1,
    last: lineCount,
    shift: lastOld - lastNew,
    tail: true,
  });
  return gaps;
}

/** Each line of every hunk that is on the new side is the file's line at its new number, so the file is the one the hunks were built from. */
export function textFits(
  hunks: readonly Hunk[],
  lines: readonly string[],
): boolean {
  return hunks.every((hunk) =>
    hunk.lines.every(
      (line) => line.new_no === null || lines[line.new_no - 1] === line.text,
    ),
  );
}

/** A run of new line numbers, first and last included. */
export type Span = readonly [number, number];

/** `spans` with `span` added: sorted, with overlapping and touching spans joined. */
export function addSpan(spans: readonly Span[], span: Span): Span[] {
  if (span[1] < span[0]) return [...spans];
  const all = [...spans, span].sort((a, b) => a[0] - b[0]);
  const out: [number, number][] = [];
  for (const [first, last] of all) {
    const prev = out[out.length - 1];
    if (prev && first <= prev[1] + 1) {
      prev[1] = Math.max(prev[1], last);
    } else {
      out.push([first, last]);
    }
  }
  return out;
}

/** A run of a gap's lines still hidden. `down` shows the lines under the row above it, `up` the lines over the row below it; a run no longer than one step offers only all. */
export interface HiddenPart {
  first: number;
  last: number;
  up: boolean;
  down: boolean;
}

/** The lines of `gap` that no span shows. */
export function hiddenParts(
  gap: Gap,
  spans: readonly Span[],
  step: number = EXPAND_STEP,
): HiddenPart[] {
  const parts: HiddenPart[] = [];
  const push = (first: number, last: number) => {
    if (last < first) return;
    const long = last - first + 1 > step;
    parts.push({
      first,
      last,
      down: long && first > 1,
      up: long && !(gap.tail && last === gap.last),
    });
  };
  let next = gap.first;
  for (const [first, last] of spans) {
    if (last < next || first > gap.last) continue;
    push(next, Math.min(first - 1, gap.last));
    next = last + 1;
  }
  push(next, gap.last);
  return parts;
}

export type ExpandHow = "up" | "down" | "all";

/** The spans after a control of `part` is used, and the new line that takes focus: the first one the control shows. */
export function expandPart(
  spans: readonly Span[],
  part: Pick<HiddenPart, "first" | "last">,
  how: ExpandHow,
  step: number = EXPAND_STEP,
): { spans: Span[]; focus: number } {
  const span: Span =
    how === "down"
      ? [part.first, Math.min(part.last, part.first + step - 1)]
      : how === "up"
        ? [Math.max(part.first, part.last - step + 1), part.last]
        : [part.first, part.last];
  return { spans: addSpan(spans, span), focus: span[0] };
}

/** Lines that must stay on screen, such as a review thread's range or the selection, on the side they are numbered on. */
export interface KeptRange {
  side: DiffSide;
  start: number;
  end: number;
}

/** The spans that show each kept range that falls in a gap, with `edge` lines around it; an old range is moved to new numbers by the gap's shift. */
export function keptSpans(
  gaps: readonly Gap[],
  kept: readonly KeptRange[],
  edge: number = KEEP_EDGE,
): Span[] {
  let spans: Span[] = [];
  for (const range of kept) {
    for (const gap of gaps) {
      const shift = range.side === "old" ? gap.shift : 0;
      const start = range.start - shift;
      const end = range.end - shift;
      if (end < gap.first || start > gap.last) continue;
      spans = addSpan(spans, [
        Math.max(gap.first, start - edge),
        Math.min(gap.last, end + edge),
      ]);
    }
  }
  return spans;
}

/** The new side's file as its query holds it. */
export type FileTextView =
  | { status: "pending" }
  | { status: "error"; retry: () => void }
  | { status: "ready"; file: FileContent };

/** The part of a file read the gaps need. */
export interface FileContent {
  hash: string;
  binary: boolean;
  too_large: boolean;
  text?: string | null;
}

/**
 * What the gaps can show of the new side: null when the file cannot give lines (binary, too large, not text), or when it is not the file the hunks
 * were built from, so no line shows with a wrong number.
 */
export function gapText(
  hunks: readonly Hunk[],
  newHash: string | null,
  view: FileTextView,
): GapText | null {
  if (view.status !== "ready") return { status: view.status };
  const { file } = view;
  if (file.binary || file.too_large || typeof file.text !== "string") {
    return null;
  }
  if (file.hash !== newHash) return null;
  const lines = splitLines(file.text);
  if (!textFits(hunks, lines) || fileGaps(hunks, lines.length) === null) {
    return null;
  }
  return { status: "ready", lines };
}

/** The file's text as the gaps read it: its lines once they fit the hunks, while they load, or after the read failed. */
export type GapText =
  | { status: "ready"; lines: readonly string[] }
  | { status: "pending" }
  | { status: "error" };

/** The lines a reader showed, with the two sides of the diff they were shown on. */
export interface ShownSpans {
  sides: string | null;
  spans: readonly Span[];
}

/** The two sides of a diff by their hashes, null while it is not read: lines shown on other sides have other numbers. */
export function diffSides(
  diff: { old_hash: string | null; new_hash: string | null } | null,
): string | null {
  return diff === null
    ? null
    : `${diff.old_hash ?? ""}..${diff.new_hash ?? ""}`;
}

/** The spans of `shown` when they were shown on `sides`, none when the diff on screen compares other sides, such as another version of the file. */
export function spansOn(
  shown: ShownSpans,
  sides: string | null,
): readonly Span[] {
  return shown.sides !== null && shown.sides === sides ? shown.spans : [];
}

export interface GapState {
  text: GapText;
  /** The lines the reader has shown. */
  spans: readonly Span[];
  /** Every line of the file is shown. */
  whole: boolean;
  kept?: readonly KeptRange[];
}

function gapLine(gap: Gap, line: number, lines: readonly string[]): LineRow {
  const raw = lines[line - 1] ?? "";
  return {
    type: "line",
    key: `g${String(line)}`,
    kind: "context",
    oldNo: line + gap.shift,
    newNo: line,
    text: lineText(raw),
    noNewline: line === lines.length && !/[\r\n]$/.test(raw),
  };
}

function gapRow(
  part: HiddenPart,
  header: string | null,
  status: GapText["status"],
): GapRow {
  return {
    type: "gap",
    key: `x${String(part.first)}`,
    first: part.first,
    last: part.last,
    hidden: part.last - part.first + 1,
    up: part.up,
    down: part.down,
    header,
    status,
  };
}

/**
 * The rows of one gap. Before the lines are read, a gap shows as one row that names its hidden lines and the header of the hunk below it. Once
 * they are read, each hidden run is one row and every shown line has both numbers; a hunk keeps its own header only while nothing of the gap
 * above it shows, and the rows of the hunk and the shown lines then read as one block.
 */
function gapRows(
  gap: Gap,
  below: Hunk | undefined,
  state: GapState,
  spans: readonly Span[],
): { rows: DiffRow[]; headerShown: boolean } {
  const header = below ? hunkHeader(below) : null;
  const text = state.text;
  if (gap.last < gap.first) {
    return { rows: [], headerShown: !(state.whole && text.status === "ready") };
  }
  if (text.status !== "ready") {
    const part = { first: gap.first, last: gap.last, up: false, down: false };
    return { rows: [gapRow(part, header, text.status)], headerShown: false };
  }
  if (state.whole) {
    const rows: DiffRow[] = [];
    for (let line = gap.first; line <= gap.last; line++) {
      rows.push(gapLine(gap, line, text.lines));
    }
    return { rows, headerShown: false };
  }
  const parts = hiddenParts(gap, spans);
  const untouched =
    parts.length === 1 &&
    parts[0]?.first === gap.first &&
    parts[0].last === gap.last;
  const rows: DiffRow[] = [];
  let line = gap.first;
  for (const part of parts) {
    for (; line < part.first; line++) rows.push(gapLine(gap, line, text.lines));
    rows.push(gapRow(part, untouched ? header : null, "ready"));
    line = part.last + 1;
  }
  for (; line <= gap.last; line++) rows.push(gapLine(gap, line, text.lines));
  return { rows, headerShown: false };
}

/**
 * `rows`, the rows of `hunks` as {@link hunkRows} and `collapseRuns` give them, with the gaps around the hunks woven in. Without the text's line
 * count the gap below the last hunk is not known and is left out. Ready lines must fit the hunks, as {@link gapText} checks; when the hunks'
 * numbers do not make gaps, the rows stay as they are.
 */
export function withGaps(
  rows: readonly DiffRow[],
  hunks: readonly Hunk[],
  state: GapState,
): DiffRow[] {
  const count = state.text.status === "ready" ? state.text.lines.length : null;
  const gaps = fileGaps(hunks, count);
  if (gaps === null || hunks.length === 0) return [...rows];
  const shown = keptSpans(gaps, state.kept ?? []).reduce<Span[]>(
    (all, span) => addSpan(all, span),
    [...state.spans],
  );
  const out: DiffRow[] = [];
  let next = 0;
  for (const row of rows) {
    if (row.type === "hunk") {
      const gap = gaps[next];
      const { rows: woven, headerShown } = gap
        ? gapRows(gap, hunks[next], state, shown)
        : { rows: [], headerShown: true };
      out.push(...woven);
      if (headerShown) out.push(row);
      next++;
      continue;
    }
    out.push(row);
  }
  const tail = gaps[hunks.length];
  if (tail) out.push(...gapRows(tail, undefined, state, shown).rows);
  return out;
}

/** Whether a gap shows anywhere: false once every line of the file is on screen, or when the hunks cover the whole file. */
export function hasGaps(hunks: readonly Hunk[], lineCount: number): boolean {
  const gaps = fileGaps(hunks, lineCount);
  return gaps !== null && gaps.some((gap) => gap.last >= gap.first);
}

/** The `side:line` keys a review thread may end on once the file's text is read: every line of both sides, as the gaps can show each one. */
export interface LineSet {
  has: (key: string) => boolean;
}

export function fileLineSet(
  hunks: readonly Hunk[],
  lineCount: number,
): LineSet {
  let added = 0;
  let removed = 0;
  for (const hunk of hunks) {
    for (const line of hunk.lines) {
      if (line.kind === "added") added++;
      if (line.kind === "removed") removed++;
    }
  }
  const oldCount = lineCount - added + removed;
  return {
    has: (key) => {
      const match = /^(old|new):(\d+)$/.exec(key);
      if (!match) return false;
      const line = Number(match[2]);
      return line >= 1 && line <= (match[1] === "old" ? oldCount : lineCount);
    },
  };
}

/** The rows of a file as {@link layoutFile} lays them out, with the gaps of `state` woven in when the file's text has a source. */
export function layoutWithGaps(
  hunks: readonly Hunk[],
  expanded: ReadonlySet<string>,
  state: GapState | null,
  limit: number = ROW_LIMIT,
  options: CollapseOptions = {},
): FileLayout {
  const base = collapseRuns(hunkRows(hunks), expanded, options);
  const rows = state === null ? base : withGaps(base, hunks, state);
  return {
    rows: rows.slice(0, limit),
    remaining: Math.max(0, rows.length - limit),
  };
}
