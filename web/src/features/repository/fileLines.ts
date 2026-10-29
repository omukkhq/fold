import { ROW_LIMIT, type LineRow } from "@/features/diff/layout";
import type { DiffSelection } from "@/features/diff/selection";

/** A text file as numbered rows, so its lines take the same selection, marks and composer as a diff. A line ends at `\r\n`, at a lone `\r` or at `\n`, as the daemon numbers the lines a request keeps. A final line terminator ends the last line and starts no row; a file without one marks its last line. */
export function fileRows(text: string): LineRow[] {
  if (text === "") return [];
  const lines = text.split(/\r\n|\r|\n/);
  const ended = lines.at(-1) === "";
  if (ended) lines.pop();
  return lines.map((line, i) => ({
    type: "line",
    key: `l${String(i + 1)}`,
    kind: "context",
    oldNo: null,
    newNo: i + 1,
    text: line,
    noNewline: !ended && i === lines.length - 1,
  }));
}

/** How many rows the file view shows first: one page, or as many pages as reach `line`, so a link to a line opens with it drawn. */
export function firstRowLimit(line: number | undefined): number {
  return line === undefined
    ? ROW_LIMIT
    : Math.max(1, Math.ceil(line / ROW_LIMIT)) * ROW_LIMIT;
}

/** The line a link opened the file at, selected so it stands out; null without one or past the end of the file. */
export function openedSelection(
  path: string,
  line: number | undefined,
  rows: readonly LineRow[],
): DiffSelection | null {
  return line === undefined || line > rows.length
    ? null
    : { path, side: "new", start_line: line, end_line: line };
}
