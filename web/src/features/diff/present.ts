import type {
  ChangeKind,
  ComparedFile,
  ComparedState,
  Comparison,
} from "@/api/diff";
import type { LineRow } from "./layout";

export interface ChangeBadge {
  letter: string;
  /** Screen readers hear the kind, since the letter alone is terse. */
  label: string;
  classes: string;
}

const BADGES: Record<ChangeKind, ChangeBadge> = {
  added: {
    letter: "A",
    label: "Added",
    classes: "bg-primary-soft text-primary-text",
  },
  modified: {
    letter: "M",
    label: "Modified",
    classes: "bg-sunken text-text-muted",
  },
  deleted: {
    letter: "D",
    label: "Deleted",
    classes: "bg-danger-soft text-danger",
  },
  renamed: {
    letter: "R",
    label: "Renamed",
    classes: "bg-secondary-soft text-secondary-text",
  },
};

export function changeBadge(kind: ChangeKind): ChangeBadge {
  return BADGES[kind];
}

/** W06 writes removals with U+2212 and leaves out a count that is zero. */
export function lineCounts(
  file: Pick<ComparedFile, "added_lines" | "removed_lines">,
): {
  added: string | null;
  removed: string | null;
} {
  return {
    added: file.added_lines > 0 ? `+${String(file.added_lines)}` : null,
    removed: file.removed_lines > 0 ? `−${String(file.removed_lines)}` : null,
  };
}

/** The words beside the path in the diff head, as W08 writes `new file`. */
export function changeNote(file: ComparedFile): string | null {
  if (file.change === "renamed" && file.previous_path !== null) {
    return `renamed from ${file.previous_path}`;
  }
  if (file.change === "added") return "new file";
  if (file.change === "deleted") return "deleted";
  return null;
}

/** The line over a binary file's hashes, which names its change kind since the hashes alone do not. */
export function binaryNote(kind: ChangeKind): string {
  return `${BADGES[kind].label} binary file, compared by hash.`;
}

/** The working files of a workspace against the revision it is based on: live edits, never history. */
export function isWorkingChanges(comparison: Comparison): boolean {
  const { from, to } = comparison;
  return (
    from.kind === "revision" && to.kind === "workspace" && to.base === from.id
  );
}

export function comparisonTitle(comparison: Comparison): string {
  const { from, to } = comparison;
  if (isWorkingChanges(comparison)) return `Working changes in ${to.id}`;
  if (to.kind === "workspace")
    return `Working files in ${to.id} against ${from.id}`;
  return `Changes from ${from.id} to ${to.id}`;
}

export interface Crumb {
  label: string;
  /** A screen path; the last crumb has none. */
  href: string | null;
  mono: boolean;
}

export function comparisonCrumbs(
  slug: string,
  comparison: Comparison,
): Crumb[] {
  const { from, to } = comparison;
  if (to.kind === "workspace") {
    return [
      { label: "Activity", href: `/p/${slug}/activity`, mono: false },
      { label: to.id, href: null, mono: true },
    ];
  }
  if (to.kind === "proposal_version") {
    return [
      { label: "Proposals", href: `/p/${slug}/proposals`, mono: false },
      { label: to.id, href: null, mono: true },
    ];
  }
  return [
    { label: "Repository", href: `/p/${slug}/repository`, mono: false },
    { label: `${from.id} → ${to.id}`, href: null, mono: true },
  ];
}

/** The label of one side in the compare row: the workspace's base is `base r7`, its working files are `Working files`. */
export function stateLabel(
  state: ComparedState,
  comparison: Comparison,
): string {
  if (state.kind === "workspace") return "Working files";
  if (state === comparison.from && isWorkingChanges(comparison)) {
    return `base ${state.id}`;
  }
  return state.id;
}

function plural(count: number, word: string): string {
  return `${String(count)} ${word}${count === 1 ? "" : "s"}`;
}

/** `Not in main. T-12 · frontend/ui · 4 files · +79 −67` for working changes; the counts alone otherwise. */
export function comparisonNote(
  comparison: Comparison,
  files: readonly ComparedFile[],
): string {
  const added = files.reduce((sum, f) => sum + f.added_lines, 0);
  const removed = files.reduce((sum, f) => sum + f.removed_lines, 0);
  const counts = `${plural(files.length, "file")} · +${String(added)} −${String(removed)}`;
  const { to } = comparison;
  if (to.kind !== "workspace") return counts;
  const who = [to.task, to.worker].filter(
    (part): part is string => part !== null,
  );
  return `Not in main. ${[...who, counts].join(" · ")}`;
}

/** The note beside a working-changes diff that says where the files come from and that they are not history. */
export function workingNote(comparison: Comparison): string {
  const task = comparison.to.task;
  const until =
    task === null
      ? "nothing here is in main until it is integrated."
      : `nothing here is in main until ${task}'s proposal is integrated.`;
  return `Working files are read from disk when you open this page. They are not history: ${until}`;
}

const LINE_WORDS: Record<LineRow["kind"], string> = {
  add: "Added line",
  remove: "Removed line",
  context: "Unchanged line",
};

/** The name a screen reader gives a line: its kind, its numbers and its text. */
export function lineLabel(row: LineRow): string {
  const numbers = [
    row.oldNo === null ? null : `old ${String(row.oldNo)}`,
    row.newNo === null ? null : `new ${String(row.newNo)}`,
  ].filter((part): part is string => part !== null);
  return `${LINE_WORDS[row.kind]}, ${numbers.join(", ")}: ${row.text}`;
}

export function hiddenLinesText(hidden: number): string {
  return `Show ${plural(hidden, "unchanged line")}`;
}

export function moreRowsText(remaining: number): string {
  return `Show more · ${plural(remaining, "more row")}`;
}
