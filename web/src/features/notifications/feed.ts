import { isRecord } from "@/live/envelope";
import { isTaskStatus, type CardMove } from "./moves";

/** The bell lists this many moves at most; an older one leaves the list when a newer one arrives. */
export const FEED_LIMIT = 10;

/** The list remembers this many entries cleared one by one, so the history does not bring them back. */
export const HIDDEN_LIMIT = 50;

/** One line of the bell's list: one card move. */
export interface FeedEntry extends CardMove {
  /** The task's title as the board showed it when the move arrived, or null when the board was not loaded. */
  title: string | null;
  /** True until the list is opened after the entry arrived. */
  unread: boolean;
}

/** A point in a project's events. The time joins the `seq` because a stored list outlives its project: a project made again under the same name numbers its events from 1 again, at later times. */
export interface Mark {
  seq: number;
  at: number;
}

/** What a browser keeps of one project's list. */
export interface FeedState {
  /** Newest first, at most {@link FEED_LIMIT}. */
  entries: readonly FeedEntry[];
  /** The newest entry when the list was last opened; an entry at or before it arrives read. */
  read: Mark | null;
  /** The newest entry when the list was last cleared; an entry at or before it stays out. */
  cleared: Mark | null;
  /** The keys of the entries cleared one by one, newest first. */
  hidden: readonly string[];
}

export const EMPTY_FEED: FeedState = {
  entries: [],
  read: null,
  cleared: null,
  hidden: [],
};

export function entryKey(entry: Mark): string {
  return `${String(entry.seq)}@${String(entry.at)}`;
}

function covers(mark: Mark | null, entry: Mark): boolean {
  return mark !== null && entry.seq <= mark.seq && entry.at <= mark.at;
}

function markOf(entry: FeedEntry | undefined): Mark | null {
  return entry === undefined ? null : { seq: entry.seq, at: entry.at };
}

function newestFirst(a: Mark, b: Mark): number {
  return b.at - a.at || b.seq - a.seq;
}

/** True when the list keeps the move: it was neither cleared nor hidden. */
export function keeps(state: FeedState, move: Mark): boolean {
  return !covers(state.cleared, move) && !state.hidden.includes(entryKey(move));
}

/** The list with the new moves in, newest first, cut to {@link FEED_LIMIT}. A move the list holds, cleared or hid stays out; the same state when nothing joins. */
export function addEntries(
  state: FeedState,
  moves: readonly (CardMove & { title: string | null })[],
): FeedState {
  const known = new Set(state.entries.map(entryKey));
  const fresh = moves
    .filter((move) => !known.has(entryKey(move)) && keeps(state, move))
    .map((move) => ({ ...move, unread: !covers(state.read, move) }));
  if (fresh.length === 0) {
    return state;
  }
  const entries = [...fresh, ...state.entries]
    .sort(newestFirst)
    .slice(0, FEED_LIMIT);
  return entries.some((entry) => fresh.includes(entry))
    ? { ...state, entries }
    : state;
}

/**
 * The list after the daemon's history: the history's moves that the list keeps, with the read state and title the list had for them, and the moves that arrived on the stream since the page loaded (`live`).
 * A stored entry that is in neither belongs to a project or a task that is gone, and leaves.
 */
export function mergeHistory(
  state: FeedState,
  history: readonly (CardMove & { title: string | null })[],
  live: ReadonlySet<string>,
): FeedState {
  const had = new Map(state.entries.map((entry) => [entryKey(entry), entry]));
  const fromHistory = history.filter((move) => keeps(state, move));
  const keys = new Set(fromHistory.map(entryKey));
  const entries = [
    ...fromHistory.map((move): FeedEntry => {
      const old = had.get(entryKey(move));
      return {
        ...move,
        title: move.title ?? old?.title ?? null,
        unread: old?.unread ?? !covers(state.read, move),
      };
    }),
    ...state.entries.filter(
      (entry) => live.has(entryKey(entry)) && !keys.has(entryKey(entry)),
    ),
  ]
    .sort(newestFirst)
    .slice(0, FEED_LIMIT);
  return { ...state, entries };
}

/** The list without the entries of a deleted task; the same state when it holds none. */
export function removeTask(state: FeedState, task: string): FeedState {
  const entries = state.entries.filter((entry) => entry.task !== task);
  return entries.length === state.entries.length
    ? state
    : { ...state, entries };
}

export function clearEntry(state: FeedState, key: string): FeedState {
  const entries = state.entries.filter((entry) => entryKey(entry) !== key);
  if (entries.length === state.entries.length) {
    return state;
  }
  return {
    ...state,
    entries,
    hidden: [key, ...state.hidden].slice(0, HIDDEN_LIMIT),
  };
}

/** No entry, and every move up to the newest one stays out; the same state when the list is empty. */
export function clearAll(state: FeedState): FeedState {
  const cleared = markOf(state.entries[0]);
  return cleared === null
    ? state
    : { entries: [], read: cleared, cleared, hidden: [] };
}

/** Every entry read; the same state when none was unread. */
export function markRead(state: FeedState): FeedState {
  if (!state.entries.some((entry) => entry.unread)) {
    return state;
  }
  return {
    ...state,
    entries: state.entries.map((entry) => ({ ...entry, unread: false })),
    read: markOf(state.entries[0]),
  };
}

export function unreadCount(entries: readonly FeedEntry[]): number {
  return entries.filter((entry) => entry.unread).length;
}

function isMark(value: unknown): value is Mark {
  return (
    isRecord(value) &&
    typeof value.seq === "number" &&
    Number.isInteger(value.seq) &&
    typeof value.at === "number"
  );
}

function isEntry(value: unknown): value is FeedEntry {
  return (
    isRecord(value) &&
    isMark(value) &&
    typeof value.task === "string" &&
    isTaskStatus(value.from) &&
    isTaskStatus(value.to) &&
    (value.title === null || typeof value.title === "string") &&
    typeof value.unread === "boolean"
  );
}

/** The state a browser stored, or an empty one when the text is missing or not a stored list. */
export function parseFeed(raw: string | null): FeedState {
  if (raw === null) {
    return EMPTY_FEED;
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return EMPTY_FEED;
  }
  if (
    !isRecord(parsed) ||
    !Array.isArray(parsed.entries) ||
    !parsed.entries.every(isEntry) ||
    !(parsed.read === null || isMark(parsed.read)) ||
    !(parsed.cleared === null || isMark(parsed.cleared)) ||
    !Array.isArray(parsed.hidden) ||
    !parsed.hidden.every((key) => typeof key === "string")
  ) {
    return EMPTY_FEED;
  }
  return {
    entries: [...parsed.entries].sort(newestFirst).slice(0, FEED_LIMIT),
    read: parsed.read,
    cleared: parsed.cleared,
    hidden: parsed.hidden.slice(0, HIDDEN_LIMIT),
  };
}
