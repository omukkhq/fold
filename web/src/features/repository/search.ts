import { parseId } from "../../format/ids";
import { isCleanPath } from "../../mentions/parse";

export interface RepositorySearch {
  /** The revision to open, for example `r17`. */
  revision?: string;
  /** A file to open, as an `@` mention names it, for example `web/src/board.ts`. */
  path?: string;
  /** The 1-based line of `path` to select and show, such as the first line a request was made on. */
  line?: number;
}

/** Keeps `revision` only when it is exactly one revision id, `path` only when it is relative and normalised, and `line` only as a positive whole number beside a path; any other value is ignored. */
export function parseRepositorySearch(
  search: Record<string, unknown>,
): RepositorySearch {
  const { revision, path } = search;
  const cleanPath = typeof path === "string" && isCleanPath(path) ? path : null;
  const line = cleanPath === null ? null : lineNumber(search.line);
  return {
    ...(typeof revision === "string" && parseId(revision)?.kind === "revision"
      ? { revision }
      : {}),
    ...(cleanPath === null ? {} : { path: cleanPath }),
    ...(line === null ? {} : { line }),
  };
}

/** The router reads `line=12` as the number 12; a hand-written address may still hold it as text. */
function lineNumber(value: unknown): number | null {
  const text =
    typeof value === "number"
      ? String(value)
      : typeof value === "string"
        ? value
        : "";
  return /^[1-9]\d{0,8}$/.test(text) ? Number(text) : null;
}

/** The directory that holds a path, `""` for the root. */
export function parentOf(path: string): string {
  return path.split("/").slice(0, -1).join("/");
}

/** Whether a mentioned path is a folder, from the entries of its parent directory; null when the parent does not list it. */
export function folderAt(
  entries: readonly { path: string; kind: string }[],
  path: string,
): boolean | null {
  const found = entries.find((entry) => entry.path === path);
  return found ? found.kind === "directory" : null;
}

/** The directories above a path, outermost first, which the tree opens to show it. */
export function parentDirs(path: string): string[] {
  const parts = path.split("/").slice(0, -1);
  return parts.map((_, i) => parts.slice(0, i + 1).join("/"));
}
