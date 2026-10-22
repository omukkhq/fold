import { describe, expect, it } from "vitest";
import {
  FEED_LIMIT,
  addEntries,
  clearEntry,
  entryKey,
  markRead,
  parseFeed,
  unreadCount,
  type FeedEntry,
} from "./feed";

function entry(seq: number, unread = true, at = seq * 1_000): FeedEntry {
  return {
    seq,
    task: `T-${String(seq)}`,
    from: "review",
    to: "done",
    at,
    title: null,
    unread,
  };
}

describe("addEntries", () => {
  it("puts the newest first and keeps at most ten", () => {
    let feed: FeedEntry[] = [];
    for (let seq = 1; seq <= 12; seq += 1) {
      feed = addEntries(feed, [entry(seq)]);
    }
    expect(feed).toHaveLength(FEED_LIMIT);
    expect(feed.map((e) => e.seq)).toEqual([12, 11, 10, 9, 8, 7, 6, 5, 4, 3]);
  });

  it("orders a batch newest first and leaves out an entry it holds", () => {
    const feed = addEntries([entry(2)], [entry(1), entry(3), entry(2)]);
    expect(feed.map((e) => e.seq)).toEqual([3, 1, 2]);
  });

  it("tells a reused seq of a newer project apart by its time", () => {
    const feed = addEntries([entry(1, true, 5)], [entry(1, true, 9)]);
    expect(feed.map((e) => e.at)).toEqual([9, 5]);
  });
});

describe("clearing", () => {
  it("clears one entry by its key", () => {
    const feed = [entry(3), entry(2), entry(1)];
    expect(clearEntry(feed, entryKey(entry(2))).map((e) => e.seq)).toEqual([
      3, 1,
    ]);
  });
});

describe("unread", () => {
  it("counts the unread entries and marks every entry read", () => {
    const feed = [entry(3), entry(2, false), entry(1)];
    expect(unreadCount(feed)).toBe(2);
    const read = markRead(feed);
    expect(unreadCount(read)).toBe(0);
    expect(markRead(read)).toBe(read);
  });
});

describe("parseFeed", () => {
  it("reads a stored list", () => {
    const stored = [entry(1), { ...entry(2), title: "Bell" }];
    expect(parseFeed(JSON.stringify(stored)).map((e) => e.seq)).toEqual([2, 1]);
  });

  it("starts empty when the text is missing or not a list of entries", () => {
    expect(parseFeed(null)).toEqual([]);
    expect(parseFeed("{")).toEqual([]);
    expect(parseFeed('{"seq":1}')).toEqual([]);
    expect(parseFeed(JSON.stringify([{ ...entry(1), to: "later" }]))).toEqual(
      [],
    );
    expect(parseFeed(JSON.stringify([{ ...entry(1), title: 4 }]))).toEqual([]);
    expect(parseFeed(JSON.stringify(["T-1"]))).toEqual([]);
  });
});
