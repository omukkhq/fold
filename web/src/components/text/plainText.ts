/** The characters a one-line preview of a message keeps: more than a list row ever shows, so the row's own cut ends it. */
export const PREVIEW_CHARS = 160;

/** A code span: a run of backticks, its text, and the same run again. */
const CODE_SPAN = /(`+)([\s\S]*?[^`])\1(?!`)/g;

/** Line starts that only shape a block: a fence, a heading, a quote, a list item and its task box. */
const FENCE = /^\s*(```|~~~)/;
const LINE_MARKS =
  /^\s*(?:>\s?)*(?:#{1,6}\s+|(?:[-*+]|\d+[.)])\s+(?:\[[ xX]\]\s+)?)?/;

/** Inline marks, in the order they are taken off; underscores count only outside a word, so snake_case names keep theirs. */
const INLINE: readonly [RegExp, string][] = [
  [/!\[([^\]]*)\]\([^)]*\)/g, "$1"],
  [/\[([^\]]+)\]\([^)]*\)/g, "$1"],
  [/\[([^\]]+)\]\[[^\]]*\]/g, "$1"],
  [/<((?:https?:|mailto:)[^>\s]+)>/gi, "$1"],
  [/\*\*(?=\S)([\s\S]*?\S)\*\*/g, "$1"],
  [/(^|\W)__(?=\S)([\s\S]*?\S)__(?!\w)/g, "$1$2"],
  [/~~(?=\S)([\s\S]*?\S)~~/g, "$1"],
  [/(^|[^*\w])\*(?=\S)([^*]*?\S)\*(?![*\w])/g, "$1$2"],
  [/(^|\W)_(?=\S)([^_]*?\S)_(?!\w)/g, "$1$2"],
  // A cut can leave half a pair behind.
  [/\*\*|__|~~|`/g, ""],
];

function plainInline(text: string): string {
  return INLINE.reduce(
    (out, [pattern, replacement]) => out.replace(pattern, replacement),
    text,
  );
}

/** One line without its block marks and inline marks; the text of a code span stays as it was written. */
function plainLine(line: string): string {
  const text = line.replace(LINE_MARKS, "");
  let out = "";
  let at = 0;
  for (const match of text.matchAll(CODE_SPAN)) {
    out += plainInline(text.slice(at, match.index));
    out += (match[2] ?? "").trim();
    at = match.index + match[0].length;
  }
  return out + plainInline(text.slice(at));
}

/**
 * Markdown as the plain words a reader sees, on one line: emphasis, code backticks, link syntax (its text stays),
 * heading, quote and list marks and code fences go; mentions and ids such as T-14 stay as written.
 */
export function plainText(markdown: string): string {
  return markdown
    .split("\n")
    .filter((line) => !FENCE.test(line))
    .map(plainLine)
    .join(" ")
    .replace(/\s+/g, " ")
    .trim();
}

/** The plain text cut to `max` characters at a word when one is near, with an ellipsis; the cut comes after the marks are gone, so it never splits one. */
export function plainPreview(markdown: string, max = PREVIEW_CHARS): string {
  const text = plainText(markdown);
  if (text.length <= max) return text;
  const space = text.lastIndexOf(" ", max);
  const cut = text.slice(0, space > max / 2 ? space : max).trimEnd();
  return `${cut}…`;
}
