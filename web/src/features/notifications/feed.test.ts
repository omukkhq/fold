import { describe, expect, it } from "vitest";
import {
  EMPTY_FEED,
  FEED_LIMIT,
  HIDDEN_LIMIT,
  addEntries,
  clearAll,
  clearEntry,
  entryKey,
  keeps,
  markRead,
  mergeHistory,
  parseFeed,
  removeTask,
  unreadCount,
  type FeedEntry,
  type FeedState,
} from "./feed";
import type { CardMove } from "./moves";

function move(
  seq: number,
  at = seq * 1_000,
  task = `T-${String(seq)}`,
): CardMove & { title: string | null } {
  return { seq, task, from: "review", to: "done", at, title: null };
}

function entry(seq: number, unread = true, at = seq * 1_000): FeedEntry {
  return { ...move(seq, at), unread };
}

function withEntries(...entries: FeedEntry[]): FeedState {
  return { ...EMPTY_FEED, entries };
}

describe("addEntries", () => {
  it("puts the newest first and keeps at most ten", () => {
    let state = EMPTY_FEED;
    for (let seq = 1; seq <= 12; seq += 1) {
      state = addEntries(state, [move(seq)]);
    }
    expect(state.entries).toHaveLength(FEED_LIMIT);
    expect(state.entries.map((e) => e.seq)).toEqual([
      12, 11, 10, 9, 8, 7, 6, 5, 4, 3,
    ]);
  });

  it("orders a batch and leaves out an entry it holds", () => {
    const state = addEntries(withEntries(entry(2)), [
      move(1),
      move(3),
      move(2),
    ]);
    expect(state.entries.map((e) => e.seq)).toEqual([3, 2, 1]);
  });

  it("returns the same state when nothing joins", () => {
    const full = addEntries(
      EMPTY_FEED,
      Array.from({ length: FEED_LIMIT }, (_, i) => move(i + 10)),
    );
    expect(addEntries(full, [move(10)])).toBe(full);
    expect(addEntries(full, [move(1)])).toBe(full);
  });

  it("tells a reused seq of a newer project apart by its time", () => {
    const state = addEntries(withEntries(entry(1, true, 5)), [move(1, 9)]);
    expect(state.entries.map((e) => e.at)).toEqual([9, 5]);
  });

  it("adds a move at or before the read mark as read", () => {
    const state: FeedState = { ...EMPTY_FEED, read: { seq: 5, at: 5_000 } };
    const next = addEntries(state, [move(4), move(6)]);
    expect(next.entries.map((e) => [e.seq, e.unread])).toEqual([
      [6, true],
      [4, false],
    ]);
  });
});

describe("clearing", () => {
  it("clears one entry, remembers it and keeps the newest keys", () => {
    const state = withEntries(entry(3), entry(2), entry(1));
    const next = clearEntry(state, entryKey(entry(2)));
    expect(next.entries.map((e) => e.seq)).toEqual([3, 1]);
    expect(keeps(next, move(2))).toBe(false);
    expect(addEntries(next, [move(2)])).toBe(next);
    expect(clearEntry(next, "9@9")).toBe(next);
    let many = withEntries(
      ...Array.from({ length: HIDDEN_LIMIT + 1 }, (_, i) => entry(i + 1)),
    );
    for (let seq = 1; seq <= HIDDEN_LIMIT + 1; seq += 1) {
      many = clearEntry(many, entryKey(entry(seq)));
    }
    expect(many.hidden).toHaveLength(HIDDEN_LIMIT);
  });

  it("clears all and keeps the cleared moves out, but not a newer project's", () => {
    const state = withEntries(entry(3), entry(2));
    const next = clearAll(state);
    expect(next.entries).toEqual([]);
    expect(keeps(next, move(2))).toBe(false);
    expect(keeps(next, move(4))).toBe(true);
    expect(keeps(next, move(1, 9_000))).toBe(true);
    expect(clearAll(next)).toBe(next);
  });

  it("removes the entries of a deleted task", () => {
    const state = withEntries(entry(2), entry(1));
    expect(removeTask(state, "T-1").entries.map((e) => e.seq)).toEqual([2]);
    expect(removeTask(state, "T-9")).toBe(state);
  });
});

describe("unread", () => {
  it("counts the unread entries, marks every entry read and sets the mark", () => {
    const state = withEntries(entry(3), entry(2, false), entry(1));
    expect(unreadCount(state.entries)).toBe(2);
    const read = markRead(state);
    expect(unreadCount(read.entries)).toBe(0);
    expect(read.read).toEqual({ seq: 3, at: 3_000 });
    expect(markRead(read)).toBe(read);
  });
});

describe("mergeHistory", () => {
  it("takes the history with the read state and titles the list had", () => {
    const state: FeedState = {
      ...withEntries({ ...entry(3, false), title: "Bell" }),
      read: { seq: 3, at: 3_000 },
    };
    const next = mergeHistory(state, [move(4), move(3), move(2)], new Set());
    expect(next.entries.map((e) => [e.seq, e.unread, e.title])).toEqual([
      [4, true, null],
      [3, false, "Bell"],
      [2, false, null],
    ]);
  });

  it("keeps a newer live entry and drops a stored entry the history lacks", () => {
    const live = entry(9);
    const stale = entry(7, false, 500);
    const state = withEntries(live, stale);
    const next = mergeHistory(state, [move(2)], new Set([entryKey(live)]));
    expect(next.entries.map((e) => e.seq)).toEqual([9, 2]);
  });

  it("leaves out what the list cleared", () => {
    const state = clearEntry(withEntries(entry(2)), entryKey(entry(2)));
    expect(mergeHistory(state, [move(2)], new Set()).entries).toEqual([]);
  });

  it("names a title the history found", () => {
    const next = mergeHistory(
      withEntries(entry(1)),
      [{ ...move(1), title: "Bell" }],
      new Set(),
    );
    expect(next.entries[0]?.title).toBe("Bell");
  });
});

describe("parseFeed", () => {
  it("reads a stored state", () => {
    const stored: FeedState = {
      entries: [entry(1), { ...entry(2), title: "Bell" }],
      read: { seq: 1, at: 1_000 },
      cleared: null,
      hidden: ["0@0"],
    };
    const parsed = parseFeed(JSON.stringify(stored));
    expect(parsed.entries.map((e) => e.seq)).toEqual([2, 1]);
    expect(parsed.read).toEqual({ seq: 1, at: 1_000 });
    expect(parsed.hidden).toEqual(["0@0"]);
  });

  it("starts empty when the text is missing or not a stored state", () => {
    const good = { entries: [], read: null, cleared: null, hidden: [] };
    for (const raw of [
      null,
      "{",
      "[]",
      JSON.stringify({ ...good, entries: "x" }),
      JSON.stringify({ ...good, entries: [{ ...entry(1), to: "later" }] }),
      JSON.stringify({ ...good, entries: [{ ...entry(1), title: 4 }] }),
      JSON.stringify({ ...good, read: { seq: 1.5, at: 1 } }),
      JSON.stringify({ ...good, cleared: "x" }),
      JSON.stringify({ ...good, hidden: "x" }),
      JSON.stringify({ ...good, hidden: [1] }),
    ]) {
      expect(parseFeed(raw)).toEqual(EMPTY_FEED);
    }
  });
});
