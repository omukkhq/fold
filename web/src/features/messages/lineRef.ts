import type { MessageLines } from "@/api/messages";
import { projectPath, withQuery } from "@/routes/paths";

/** One line of a question's excerpt with its number in the file. */
export interface QuotedLine {
  number: number;
  text: string;
}

/** The head of the lines a question is about: `web/src/board.ts · v2 · lines 2–4 · new side`. */
export function lineRefText(lines: MessageLines): string {
  const range =
    lines.start_line === lines.end_line
      ? `line ${String(lines.start_line)}`
      : `lines ${String(lines.start_line)}–${String(lines.end_line)}`;
  return `${lines.path} · v${String(lines.version)} · ${range} · ${lines.side} side`;
}

/** The excerpt's lines, numbered from the first line of the run. */
export function quotedLines(lines: MessageLines): QuotedLine[] {
  return lines.excerpt
    .split("\n")
    .map((text, index) => ({ number: lines.start_line + index, text }));
}

/** The note under an excerpt that holds only the first lines of its run, or null. */
export function cutNote(lines: MessageLines): string | null {
  if (!lines.excerpt_cut) return null;
  const shown = quotedLines(lines).length;
  const total = lines.end_line - lines.start_line + 1;
  return `The first ${String(shown)} of ${String(total)} lines.`;
}

/** The proposal's diff at the version and file the lines belong to. */
export function lineRefHref(
  slug: string,
  proposal: string,
  lines: MessageLines,
): string {
  return withQuery(projectPath(slug, "proposals", proposal), {
    version: String(lines.version),
    path: lines.path,
  });
}
