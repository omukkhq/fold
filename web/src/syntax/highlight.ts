import type { GrammarState, HighlighterCore } from "shiki/core";
import type { Language } from "./languages";

/** A run of a line in one colour; `color` is null where the text takes the line's own colour. */
export interface SyntaxToken {
  content: string;
  color: string | null;
}

/** One line's tokens. */
export type SyntaxLine = SyntaxToken[];

/** Past either limit the code shows as plain text: highlighting it would cost more than it helps. */
export const MAX_CHARS = 200_000;
export const MAX_LINES = 5_000;
/** A longer line, such as minified code, is left plain while the lines around it are highlighted. */
export const MAX_LINE_LENGTH = 1_000;
/** The lines tokenized at a time; the work yields to the event loop once a chunk ends past the time budget, so a large file never holds the page still. */
export const CHUNK_LINES = 100;
export const YIELD_AFTER_MS = 8;

type Shiki = typeof import("./shiki");

let shiki: Promise<{ module: Shiki; highlighter: HighlighterCore }> | null =
  null;
const languages = new Map<Language, Promise<void>>();

/** Shiki, its engine, fold's theme and the grammar load on first use; a failed load is forgotten so the next code tries again. */
async function ready(
  language: Language,
): Promise<{ module: Shiki; highlighter: HighlighterCore }> {
  shiki ??= import("./shiki")
    .then(async (module) => ({
      module,
      highlighter: await module.createCore(),
    }))
    .catch((error: unknown) => {
      shiki = null;
      throw error;
    });
  const loading = shiki;
  let grammar = languages.get(language);
  if (grammar === undefined) {
    grammar = loading
      .then(({ module, highlighter }) =>
        module.loadGrammar(highlighter, language),
      )
      .catch((error: unknown) => {
        languages.delete(language);
        throw error;
      });
    languages.set(language, grammar);
  }
  await grammar;
  return loading;
}

function yieldToPage(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 0));
}

/** Whether the segments are small enough to highlight. */
export function withinLimits(
  segments: readonly (readonly string[])[],
): boolean {
  let lines = 0;
  let chars = 0;
  for (const segment of segments) {
    lines += segment.length;
    for (const line of segment) chars += line.length + 1;
    if (lines > MAX_LINES || chars > MAX_CHARS) return false;
  }
  return true;
}

export interface HighlightOptions {
  /** Stops the work between chunks; the result is then null. */
  signal?: AbortSignal;
  chunkLines?: number;
  yieldAfterMs?: number;
}

/**
 * Tokenizes each segment from a fresh grammar state, so a segment such as one hunk of a diff never inherits a comment
 * opened in another; within a segment the state carries across chunks. The result has a line of tokens for every
 * input line. Null when there is no language, the code is past the limits, the work was stopped, or the grammar failed
 * to load: the caller then shows plain text.
 */
export async function highlightSegments(
  language: Language | null,
  segments: readonly (readonly string[])[],
  {
    signal,
    chunkLines = CHUNK_LINES,
    yieldAfterMs = YIELD_AFTER_MS,
  }: HighlightOptions = {},
): Promise<SyntaxLine[][] | null> {
  if (language === null || !withinLimits(segments)) return null;
  let loaded: { module: Shiki; highlighter: HighlighterCore };
  try {
    loaded = await ready(language);
  } catch {
    return null;
  }
  const out: SyntaxLine[][] = [];
  let since = performance.now();
  for (const segment of segments) {
    const lines: SyntaxLine[] = [];
    let state: GrammarState | undefined;
    for (let start = 0; start < segment.length; start += chunkLines) {
      if (signal?.aborted) return null;
      const chunk = loaded.module.tokenizeLines(
        loaded.highlighter,
        language,
        segment.slice(start, start + chunkLines),
        state,
        MAX_LINE_LENGTH,
      );
      lines.push(...chunk.lines);
      state = chunk.state;
      if (performance.now() - since >= yieldAfterMs) {
        await yieldToPage();
        since = performance.now();
      }
    }
    out.push(lines);
  }
  return signal?.aborted ? null : out;
}
