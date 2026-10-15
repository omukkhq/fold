import { parseId } from "../../format/ids";
import { isCleanPath } from "../../mentions/parse";

export interface RepositorySearch {
  /** The revision to open, for example `r17`. */
  revision?: string;
  /** A file to open, as an `@` mention names it, for example `web/src/board.ts`. */
  path?: string;
}

/** Keeps `revision` only when it is exactly one revision id and `path` only when it is relative and normalised; any other value is ignored. */
export function parseRepositorySearch(
  search: Record<string, unknown>,
): RepositorySearch {
  const { revision, path } = search;
  return {
    ...(typeof revision === "string" && parseId(revision)?.kind === "revision"
      ? { revision }
      : {}),
    ...(typeof path === "string" && isCleanPath(path) ? { path } : {}),
  };
}

/** The directories above a path, outermost first, which the tree opens to show it. */
export function parentDirs(path: string): string[] {
  const parts = path.split("/").slice(0, -1);
  return parts.map((_, i) => parts.slice(0, i + 1).join("/"));
}
