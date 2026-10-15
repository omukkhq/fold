import type {
  ComparedFile,
  ComparedState,
  Comparison,
  FileDiff,
  Hunk,
  HunkLine,
} from "@/api/diff";

/** A line before the hunk numbers it; the text gets the terminator the daemon keeps. */
export type Draft = Pick<HunkLine, "kind" | "text">;

export const ctx = (text: string): Draft => ({
  kind: "context",
  text: `${text}\n`,
});
export const add = (text: string): Draft => ({
  kind: "added",
  text: `${text}\n`,
});
export const del = (text: string): Draft => ({
  kind: "removed",
  text: `${text}\n`,
});

/** A hunk as the daemon sends it: each line numbered from the hunk's start on the sides it is on. */
export function hunk(
  oldStart: number,
  newStart: number,
  drafts: readonly Draft[],
): Hunk {
  let oldNo = oldStart;
  let newNo = newStart;
  const lines: HunkLine[] = drafts.map((draft) => ({
    ...draft,
    old_no: draft.kind === "added" ? null : oldNo++,
    new_no: draft.kind === "removed" ? null : newNo++,
    no_newline: false,
  }));
  return {
    old_start: oldStart,
    old_lines: lines.filter((l) => l.kind !== "added").length,
    new_start: newStart,
    new_lines: lines.filter((l) => l.kind !== "removed").length,
    lines,
  };
}

export function comparedFile(
  path: string,
  change: ComparedFile["change"],
  extra: Partial<ComparedFile> = {},
): ComparedFile {
  return {
    path,
    previous_path: null,
    change,
    added_lines: 1,
    removed_lines: 1,
    binary: false,
    old_hash: change === "added" ? null : `old-${path}`,
    new_hash: change === "deleted" ? null : `new-${path}`,
    ...extra,
  };
}

export function state(
  kind: ComparedState["kind"],
  id: string,
  extra: Partial<ComparedState> = {},
): ComparedState {
  return { kind, id, base: null, task: null, worker: null, ...extra };
}

/** The working files of `ws-7` (task T-12, worker frontend/ui) against its base `r7`. */
export const WORKING = {
  from: state("revision", "r7"),
  to: state("workspace", "ws-7", {
    base: "r7",
    task: "T-12",
    worker: "frontend/ui",
  }),
};

export function comparison(
  files: ComparedFile[],
  extra: Partial<Comparison> = {},
): Comparison {
  const added = files.reduce((n, f) => n + f.added_lines, 0);
  const removed = files.reduce((n, f) => n + f.removed_lines, 0);
  return {
    source: "working",
    ...WORKING,
    files,
    totals: { files: files.length, added_lines: added, removed_lines: removed },
    warnings: [],
    ...extra,
  };
}

/** The hunks of `file` as the daemon answers them. */
export function fileDiff(
  file: ComparedFile,
  hunks: Hunk[],
  extra: Partial<FileDiff> = {},
): FileDiff {
  return {
    source: "working",
    path: file.path,
    previous_path: file.previous_path,
    change: file.change,
    binary: file.binary,
    too_large: false,
    old_hash: file.old_hash,
    new_hash: file.new_hash,
    hunks,
    ...extra,
  };
}
