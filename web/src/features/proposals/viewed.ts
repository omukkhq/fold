import type { ComparedFile, FileWarning } from "@/api/diff";
import type { ChangeRange } from "@/api/proposals";
import { listedFiles, shownEntry } from "@/features/diff/layout";

/** The part of the browser's storage the viewed marks use; null where the browser gives none. */
export interface MarkStore {
  getItem: (key: string) => string | null;
  setItem: (key: string, value: string) => void;
}

/** The viewed marks belong to one browser and one change: a file viewed in the change since v1 is not viewed in the full change, nor in the next version. */
export function viewedKey(
  slug: string,
  id: string,
  range: ChangeRange,
): string {
  const since = range.since === null ? "base" : `v${String(range.since)}`;
  return `fold.viewed.${slug}.${id}.${since}..v${String(range.version)}`;
}

/** The viewed paths, empty when the browser keeps none or refuses to read them. */
export function readViewed(store: MarkStore | null, key: string): Set<string> {
  try {
    const raw = store?.getItem(key) ?? null;
    const parsed: unknown = raw === null ? [] : JSON.parse(raw);
    return new Set(
      Array.isArray(parsed)
        ? parsed.filter((p): p is string => typeof p === "string")
        : [],
    );
  } catch {
    return new Set();
  }
}

/** Keeps the marks when the browser lets it; a refused write keeps them only on screen. */
export function writeViewed(
  store: MarkStore | null,
  key: string,
  viewed: ReadonlySet<string>,
): void {
  try {
    store?.setItem(key, JSON.stringify([...viewed].sort()));
  } catch {
    // The marks stay on screen until the page closes.
  }
}

export function toggleViewed(
  viewed: ReadonlySet<string>,
  path: string,
): Set<string> {
  const next = new Set(viewed);
  if (next.has(path)) next.delete(path);
  else next.add(path);
  return next;
}

/** The file whose diff the panel shows for `selectedPath`; null when it shows a warning, which has no viewed mark, or nothing. */
export function shownPath(
  comparison: {
    files: readonly ComparedFile[];
    warnings: readonly FileWarning[];
  },
  selectedPath: string | undefined,
): string | null {
  const shown = shownEntry(
    listedFiles(comparison.files),
    comparison.warnings,
    selectedPath,
  );
  return shown?.kind === "file" ? shown.file.path : null;
}

/** The viewed marks with the file last opened. */
export interface OpenMarks {
  opened: string | null;
  viewed: ReadonlySet<string>;
}

function markOpened(marks: OpenMarks, path: string): OpenMarks {
  if (marks.opened === path && marks.viewed.has(path)) return marks;
  const viewed = marks.viewed.has(path)
    ? marks.viewed
    : new Set([...marks.viewed, path]);
  return { opened: path, viewed };
}

/** Opening a file marks it viewed. The shown file opens only when it changes, so a re-render, a live update or a refetch keeps a file the reader unchecked unviewed. The same marks come back when nothing changes. */
export function showFile(marks: OpenMarks, shown: string | null): OpenMarks {
  if (shown === marks.opened) return marks;
  if (shown === null) return { ...marks, opened: null };
  return markOpened(marks, shown);
}

/** A reader who selects a file opens it, even when it is already shown; null selects a file without a viewed mark. */
export function selectFile(marks: OpenMarks, path: string | null): OpenMarks {
  return path === null ? marks : markOpened(marks, path);
}

/** `2 of 33 viewed`, counting only the files the change lists. */
export function viewedText(
  viewed: ReadonlySet<string>,
  paths: readonly string[],
): string {
  const n = paths.filter((p) => viewed.has(p)).length;
  return `${String(n)} of ${String(paths.length)} viewed`;
}
