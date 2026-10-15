/** How many events the sidebar keeps; older ones are not shown. */
export const EVENT_LIMIT = 30;

/** Newest first by `seq`, which the daemon never reuses; an event already in the list is not added twice. */
export function withEvent<T extends { seq: number }>(
  list: readonly T[],
  event: T,
): T[] {
  if (list.some((known) => known.seq === event.seq)) {
    return [...list];
  }
  return [...list, event].sort((a, b) => b.seq - a.seq).slice(0, EVENT_LIMIT);
}
