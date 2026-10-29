/** Where a panel's list stands: it waits for its first answer, the read failed with nothing to show, or the list is there. */
export type ListState = "pending" | "error" | "ready";

/** A query's state for its list: a failed read that still has an earlier answer shows that answer. */
export function listState(query: {
  isPending: boolean;
  isError: boolean;
}): ListState {
  if (query.isPending) return "pending";
  return query.isError ? "error" : "ready";
}

export interface ListNote {
  text: string;
  /** The read failed: the note is an alert and offers Retry. */
  failed: boolean;
}

/** The line a panel shows in place of its list: while it loads, when the read failed, or when the list is empty; null when there are rows to show. */
export function listNote(
  state: ListState,
  count: number,
  words: { loading: string; failed: string; empty: string },
): ListNote | null {
  if (count > 0) return null;
  switch (state) {
    case "pending":
      return { text: words.loading, failed: false };
    case "error":
      return { text: words.failed, failed: true };
    case "ready":
      return { text: words.empty, failed: false };
  }
}
