import { describe, expect, it } from "vitest";
import { paneShown, readDue, readsDue } from "./readGate";

const M = `m-${String(31)}`;
const N = `m-${String(32)}`;

describe("paneShown", () => {
  it("shows the pane for a thread in the address or in the wide layout", () => {
    expect(paneShown(M, false)).toBe(true);
    expect(paneShown(M, true)).toBe(true);
    expect(paneShown(undefined, true)).toBe(true);
  });

  it("shows only the list on a narrow screen with no thread in the address", () => {
    expect(paneShown(undefined, false)).toBe(false);
  });
});

describe("readDue", () => {
  const gate = { unread: M, shown: true, visible: true, marked: null };

  it("is due for the newest unread message of a thread on the screen", () => {
    expect(readDue(gate)).toBe(M);
  });

  it("waits while nothing is unread, the pane is hidden or the tab is in the background", () => {
    expect(readDue({ ...gate, unread: null })).toBeNull();
    expect(readDue({ ...gate, shown: false })).toBeNull();
    expect(readDue({ ...gate, visible: false })).toBeNull();
  });

  it("makes one call per newest unread message", () => {
    expect(readDue({ ...gate, marked: M })).toBeNull();
    expect(readDue({ ...gate, unread: N, marked: M })).toBe(N);
  });
});

describe("readsDue", () => {
  const calls = [
    { thread: M, unread: M },
    { thread: N, unread: N },
  ];

  it("is due for each thread of the chat whose newest unread message has no call yet", () => {
    expect(readsDue(calls, true, true, new Map())).toEqual(calls);
    expect(readsDue(calls, true, true, new Map([[M, M]]))).toEqual([
      { thread: N, unread: N },
    ]);
  });

  it("waits while the chat is hidden or the tab is in the background", () => {
    expect(readsDue(calls, false, true, new Map())).toEqual([]);
    expect(readsDue(calls, true, false, new Map())).toEqual([]);
  });
});
