/** The part of a revision the parent walk reads. */
interface Revision {
  id: string;
  parent: string | null;
}

/** How far main is ahead of `base`, `{ count, atLeast }`: the steps along the parents of main's newest revisions, newest first; when `base` is older than the list, at least the steps the list holds. */
export function revisionsAhead(
  recent: readonly Revision[],
  base: string,
): { count: number; atLeast: boolean } {
  let at = recent[0];
  let count = 0;
  while (at !== undefined) {
    if (at.id === base) {
      return { count, atLeast: false };
    }
    const parent = at.parent;
    at = recent.find((revision) => revision.id === parent);
    count += 1;
  }
  return { count, atLeast: true };
}

/** `1 revision`, `3 revisions`, `at least 10 revisions`. */
export function revisionCount(ahead: {
  count: number;
  atLeast: boolean;
}): string {
  const noun = ahead.count === 1 ? "revision" : "revisions";
  return `${ahead.atLeast ? "at least " : ""}${String(ahead.count)} ${noun}`;
}
