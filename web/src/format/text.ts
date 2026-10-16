/** The longest text of one body the daemon takes: a review, a comment, a request description. */
export const MAX_TEXT_CHARS = 20_000;

/** Characters as the daemon counts them: Unicode scalar values, so a surrogate pair is one. */
export function charCount(text: string): number {
  return text.replace(/[\uD800-\uDBFF][\uDC00-\uDFFF]/g, "_").length;
}
