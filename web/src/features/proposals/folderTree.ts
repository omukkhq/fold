import type { ComparedFile } from "@/api/diff";
import { fileTree, pairRenames, type TreeNode } from "@/features/diff/layout";

/** What a folder holds below it: its changed files and their added and removed lines. */
export interface Totals {
  files: number;
  added: number;
  removed: number;
}

/** A row of the changed-files tree. A folder whose only entry is another folder shares one row with it, so `name` can be `alembic/versions`. */
export type ChangeNode =
  | {
      kind: "folder";
      name: string;
      /** The folder's path from the project root, which also names it in the open and closed state. */
      path: string;
      totals: Totals;
      children: ChangeNode[];
    }
  | { kind: "file"; name: string; path: string; file: ComparedFile };

function totalsOf(nodes: readonly ChangeNode[]): Totals {
  return nodes.reduce<Totals>(
    (sum, node) => {
      const own =
        node.kind === "folder"
          ? node.totals
          : {
              files: 1,
              added: node.file.added_lines,
              removed: node.file.removed_lines,
            };
      return {
        files: sum.files + own.files,
        added: sum.added + own.added,
        removed: sum.removed + own.removed,
      };
    },
    { files: 0, added: 0, removed: 0 },
  );
}

function toChangeNodes(nodes: readonly TreeNode[]): ChangeNode[] {
  return nodes.map((node): ChangeNode => {
    if (node.type === "file") {
      return {
        kind: "file",
        name: node.name,
        path: node.file.path,
        file: node.file,
      };
    }
    const children = toChangeNodes(node.children);
    return {
      kind: "folder",
      name: node.name,
      path: node.path,
      totals: totalsOf(children),
      children,
    };
  });
}

/** The changed files as folders before files, each by name, with renames paired as the diff lists them. */
export function changeTree(files: readonly ComparedFile[]): ChangeNode[] {
  return toChangeNodes(fileTree(pairRenames(files)));
}

/** The words of a filter, lower case; an empty filter has none. */
function filterWords(filter: string): string[] {
  return filter.trim().toLowerCase().split(/\s+/).filter(Boolean);
}

/** A path matches when it holds every word of the filter, in any case. */
export function matchesFilter(path: string, filter: string): boolean {
  const lower = path.toLowerCase();
  return filterWords(filter).every((word) => lower.includes(word));
}

/** One line of the tree as the screen draws it. */
export interface TreeRow {
  node: ChangeNode;
  depth: number;
  /** A folder shows its entries; false for a closed folder and for a file. */
  open: boolean;
}

function keep(node: ChangeNode, filter: string): boolean {
  return node.kind === "file"
    ? matchesFilter(node.path, filter)
    : node.children.some((child) => keep(child, filter));
}

/**
 * The rows on screen: every folder that is not in `closed` shows its entries. With a filter only the matching files and the folders that hold them show, and those folders stay open, so no match is hidden.
 */
export function treeRows(
  nodes: readonly ChangeNode[],
  closed: ReadonlySet<string>,
  filter = "",
): TreeRow[] {
  const filtering = filterWords(filter).length > 0;
  const rows: TreeRow[] = [];
  const visit = (list: readonly ChangeNode[], depth: number) => {
    for (const node of list) {
      if (filtering && !keep(node, filter)) continue;
      const open =
        node.kind === "folder" && (filtering || !closed.has(node.path));
      rows.push({ node, depth, open });
      if (node.kind === "folder" && open) visit(node.children, depth + 1);
    }
  };
  visit(nodes, 0);
  return rows;
}

/** Every folder of the tree, for Collapse all. */
export function folderPaths(nodes: readonly ChangeNode[]): string[] {
  return nodes.flatMap((node) =>
    node.kind === "folder" ? [node.path, ...folderPaths(node.children)] : [],
  );
}

/** The files in tree order. */
export function treeFiles(nodes: readonly ChangeNode[]): ComparedFile[] {
  return nodes.flatMap((node) =>
    node.kind === "file" ? [node.file] : treeFiles(node.children),
  );
}

/** The folders that hold `path`, outermost first, so the screen can open them when it shows the file. */
export function foldersOf(
  nodes: readonly ChangeNode[],
  path: string,
): string[] {
  for (const node of nodes) {
    if (node.kind === "file") {
      if (node.path === path) return [];
      continue;
    }
    if (!path.startsWith(`${node.path}/`)) continue;
    return [node.path, ...foldersOf(node.children, path)];
  }
  return [];
}

/** One line of the changed-folders summary. */
export interface ChangedFolder {
  /** The folder's path; empty for the files at the project root. */
  path: string;
  totals: Totals;
  /** The first file below it in tree order, which the summary's link opens. */
  first: string;
}

function folderEntry(
  path: string,
  nodes: readonly ChangeNode[],
): ChangedFolder {
  return {
    path,
    totals: totalsOf(nodes),
    first: treeFiles(nodes)[0]?.path ?? "",
  };
}

/**
 * Where the change is, in at most two levels of the tree: each top folder that holds folders gives one line per folder it holds and one line for the files directly in it, and any other top folder gives one line. Files at the project root share one line with an empty path.
 */
export function changedFolders(nodes: readonly ChangeNode[]): ChangedFolder[] {
  const rootFiles = nodes.filter((node) => node.kind === "file");
  const lines = nodes.flatMap((node): ChangedFolder[] => {
    if (node.kind === "file") return [];
    const folders = node.children.filter((child) => child.kind === "folder");
    if (folders.length === 0) return [folderEntry(node.path, node.children)];
    const files = node.children.filter((child) => child.kind === "file");
    return [
      ...folders.map((folder) => folderEntry(folder.path, folder.children)),
      ...(files.length > 0 ? [folderEntry(node.path, files)] : []),
    ];
  });
  return rootFiles.length > 0 ? [...lines, folderEntry("", rootFiles)] : lines;
}

/** `3 files`, `1 file`. */
export function filesWord(n: number): string {
  return `${String(n)} ${n === 1 ? "file" : "files"}`;
}

/** A path as the diff head draws it: the folder part, with its closing slash, dimmed before the name. */
export function pathParts(path: string): { folder: string; name: string } {
  const cut = path.lastIndexOf("/") + 1;
  return { folder: path.slice(0, cut), name: path.slice(cut) };
}

/** Opens a closed folder and closes an open one. */
export function toggleFolder(
  closed: ReadonlySet<string>,
  path: string,
): Set<string> {
  const next = new Set(closed);
  if (next.has(path)) next.delete(path);
  else next.add(path);
  return next;
}
