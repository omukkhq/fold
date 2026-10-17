import type { SyntaxLine } from "@/syntax/highlight";
import type { Language } from "@/syntax/languages";
import { highlightRequest, type HighlightRequest } from "@/syntax/useHighlight";
import type { DiffRow } from "./layout";

/** The shown lines of a diff as code to highlight, and where each row's line sits in it. */
export interface DiffSyntax {
  request: HighlightRequest;
  /** A line row's key to its segment and line. */
  at: ReadonlyMap<string, readonly [number, number]>;
}

/**
 * Each hunk is two segments: the old side (unchanged and removed lines) and the new side (unchanged and added lines),
 * so each side reads as the code it was. Only the rows on screen are taken, and an unchanged line is highlighted on
 * the new side.
 */
export function diffSyntax(
  rows: readonly DiffRow[],
  language: Language | null,
): DiffSyntax {
  const segments: string[][] = [];
  const at = new Map<string, readonly [number, number]>();
  let side: { old: string[]; new: string[]; first: number } | null = null;
  for (const row of rows) {
    if (row.type === "hunk") {
      side = null;
      continue;
    }
    if (row.type !== "line") continue;
    if (side === null) {
      side = { old: [], new: [], first: segments.length };
      segments.push(side.old, side.new);
    }
    if (row.kind === "removed") {
      at.set(row.key, [side.first, side.old.length]);
    } else {
      at.set(row.key, [side.first + 1, side.new.length]);
      side.new.push(row.text);
    }
    if (row.kind !== "added") side.old.push(row.text);
  }
  return { request: highlightRequest(language, segments), at };
}

/** A row's tokens, or null while the diff is plain. */
export function rowTokens(
  syntax: DiffSyntax,
  tokens: readonly SyntaxLine[][] | null,
  key: string,
): SyntaxLine | null {
  const where = syntax.at.get(key);
  if (!tokens || !where) return null;
  return tokens[where[0]]?.[where[1]] ?? null;
}
