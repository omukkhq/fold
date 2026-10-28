import type { LiveWorkspace } from "../../live/envelope";

type ChangedFile = LiveWorkspace["files"][number];

export const COLLAPSED_FILES = 3;

export function visibleFiles(
  files: readonly ChangedFile[],
  expanded: boolean,
): { shown: ChangedFile[]; hidden: number } {
  if (expanded || files.length <= COLLAPSED_FILES) {
    return { shown: [...files], hidden: 0 };
  }
  return {
    shown: files.slice(0, COLLAPSED_FILES),
    hidden: files.length - COLLAPSED_FILES,
  };
}

export interface ChangeKind {
  letter: string;
  label: string;
  /** The letter's fill and colour: an added file in green, a deleted one in red, the rest quiet. */
  tone: string;
}

const QUIET = "bg-fill text-text-2";

const KIND: Record<ChangedFile["change"], ChangeKind> = {
  added: { letter: "A", label: "Added", tone: "bg-ok-soft text-ok" },
  modified: { letter: "M", label: "Modified", tone: QUIET },
  deleted: { letter: "D", label: "Deleted", tone: "bg-bad-soft text-bad" },
  renamed: { letter: "R", label: "Renamed", tone: QUIET },
};

export function changeKind(change: ChangedFile["change"]): ChangeKind {
  return KIND[change];
}

/** A rename shows both paths. */
export function fileLabel(file: ChangedFile): string {
  return file.previous_path === null
    ? file.path
    : `${file.previous_path} → ${file.path}`;
}
