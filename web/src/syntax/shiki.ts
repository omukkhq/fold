import {
  createHighlighterCore,
  type GrammarState,
  type HighlighterCore,
  type LanguageRegistration,
} from "shiki/core";
import { createJavaScriptRegexEngine } from "shiki/engine/javascript";
import type { SyntaxLine } from "./highlight";
import type { Language } from "./languages";
import { FOLD_THEME, PLAIN, THEME_NAME } from "./theme";

type Loader = () => Promise<{ default: LanguageRegistration[] }>;

/**
 * Each grammar in its own chunk. Every specifier is a literal so the bundler emits only these grammars; a computed
 * specifier or Shiki's full language map would put all of them in the build, which the daemon binary embeds.
 */
export const GRAMMARS = {
  c: () => import("shiki/langs/c.mjs"),
  cpp: () => import("shiki/langs/cpp.mjs"),
  csharp: () => import("shiki/langs/csharp.mjs"),
  css: () => import("shiki/langs/css.mjs"),
  dart: () => import("shiki/langs/dart.mjs"),
  diff: () => import("shiki/langs/diff.mjs"),
  docker: () => import("shiki/langs/docker.mjs"),
  elixir: () => import("shiki/langs/elixir.mjs"),
  go: () => import("shiki/langs/go.mjs"),
  graphql: () => import("shiki/langs/graphql.mjs"),
  hcl: () => import("shiki/langs/hcl.mjs"),
  html: () => import("shiki/langs/html.mjs"),
  ini: () => import("shiki/langs/ini.mjs"),
  java: () => import("shiki/langs/java.mjs"),
  javascript: () => import("shiki/langs/javascript.mjs"),
  json: () => import("shiki/langs/json.mjs"),
  jsonc: () => import("shiki/langs/jsonc.mjs"),
  jsx: () => import("shiki/langs/jsx.mjs"),
  kotlin: () => import("shiki/langs/kotlin.mjs"),
  lua: () => import("shiki/langs/lua.mjs"),
  make: () => import("shiki/langs/make.mjs"),
  markdown: () => import("shiki/langs/markdown.mjs"),
  php: () => import("shiki/langs/php.mjs"),
  proto: () => import("shiki/langs/proto.mjs"),
  python: () => import("shiki/langs/python.mjs"),
  ruby: () => import("shiki/langs/ruby.mjs"),
  rust: () => import("shiki/langs/rust.mjs"),
  scss: () => import("shiki/langs/scss.mjs"),
  shellscript: () => import("shiki/langs/shellscript.mjs"),
  sql: () => import("shiki/langs/sql.mjs"),
  svelte: () => import("shiki/langs/svelte.mjs"),
  swift: () => import("shiki/langs/swift.mjs"),
  terraform: () => import("shiki/langs/terraform.mjs"),
  toml: () => import("shiki/langs/toml.mjs"),
  tsx: () => import("shiki/langs/tsx.mjs"),
  typescript: () => import("shiki/langs/typescript.mjs"),
  vue: () => import("shiki/langs/vue.mjs"),
  xml: () => import("shiki/langs/xml.mjs"),
  yaml: () => import("shiki/langs/yaml.mjs"),
  zig: () => import("shiki/langs/zig.mjs"),
} satisfies Record<Language, Loader>;

export function createCore(): Promise<HighlighterCore> {
  return createHighlighterCore({
    themes: [FOLD_THEME],
    langs: [],
    // ES2018 patterns avoid the RegExp `v` flag: JavaScriptCore, the engine of Bun and Safari, returns wrong tokens for patterns compiled with it.
    engine: createJavaScriptRegexEngine({ forgiving: true, target: "ES2018" }),
  });
}

export async function loadGrammar(
  highlighter: HighlighterCore,
  language: Language,
): Promise<void> {
  const module = await GRAMMARS[language]();
  await highlighter.loadLanguage(module.default);
}

/** Tokenizes consecutive lines from `state`, the state the lines before them ended in. */
export function tokenizeLines(
  highlighter: HighlighterCore,
  language: Language,
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
