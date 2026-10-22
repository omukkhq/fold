/**
 * How a block of text uses the width it gets.
 *
 * `prose` is text a person or an agent wrote (evidence, review and message bodies, notes): it keeps its line breaks and
 * wraps, and a long unbroken run such as a backticked path breaks anywhere, so it never widens its box.
 * `lines` is the same text with runs of spaces folded, for a short excerpt.
 * `code` is a diff, a log or a file: each line stays one line, and the box around it scrolls on its own.
 */
export type TextKind = "prose" | "lines" | "code";

const FLOW: Record<TextKind, string> = {
  prose: "whitespace-pre-wrap wrap-anywhere",
  lines: "whitespace-pre-line wrap-anywhere",
  code: "whitespace-pre",
};

export function textFlow(kind: TextKind): string {
  return FLOW[kind];
}

/** The box around `code` text: it scrolls sideways itself, and as a flex or grid child it shrinks below its lines' width. */
export const CODE_BOX = "min-w-0 max-w-full overflow-x-auto";
