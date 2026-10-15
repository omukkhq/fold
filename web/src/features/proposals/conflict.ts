import type {
  ConflictEntry,
  ConflictFile,
  ConflictKind,
  ConflictReport,
} from "@/api/proposals";
import { displayName } from "./present";

const KIND_TEXTS: Record<ConflictKind, string> = {
  both_modified_text: "Text conflict: both sides changed the same lines",
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
    { label: `base ${report.base}`, entry: entryText(file.base) },
    { label: `main ${report.main}`, entry: entryText(file.main) },
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
      label: `base ${report.base}`,
      entry: `tree ${report.trees.base.slice(0, 7)}`,
    },
    {
      label: `main ${report.main}`,
      entry: `tree ${report.trees.main.slice(0, 7)}`,
    },
    { label: versionRef, entry: `tree ${report.trees.version.slice(0, 7)}` },
  ];
}

/** What the author does next; the daemon sent them the same report. */
export function nextStepText(author: string, version: number): string {
  return `Next: ${displayName(author)} runs sync_workspace, resolves, and sends v${String(version + 1)}.`;
}
