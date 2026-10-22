/** The most lines a message box shows before it scrolls inside. */
export const COMPOSER_MAX_ROWS = 6;

/** What a key press in a message box does: send the message, do nothing, or let the textarea take the key (a new line for Shift+Enter). */
export type ComposerKey = "send" | "swallow" | "pass";

export interface ComposerKeyInput {
  key: string;
  shiftKey: boolean;
  /** True while an input method still composes the text, and for the key that ends the composition. */
  isComposing: boolean;
  pending: boolean;
  text: string;
}

export function composerKey(input: ComposerKeyInput): ComposerKey {
  if (input.key !== "Enter" || input.shiftKey) return "pass";
  if (input.isComposing) return "pass";
  // An Enter that cannot send must not add a new line either, or the box grows with empty lines.
  if (input.pending || input.text.trim() === "") return "swallow";
  return "send";
}

export interface ComposerMetrics {
  /** The textarea's `scrollHeight` with its height set to `auto`: the content and the padding. */
  scrollHeight: number;
  lineHeight: number;
  paddingY: number;
  borderY: number;
  maxRows?: number;
}

/** The height that shows all of the text from one line up to `maxRows` lines, and whether the box must scroll for the rest. */
export function composerHeight(metrics: ComposerMetrics): {
  height: number;
  scrolls: boolean;
} {
  const { scrollHeight, lineHeight, paddingY, borderY } = metrics;
  const rows = metrics.maxRows ?? COMPOSER_MAX_ROWS;
  const least = lineHeight + paddingY;
  const most = lineHeight * rows + paddingY;
  const inner = Math.min(Math.max(scrollHeight, least), most);
  return { height: inner + borderY, scrolls: scrollHeight > most };
}

/** A computed-style length such as `19.25px`; `fallback` for `normal`, an empty string or any other value. */
export function parsePx(value: string, fallback: number): number {
  const number = Number.parseFloat(value);
  return value.endsWith("px") && Number.isFinite(number) ? number : fallback;
}

/** A `normal` line height is about 1.2 times the font size in every browser. */
export function lineHeightOf(lineHeight: string, fontSize: string): number {
  return parsePx(lineHeight, parsePx(fontSize, 16) * 1.2);
}
