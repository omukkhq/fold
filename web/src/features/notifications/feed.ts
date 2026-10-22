import { isRecord } from "@/live/envelope";
import { isTaskStatus, type CardMove } from "./moves";

/** The bell lists this many moves at most; an older one leaves the list when a newer one arrives. */
export const FEED_LIMIT = 10;

/** One line of the bell's list: one card move. */
export interface FeedEntry extends CardMove {
  /** The task's title as the board showed it when the move arrived, or null when the board was not loaded. */
  title: string | null;
  /** True until the list is opened after the entry arrived. */
  unread: boolean;
}

/** Tells entries apart. The time joins the `seq` because a stored list outlives its project: a project made again under the same name numbers its events from 1 again. */
export function entryKey(entry: Pick<FeedEntry, "seq" | "at">): string {
  return `${String(entry.seq)}@${String(entry.at)}`;
}

/** The list with the new entries in front, newest first, cut to {@link FEED_LIMIT}. An entry the list already holds is left out. */
export function addEntries(
  feed: readonly FeedEntry[],
  entries: readonly FeedEntry[],
): FeedEntry[] {
  const known = new Set(feed.map(entryKey));
  const fresh = entries
    .filter((entry) => !known.has(entryKey(entry)))
    .sort((a, b) => b.at - a.at || b.seq - a.seq);
  return [...fresh, ...feed].slice(0, FEED_LIMIT);
}

export function clearEntry(
  feed: readonly FeedEntry[],
  key: string,
): FeedEntry[] {
  return feed.filter((entry) => entryKey(entry) !== key);
}

/** Every entry read; the same list when none was unread, so a store can skip the update. */
export function markRead(feed: readonly FeedEntry[]): readonly FeedEntry[] {
  return feed.some((entry) => entry.unread)
    ? feed.map((entry) => ({ ...entry, unread: false }))
    : feed;
}

export function unreadCount(feed: readonly FeedEntry[]): number {
  return feed.filter((entry) => entry.unread).length;
}

function isEntry(value: unknown): value is FeedEntry {
  return (
    isRecord(value) &&
    typeof value.seq === "number" &&
    Number.isInteger(value.seq) &&
    typeof value.task === "string" &&
    isTaskStatus(value.from) &&
    isTaskStatus(value.to) &&
    typeof value.at === "number" &&
    (value.title === null || typeof value.title === "string") &&
    typeof value.unread === "boolean"
  );
}

/** The list a browser stored, or an empty one when the text is missing or not a list of entries. */
export function parseFeed(raw: string | null): FeedEntry[] {
  if (raw === null) {
    return [];
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return [];
  }
  if (!Array.isArray(parsed) || !parsed.every(isEntry)) {
    return [];
  }
  return addEntries([], parsed);
}
