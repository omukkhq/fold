import type { components } from "@/api/schema";

/** A line of the generated diff, or the header row of a hunk. */
export type DiffLineKind = components["schemas"]["DiffLineKind"] | "hunk";

export interface DiffLineStyle {
  sign: string;
  /** Screen readers hear the kind, since the sign and the fill are only visual. */
  description: string;
  row: string;
  signClass: string;
  /** A hunk header spans the row without number and sign columns. */
  header: boolean;
}

const STYLES: Record<DiffLineKind, DiffLineStyle> = {
  added: {
    sign: "+",
    description: "Added line",
    row: "bg-diff-add",
    signClass: "text-primary-text",
    header: false,
  },
  removed: {
    // A removal is signed with the minus sign, U+2212, not a hyphen.
    sign: "\u2212",
    description: "Removed line",
    row: "bg-diff-del",
    signClass: "text-danger",
    header: false,
  },
  context: {
    sign: " ",
    description: "Unchanged line",
    row: "bg-diff-context",
    signClass: "text-text-muted",
    header: false,
  },
  hunk: {
    sign: "",
    description: "Hunk header",
    row: "bg-diff-hunk",
    signClass: "text-text-muted",
    header: true,
  },
};

export function diffLineStyle(kind: DiffLineKind): DiffLineStyle {
  return STYLES[kind];
}

/**
 * A selected row has a 3px primary edge on its left, drawn inside the row as an inner stroke so the number columns keep
 * their place.
 */
const SELECTED_ROW =
  "relative bg-diff-selected before:absolute before:inset-y-0 before:left-0 before:w-3 before:bg-primary";

/** The row fill: a selected row overrides the kind's fill. */
export function diffRowClass(kind: DiffLineKind, selected: boolean): string {
  return selected ? SELECTED_ROW : STYLES[kind].row;
}

/** A selected row's line numbers are in primary text and semibold. */
export function diffNumberClass(selected: boolean): string {
  return selected ? "font-semibold text-primary-text" : "text-text-muted";
}
