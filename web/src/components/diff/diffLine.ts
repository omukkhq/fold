export type DiffLineKind = "add" | "remove" | "context" | "hunk";

export interface DiffLineStyle {
  sign: string;
  /** Screen readers hear the kind, since the sign and the fill are only visual. */
  description: string;
  row: string;
  signClass: string;
  /** A hunk header spans the row without number and sign columns, as W06 draws it. */
  header: boolean;
}

const STYLES: Record<DiffLineKind, DiffLineStyle> = {
  add: {
    sign: "+",
    description: "Added line",
    row: "bg-diff-add",
    signClass: "text-primary-text",
    header: false,
  },
  remove: {
    // W06 signs a removal with the minus sign, U+2212, not a hyphen.
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

/** The row fill: a selected row overrides the kind's fill. */
export function diffRowClass(kind: DiffLineKind, selected: boolean): string {
  return selected ? "bg-diff-selected" : STYLES[kind].row;
}
