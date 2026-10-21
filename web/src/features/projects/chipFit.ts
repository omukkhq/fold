/**
 * How many of the chips fit on one line of `available` pixels, in order, with `gap` pixels between neighbours. When some do not fit, the `+N` chip of `moreWidth` pixels takes its place at the end of the line, so the count leaves room for it. The `+N` chip alone shows when not even the first chip fits beside it.
 */
export function fittingChips(
  widths: readonly number[],
  available: number,
  gap: number,
  moreWidth: number,
): number {
  if (lineWidth(widths, gap) <= available) {
    return widths.length;
  }
  for (let shown = widths.length - 1; shown > 0; shown -= 1) {
    const line = lineWidth(widths.slice(0, shown), gap) + gap + moreWidth;
    if (line <= available) {
      return shown;
    }
  }
  return 0;
}

function lineWidth(widths: readonly number[], gap: number): number {
  if (widths.length === 0) {
    return 0;
  }
  return (
    widths.reduce((sum, width) => sum + width, 0) + gap * (widths.length - 1)
  );
}

/** The text of the chip that stands for the chips that do not fit. */
export function moreLabel(hidden: number): string {
  return `+${String(hidden)}`;
}

/** What a screen reader and the tooltip say of the chips that do not fit. */
export function moreTitle(hidden: readonly string[]): string {
  const names = hidden.map((name) => `@${name}`).join(", ");
  return `${String(hidden.length)} more ${hidden.length === 1 ? "agent" : "agents"}: ${names}`;
}
