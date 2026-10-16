import type { ComparedFile, FileWarning, Hunk, HunkLineKind } from "@/api/diff";

/** An unchanged run longer than this many lines collapses behind an expander. */
export const COLLAPSE_THRESHOLD = 8;
/** The unchanged lines a collapsed run keeps visible next to each change. */
export const COLLAPSE_EDGE = 3;
/** A file with more rows than this shows the first rows and a "Show more" control. */
export const ROW_LIMIT = 1000;

export type HunkRow = { type: "hunk"; key: string; header: string };

export type LineRow = {
  type: "line";
  key: string;
  kind: HunkLineKind;
  oldNo: number | null;
  newNo: number | null;
  /** The line without its terminator. */
  text: string;
  /** The last line of a file that does not end with a line terminator. */
  noNewline: boolean;
};

export type ExpanderRow = { type: "expander"; key: string; hidden: number };

export type DiffRow = HunkRow | LineRow | ExpanderRow;

/** The header of a hunk, with U+2212 before the old range. */
export function hunkHeader(hunk: Hunk): string {
  return `@@ −${String(hunk.old_start)},${String(hunk.old_lines)} +${String(hunk.new_start)},${String(hunk.new_lines)} @@`;
}

/** The daemon keeps each line's terminator; the row shows the line without it. */
export function lineText(text: string): string {
  return text.replace(/\r?\n$/, "");
}

/** Every hunk as its header row and its line rows, with the line numbers the daemon sent. */
export function hunkRows(hunks: readonly Hunk[]): (HunkRow | LineRow)[] {
  const rows: (HunkRow | LineRow)[] = [];
  hunks.forEach((hunk, h) => {
    rows.push({ type: "hunk", key: `h${String(h)}`, header: hunkHeader(hunk) });
    hunk.lines.forEach((line, i) => {
      rows.push({
        type: "line",
        key: `h${String(h)}-${String(i)}`,
        kind: line.kind,
        oldNo: line.old_no,
        newNo: line.new_no,
        text: lineText(line.text),
        noNewline: line.no_newline,
      });
    });
  });
  return rows;
}

interface CollapseOptions {
  threshold?: number;
  edge?: number;
  /** The lines, as `side:line` keys, that end a review thread: a run that holds one stays open, so the thread is drawn under its line. */
  keep?: ReadonlySet<string>;
}

/** The `side:line` keys of the lines that end a thread on `path`. */
export function threadEnds(
  threads: readonly {
    range: { path: string; side: string; end_line: number };
  }[],
  path: string,
): Set<string> {
  return new Set(
    threads
      .filter((t) => t.range.path === path)
      .map((t) => `${t.range.side}:${String(t.range.end_line)}`),
  );
}

function holds(row: LineRow, keep: ReadonlySet<string>): boolean {
  return (
    (row.oldNo !== null && keep.has(`old:${String(row.oldNo)}`)) ||
    (row.newNo !== null && keep.has(`new:${String(row.newNo)}`))
  );
}

/**
 * Collapses each unchanged run longer than the threshold. A run keeps `edge` lines next to each change; a run at the start or the end of a hunk
 * keeps lines only on its changed side. An expander's key is in `expanded` once the reader opened it; a run that ends a thread never collapses.
 */
export function collapseRuns(
  rows: readonly (HunkRow | LineRow)[],
  expanded: ReadonlySet<string>,
  {
    threshold = COLLAPSE_THRESHOLD,
    edge = COLLAPSE_EDGE,
    keep = new Set<string>(),
  }: CollapseOptions = {},
): DiffRow[] {
  const out: DiffRow[] = [];
  let i = 0;
  while (i < rows.length) {
    const row = rows[i];
    if (row?.type !== "line" || row.kind !== "context") {
      if (row) out.push(row);
      i++;
      continue;
    }
    let end = i;
    while (end < rows.length) {
      const next = rows[end];
      if (next?.type !== "line" || next.kind !== "context") break;
      end++;
    }
    const run = rows.slice(i, end);
    const atStart = rows[i - 1]?.type === "hunk";
    const after = rows[end];
    const atEnd = after === undefined || after.type === "hunk";
    const keepStart = atStart ? 0 : edge;
    const keepEnd = atEnd ? 0 : edge;
    const hidden = run.length - keepStart - keepEnd;
    const first = run[keepStart];
    const open = run.some((r) => r.type === "line" && holds(r, keep));
    if (run.length > threshold && hidden > 0 && first && !open) {
      const key = `x-${first.key}`;
      if (expanded.has(key)) {
        out.push(...run);
      } else {
        out.push(...run.slice(0, keepStart));
        out.push({ type: "expander", key, hidden });
        out.push(...run.slice(run.length - keepEnd));
      }
    } else {
      out.push(...run);
    }
    i = end;
  }
  return out;
}

export interface FileLayout {
  rows: DiffRow[];
  /** Rows past the limit, not rendered until the reader asks for more. */
  remaining: number;
}

