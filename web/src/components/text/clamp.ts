export type ClampLines = 2 | 3;

const CLAMPS: Record<ClampLines, string> = {
  2: "line-clamp-2",
  3: "line-clamp-3",
};

/** The class that cuts the text to its lines while it is collapsed; open, it shows every line. */
export function clampClass(lines: ClampLines, expanded: boolean): string {
  return expanded ? "" : CLAMPS[lines];
}

/** A collapsed text is cut when its full height is more than the height it shows; one pixel of rounding is not a cut. */
export function isCut(box: { scrollHeight: number; clientHeight: number }) {
  return box.scrollHeight - box.clientHeight > 1;
}

/** The toggle shows while the text is cut, and stays while it is open so that it can close again. */
export function showsToggle(expanded: boolean, cut: boolean): boolean {
  return expanded || cut;
}

export function toggleLabel(expanded: boolean): "More" | "Less" {
  return expanded ? "Less" : "More";
}
