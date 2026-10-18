import {
  createHighlighterCore,
  type GrammarState,
  type HighlighterCore,
} from "shiki/core";
import { createOnigurumaEngine } from "shiki/engine/oniguruma";
import { bundledLanguages, type BundledLanguage } from "shiki/langs";
import onigUrl from "shiki/onig.wasm?url";
import type { SyntaxLine } from "./highlight";
import { FOLD_THEME, PLAIN, THEME_NAME } from "./theme";

/** Every grammar Shiki ships, each in its own chunk that loads when code first needs it. */
export const GRAMMARS = bundledLanguages;

export const ENGINE = {
  /**
   * The Oniguruma engine's WebAssembly, a file of the bundle. Shiki keeps a failed instantiation for the life of the
   * page, so it gets the bytes only once they have arrived, and a failed fetch is tried again with the next code.
   */
  async wasm(): Promise<ArrayBuffer> {
    const response = await fetch(onigUrl);
    if (!response.ok)
      throw new Error(`onig.wasm: HTTP ${String(response.status)}`);
    return response.arrayBuffer();
  },
};

export async function createCore(): Promise<HighlighterCore> {
  const wasm = await ENGINE.wasm();
  return createHighlighterCore({
    themes: [FOLD_THEME],
    langs: [],
    engine: createOnigurumaEngine(wasm),
  });
}

export async function loadGrammar(
  highlighter: HighlighterCore,
  language: BundledLanguage,
): Promise<void> {
  const module = await GRAMMARS[language]();
  await highlighter.loadLanguage(module.default);
}

/** Tokenizes consecutive lines from `state`, the state the lines before them ended in. */
export function tokenizeLines(
  highlighter: HighlighterCore,
  language: BundledLanguage,
  lines: readonly string[],
  state: GrammarState | undefined,
  maxLineLength: number,
): { lines: SyntaxLine[]; state: GrammarState | undefined } {
  const result = highlighter.codeToTokens(lines.join("\n"), {
    lang: language,
    theme: THEME_NAME,
    tokenizeMaxLineLength: maxLineLength,
    grammarState: state,
  });
  return {
    lines: lines.map((_, i) =>
      (result.tokens[i] ?? []).map((token) => ({
        content: token.content,
        // A line past the length limit comes back as one token without a colour.
        color: !token.color || token.color === PLAIN ? null : token.color,
      })),
    ),
    state: result.grammarState,
  };
}
