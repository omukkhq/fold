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

const KIND: Record<ChangedFile["change"], { letter: string; label: string }> = {
  added: { letter: "A", label: "Added" },
  modified: { letter: "M", label: "Modified" },
  deleted: { letter: "D", label: "Deleted" },
  renamed: { letter: "R", label: "Renamed" },
};

export function changeKind(change: ChangedFile["change"]): {
  letter: string;
  label: string;
} {
  return KIND[change];
}

/** A rename shows both paths. */
export function fileLabel(file: ChangedFile): string {
  return file.previous_path === null
    ? file.path
    : `${file.previous_path} → ${file.path}`;
}
