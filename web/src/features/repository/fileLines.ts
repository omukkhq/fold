import type { LineRow } from "@/features/diff/layout";

/** A text file as numbered rows, so its lines take the same selection, marks and composer as a diff. A final line terminator ends the last line and starts no row; a file without one marks its last line. */
export function fileRows(text: string): LineRow[] {
  if (text === "") return [];
  const lines = text.split(/\r?\n/);
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