export function layoutFile(
  hunks: readonly Hunk[],
  expanded: ReadonlySet<string>,
  limit: number = ROW_LIMIT,
  options: CollapseOptions = {},
): FileLayout {
  const rows = collapseRuns(hunkRows(hunks), expanded, options);
  return {
    rows: rows.slice(0, limit),
    remaining: Math.max(0, rows.length - limit),
  };
}

/**
 * A deleted file and an added file with the same content hash are one file moved; the list shows them as one rename in the added file's place.
 * Each deleted file pairs with at most one added file, in list order.
 */
export function pairRenames(files: readonly ComparedFile[]): ComparedFile[] {
  const used = new Set<ComparedFile>();
  const pairs = new Map<ComparedFile, ComparedFile>();
  for (const added of files) {
    if (added.change !== "added" || added.new_hash === null) continue;
    const deleted = files.find(
      (f) =>
        f.change === "deleted" && f.old_hash === added.new_hash && !used.has(f),
    );
    if (deleted) {
      used.add(deleted);
      pairs.set(added, deleted);
    }
  }
  return files
    .filter((f) => !used.has(f))
    .map((f) => {
      const deleted = pairs.get(f);
      return deleted
        ? {
            ...f,
            change: "renamed" as const,
            previous_path: deleted.path,
            old_hash: deleted.old_hash,
            added_lines: 0,
            removed_lines: 0,
          }
        : f;
    });
}

export type TreeNode =
  | { type: "dir"; name: string; path: string; children: TreeNode[] }
  | { type: "file"; name: string; file: ComparedFile };

interface DirBuild {
  dirs: Map<string, DirBuild>;
  files: ComparedFile[];
}

function toNodes(dir: DirBuild, prefix: string): TreeNode[] {
  const dirs = [...dir.dirs.entries()]
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([name, child]): TreeNode => {
      let label = name;
      let node = child;
      // A directory whose only entry is another directory shares one row with it, as `src/routes`.
      for (;;) {
        const only = node.dirs.size === 1 && node.files.length === 0;
        const next = only ? [...node.dirs.entries()][0] : undefined;
        if (!next) break;
        label = `${label}/${next[0]}`;
        node = next[1];
      }
      const path = prefix === "" ? label : `${prefix}/${label}`;
      return { type: "dir", name: label, path, children: toNodes(node, path) };
    });
  const files = [...dir.files]
    .sort((a, b) => a.path.localeCompare(b.path))
    .map(
      (file): TreeNode => ({
        type: "file",
        name: file.path.slice(file.path.lastIndexOf("/") + 1),
        file,
      }),
    );
  return [...dirs, ...files];
}

/** The changed files as a directory tree: directories before files, each by name. */
export function fileTree(files: readonly ComparedFile[]): TreeNode[] {
  const root: DirBuild = { dirs: new Map(), files: [] };
  for (const file of files) {
    const parts = file.path.split("/");
    parts.pop();
    let dir = root;
    for (const part of parts) {
      let child = dir.dirs.get(part);
      if (!child) {
        child = { dirs: new Map(), files: [] };
        dir.dirs.set(part, child);
      }
      dir = child;
    }
    dir.files.push(file);
  }
  return toNodes(root, "");
}

function flatten(nodes: readonly TreeNode[]): ComparedFile[] {
  return nodes.flatMap((node) =>
    node.type === "file" ? [node.file] : flatten(node.children),
  );
}

/** The files in the order the list shows them: renames paired, then in tree order, so files of one directory stay together. */
export function listedFiles(files: readonly ComparedFile[]): ComparedFile[] {
  return flatten(fileTree(pairRenames(files)));
}

export type FileBody = "binary" | "unchanged" | "text";

/** What the diff panel shows for a file: hashes for a binary file, a note for a rename or mode change with no content change, and hunks otherwise. A file too large to diff is known only once its hunks are read. */
export function fileBody(file: ComparedFile): FileBody {
  if (file.binary) return "binary";
  if (
    (file.change === "renamed" || file.change === "mode_changed") &&
    file.old_hash === file.new_hash
  ) {
    return "unchanged";
  }
  return "text";
}

/** The file on screen: the one the address names, else the first listed file. */
export function currentFile(
  files: readonly ComparedFile[],
  path: string | undefined,
): ComparedFile | null {
  return files.find((f) => f.path === path) ?? files[0] ?? null;
}

/** What the panel shows: the warning of the path the address names, a file, or the first warning when nothing else is listed. A warned file has no trustworthy diff, so its warning wins over its file. */
export type Shown =
  | { kind: "file"; file: ComparedFile }
  | { kind: "warning"; warning: FileWarning };

export function shownEntry(
  files: readonly ComparedFile[],
  warnings: readonly FileWarning[],
  path: string | undefined,
): Shown | null {
  const warned = warnings.find((w) => w.path === path);
  if (warned) return { kind: "warning", warning: warned };
  const file = currentFile(files, path);
  if (file) {
    const own = warnings.find((w) => w.path === file.path);
    return own ? { kind: "warning", warning: own } : { kind: "file", file };
  }
  const first = warnings[0];
  return first ? { kind: "warning", warning: first } : null;
}
