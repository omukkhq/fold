import type { ThemeRegistrationRaw } from "shiki/core";

export type SyntaxKind =
  | "keyword"
  | "string"
  | "comment"
  | "number"
  | "function"
  | "type"
  | "constant"
  | "punctuation";

export const THEME_NAME = "fold";

/** The colour of text that no scope colours; a token in it carries no colour of its own and inherits the line's. */
export const PLAIN = "var(--text)";

export function syntaxColor(kind: SyntaxKind): string {
  return `var(--syntax-${kind})`;
}

/** TextMate scopes by the token colour that draws them. A longer scope selector wins over a shorter one, so `keyword.operator` is punctuation while other keywords are keywords. */
const SCOPES: Record<SyntaxKind, string[]> = {
  keyword: [
    "keyword",
    "storage",
    "storage.type",
    "storage.modifier",
    "entity.name.tag",
    "markup.heading",
    "punctuation.definition.heading",
  ],
  string: [
    "string",
    "string.regexp",
    "punctuation.definition.string",
    "markup.inline.raw",
    "markup.fenced_code",
    "markup.inserted",
  ],
  comment: [
    "comment",
    "punctuation.definition.comment",
    "string.comment",
    "meta.diff.header",
    "meta.diff.range",
  ],
  number: ["constant.numeric"],
  function: [
    "entity.name.function",
    "support.function",
    "meta.function-call entity.name.function",
    "support.type.property-name",
    "markup.underline.link",
  ],
  type: [
    "entity.name.type",
    "entity.name.class",
    "entity.name.namespace",
    "entity.other.inherited-class",
    "support.type",
    "support.class",
    "entity.other.attribute-name",
  ],
  constant: [
    "constant",
    "constant.language",
    "constant.character",
    "constant.other",
    "support.constant",
    "variable.other.constant",
    "variable.other.enummember",
    "markup.deleted",
  ],
  punctuation: [
    "punctuation",
    "meta.brace",
    "keyword.operator",
    "punctuation.separator",
    "punctuation.terminator",
  ],
};

/** Shiki's theme for fold: every colour is a syntax token of the design, so the page's light or dark theme recolours code without a second highlight. */
export const FOLD_THEME: ThemeRegistrationRaw = {
  name: THEME_NAME,
  type: "light",
  fg: PLAIN,
  bg: "transparent",
  settings: [
    { settings: { foreground: PLAIN } },
    ...Object.entries(SCOPES).map(([kind, scope]) => ({
      scope,
      settings: { foreground: syntaxColor(kind as SyntaxKind) },
    })),
  ],
};
