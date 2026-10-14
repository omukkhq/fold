/** `from` and `to` name a revision, changeset, proposal version or workspace as the daemon defines it, so the screen passes them on unparsed. */
export interface DiffSearch {
  /** Absent when a link knows only `to`; the daemon then compares `to` with the base it recorded for it. */
  from?: string;
  to: string;
  /** The file on screen; the first listed file when absent. */
  path?: string;
}

function text(value: unknown): string {
  return typeof value === "string" ? value.trim() : "";
}

/** An empty `to` means the address names no comparison; an empty `from` or `path` is left out. */
export function parseDiffSearch(search: Record<string, unknown>): DiffSearch {
  const from = text(search.from);
  const path = text(search.path);
  return {
    ...(from === "" ? {} : { from }),
    to: text(search.to),
    ...(path === "" ? {} : { path }),
  };
}

export function namesComparison(search: DiffSearch): boolean {
  return search.to !== "";
}
