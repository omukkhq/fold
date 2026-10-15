import type {
  ConflictEntry,
  ConflictFile,
  ConflictHunk,
  ConflictKind,
  ConflictReport,
} from "@/api/proposals";
import { actorSegment, seg, type Segment } from "@/people/segments";

const KIND_TEXTS: Record<ConflictKind, string> = {
  text: "Text conflict: both sides changed the same lines",
  both_modified_binary: "Binary conflict: both sides changed the file",
  modify_delete: "One side changed the file and the other deleted it",
  add_add: "Both sides added a different file at this path",
  type_change: "The sides hold different kinds of entry at this path",
};

export function conflictKindText(kind: ConflictKind): string {
  return KIND_TEXTS[kind];
}

/** `file 3f2a9c1`, `symlink 77ab012`, or `nothing` where the side has no entry. */
export function entryText(entry: ConflictEntry | null): string {
  return entry ? `${entry.kind} ${entry.hash.slice(0, 7)}` : "nothing";
}

export interface SideLine {
  label: string;
  entry: string;
}

/** The three sides of one file, in the order the report names the trees. */
export function fileSides(
  file: ConflictFile,
  versionRef: string,
  report: ConflictReport,
): SideLine[] {
  return [
    { label: `base ${report.base_revision}`, entry: entryText(file.base) },
    { label: `main ${report.main_revision}`, entry: entryText(file.main) },
    { label: versionRef, entry: entryText(file.version) },
  ];
}

/** The three trees the merge read, by the revision or version each belongs to. */
export function reportTrees(
  report: ConflictReport,
  versionRef: string,
): SideLine[] {
  return [
    {
      label: `base ${report.base_revision}`,
      entry: `tree ${report.trees.base.slice(0, 7)}`,
    },
    {
      label: `main ${report.main_revision}`,
      entry: `tree ${report.trees.main.slice(0, 7)}`,
    },
    { label: versionRef, entry: `tree ${report.trees.version.slice(0, 7)}` },
  ];
}

export interface HunkSideView {
  /** `base · line 4`, `main · lines 4–6`, `P-3 v2 · after line 3` for an empty side. */
  label: string;
  /** The lines without their terminators. */
  lines: string[];
}

function at(start: number, count: number): string {
  if (count === 0) return `after line ${String(start - 1)}`;
  if (count === 1) return `line ${String(start)}`;
  return `lines ${String(start)}–${String(start + count - 1)}`;
}

/** The three sides of a text hunk in report order, each named with the lines it spans. */
export function hunkSides(
  hunk: ConflictHunk,
  versionRef: string,
): HunkSideView[] {
  return (
    [
      ["base", hunk.base],
      ["main", hunk.main],
      [versionRef, hunk.version],
    ] as const
  ).map(([name, side]) => ({
    label: `${name} · ${at(side.start, side.count)}`,
    lines: side.lines.map((line) => line.replace(/\r?\n$/, "")),
  }));
}

/** What the author does next; the daemon sent them the same report. */
export function nextStepText(author: string, version: number): Segment[] {
  return seg`Next: ${actorSegment(author)} runs sync_workspace, resolves, and sends v${String(version + 1)}.`;
}
