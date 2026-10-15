import { parseId } from "../../format/ids";

export interface RepositorySearch {
  /** The revision to open, for example `r17`. */
  revision?: string;
}

/** Keeps `revision` only when it is exactly one revision id; any other value is ignored. */
export function parseRepositorySearch(
  search: Record<string, unknown>,
): RepositorySearch {
  const raw = search.revision;
  if (typeof raw !== "string") {
    return {};
  }
  return parseId(raw)?.kind === "revision" ? { revision: raw } : {};
}
