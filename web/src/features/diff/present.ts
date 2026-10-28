import type {
  ChangeKind,
  ComparedFile,
  ComparedState,
  Comparison,
  FileWarning,
} from "@/api/diff";
import type { CheckpointSummary } from "@/api/tasks";
import { revisionCount, revisionsAhead } from "@/format/revisions";
import type { LineRow } from "./layout";
import { projectPath, withQuery } from "@/routes/paths";

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
    classes: "bg-accent-soft text-accent",
  },
  modified: {
    letter: "M",
    label: "Modified",
    classes: "bg-sunken text-text-2",
  },
  deleted: {
    letter: "D",
    label: "Deleted",
    classes: "bg-bad-soft text-bad",
  },
  renamed: {
    letter: "R",
    label: "Renamed",
    classes: "bg-warn-soft text-warn",
  },
  mode_changed: {
    letter: "T",
    label: "Mode changed",
    classes: "bg-sunken text-text-2",
  },
};

export function changeBadge(kind: ChangeKind): ChangeBadge {
  return BADGES[kind];
}

/** Removals are written with U+2212. */
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

/** The words beside the path in the diff head, such as `new file`. */
export function changeNote(file: ComparedFile): string | null {
  if (file.change === "renamed" && file.previous_path !== null) {
    return `renamed from ${file.previous_path}`;
  }
  if (file.change === "added") return "new file";
  if (file.change === "deleted") return "deleted";
  if (file.change === "mode_changed") return "mode changed";
  return null;
}

/** The line over a binary file's hashes, which names its change kind since the hashes alone do not. */
export function binaryNote(kind: ChangeKind): string {
  return `${BADGES[kind].label} binary file, compared by hash.`;
}

/** A side of a file larger than 1 MiB, which the daemon compares by hash only. */
export function tooLargeNote(kind: ChangeKind): string {
  return `${BADGES[kind].label} file over 1 MiB, compared by hash.`;
}

const WARNING_TEXT: Record<FileWarning["reason"], string> = {
  unstable: "unstable",
  too_large: "too large to read",
  special: "not a regular file",
  unreadable: "not readable",
  changed_during_snapshot: "changed while it was read",
};

/** The short mark beside a warned path in the file list. */
export function warningMark(warning: FileWarning): string {
  return WARNING_TEXT[warning.reason];
}

/** Why a warned file has no diff, in the panel where the diff would be. */
export function warningNote(warning: FileWarning): string {
  if (warning.reason === "unstable") {
    return "This file kept changing while fold read it, so no diff is shown. fold keeps the base version and reads the file again on the next change.";
  }
  return `fold could not read this file (${WARNING_TEXT[warning.reason]}), so no diff is shown. It keeps its base version.`;
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
    return `Changes from ${from.id} to the working files in ${to.id}`;
  return `Changes from ${from.id} to ${to.id}`;
}

/** The two compared states in the order the title, the crumb and the side line name them: the older one first. */
export function comparisonSides(
  comparison: Comparison,
): readonly [ComparedState, ComparedState] {
  return [comparison.from, comparison.to];
}

export interface Crumb {
  label: string;
  /** A screen path; the last crumb has none. */
  href: string | null;
}

export function comparisonCrumbs(
  slug: string,
  comparison: Comparison,
): Crumb[] {
  const { from, to } = comparison;
  if (to.kind === "workspace") {
    return [
      { label: "Activity", href: projectPath(slug, "activity") },
      { label: to.id, href: null },
    ];
  }
  return [
    { label: "Repository", href: projectPath(slug, "repository") },
    { label: `${from.id} → ${to.id}`, href: null },
  ];
}

/** A workspace comparison names its two sides under the title, which names the workspace alone; the title and the crumb already hold a pair of recorded states. */
export function namesSides(comparison: Comparison): boolean {
  return comparison.to.kind === "workspace";
}

/** The label of one side in the side line: the workspace's base is `base r7`, its working files are `Working files`. */
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
  return `Working files are read from disk when you open this page and again on each change. They are not history: ${until}`;
}

const LINE_WORDS: Record<LineRow["kind"], string> = {
  added: "Added line",
  removed: "Removed line",
  context: "Unchanged line",
};

export const NO_NEWLINE = "no newline at end of file";

/** The name a screen reader gives a line: its kind, its numbers and its text. */
export function lineLabel(row: LineRow): string {
  const numbers = [
    row.oldNo === null ? null : `old ${String(row.oldNo)}`,
    row.newNo === null ? null : `new ${String(row.newNo)}`,
  ].filter((part): part is string => part !== null);
  const tail = row.noNewline ? `, ${NO_NEWLINE}` : "";
  return `${LINE_WORDS[row.kind]}, ${numbers.join(", ")}: ${row.text}${tail}`;
}

export function hiddenLinesText(hidden: number): string {
  return `Show ${plural(hidden, "unchanged line")}`;
}

/** The name of a control that shows the run of lines `first` to `last`. */
export function showLinesText(first: number, last: number): string {
  return first === last
    ? `Show line ${String(first)}`
    : `Show lines ${String(first)}–${String(last)}`;
}

export const GAP_ERROR = "The unchanged lines could not load.";

/** The file head's control that shows every line of the file, or only the changes again. */
export function wholeFileText(whole: boolean): string {
  return whole ? "Show changes only" : "Show whole file";
}

export function moreRowsText(remaining: number): string {
  return `Show more · ${plural(remaining, "more row")}`;
}

export interface CompareOption {
  /** The `from` this option compares the workspace with. */
  from: string;
  text: string;
  href: string;
  current: boolean;
}

export interface CompareSources {
  /** The workspace's base revision. */
  base: string;
  /** Main's head and newest revisions; null until they are read. */
  main: {
    head: string;
    recent: { id: string; parent: string | null }[];
  } | null;
  checkpoints: readonly CheckpointSummary[];
}

function diffHref(slug: string, from: string, to: string): string {
  return withQuery(projectPath(slug, "diff"), { from, to });
}

/** The "Compare with" list for a workspace: its base, main when main moved past the base, and its checkpoints, newest first. */
export function compareOptions(
  slug: string,
  comparison: Comparison,
  sources: CompareSources,
): CompareOption[] {
  const to = comparison.to.id;
  const option = (from: string, text: string): CompareOption => ({
    from,
    text,
    href: diffHref(slug, from, to),
    current: comparison.from.id === from,
  });
  const options = [option(sources.base, `base ${sources.base} (default)`)];
  const main = sources.main;
  if (main !== null && main.head !== sources.base) {
    const ahead = revisionsAhead(main.recent, sources.base);
    options.push(
      option(main.head, `main ${main.head} · ${revisionCount(ahead)} ahead`),
    );
  }
  const checkpoints = [...sources.checkpoints].sort(
    (a, b) => b.created_at - a.created_at,
  );
  for (const checkpoint of checkpoints) {
    options.push(option(checkpoint.id, `checkpoint ${checkpoint.id}`));
  }
  return options;
}
