/** One page of a cursor-paged list: its items and the cursor of the next page, null on the last. */
export interface Page<T> {
  items: T[];
  next: string | null;
}

/** The items read, and whether the list goes on past the last page read. */
export interface Pages<T> {
  items: T[];
  more: boolean;
}

/** Reads pages in order until the last one or `maxPages`, whichever comes first, so a very long list costs a bounded number of requests. */
export async function readPages<T>(
  readPage: (cursor: string | undefined) => Promise<Page<T>>,
  maxPages: number,
): Promise<Pages<T>> {
  const items: T[] = [];
  let cursor: string | undefined;
  for (let read = 0; read < maxPages; read += 1) {
    const page = await readPage(cursor);
    items.push(...page.items);
    if (page.next === null) {
      return { items, more: false };
    }
    cursor = page.next;
  }
  return { items, more: true };
}

/** The line under a list that stopped at its page limit: `Showing the newest 1000 threads. Older threads are not listed.` */
export function olderNotListed(count: number, noun: string): string {
  return `Showing the newest ${String(count)} ${noun}. Older ${noun} are not listed.`;
}
