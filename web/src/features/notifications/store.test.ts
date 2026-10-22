import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { TaskStatus } from "@/api/board";
import type { EventEnvelope } from "@/live/envelope";
import { BATCH_WINDOW_MS } from "./batch";
import { entryKey, FEED_LIMIT, type FeedEntry } from "./feed";
import {
  TOAST_LIMIT,
  createNotificationStore,
  storageKey,
  type NotificationOptions,
} from "./store";

function moved(
  seq: number,
  task: string,
  to: TaskStatus,
  from: TaskStatus = "in_progress",
): EventEnvelope {
  return {
    kind: "event",
    seq,
    type: "task.status_changed",
    actor: "fold",
    task,
    proposal: null,
    workspace: null,
    revision: null,
    data: { from, to },
    created_at: seq * 1_000,
  };
}

function memoryStorage() {
  const items = new Map<string, string>();
  return {
    items,
    getItem: (key: string) => items.get(key) ?? null,
    setItem: (key: string, value: string) => {
      items.set(key, value);
    },
  };
}

const noTitle = () => null;

function storeWith(options: Partial<NotificationOptions> = {}) {
  const storage = memoryStorage();
  const store = createNotificationStore({ storage: () => storage, ...options });
  const changes = vi.fn();
  store.subscribe(changes);
  return { store, storage, changes };
}

beforeEach(() => {
  vi.useFakeTimers();
});
afterEach(() => {
  vi.useRealTimers();
});

describe("toasts", () => {
  it("shows one toast for the moves inside the window", () => {
    const { store } = storeWith();
    store.receive("demo", moved(1, "T-12", "review"), false, noTitle);
    vi.advanceTimersByTime(800);
    store.receive("demo", moved(2, "T-14", "done"), false, noTitle);
    expect(store.toasts()).toEqual([]);
    vi.advanceTimersByTime(BATCH_WINDOW_MS);
    expect(store.toasts()).toEqual([
      {
        id: 1,
        slug: "demo",
        parts: ["T-12 moved to Review", "T-14 moved to Done"],
      },
    ]);
  });

  it("lists a replayed move without a toast", () => {
    const { store } = storeWith();
    store.receive("demo", moved(1, "T-12", "review"), true, noTitle);
    vi.advanceTimersByTime(BATCH_WINDOW_MS);
    expect(store.toasts()).toEqual([]);
    expect(store.feed("demo")).toHaveLength(1);
  });

  it("keeps the newest toasts and closes one on dismiss", () => {
    const { store } = storeWith();
    for (let seq = 1; seq <= TOAST_LIMIT + 1; seq += 1) {
      store.receive(
        "demo",
        moved(seq, `T-${String(seq)}`, "done"),
        false,
        noTitle,
      );
      vi.advanceTimersByTime(BATCH_WINDOW_MS);
    }
    expect(store.toasts().map((t) => t.id)).toEqual([2, 3, 4]);
    store.dismiss(3);
    store.dismiss(99);
    expect(store.toasts().map((t) => t.id)).toEqual([2, 4]);
  });

  it("drops the waiting moves and the toasts of a project the stream leaves", () => {
    const { store, changes } = storeWith();
    store.receive("demo", moved(1, "T-1", "done"), false, noTitle);
    vi.advanceTimersByTime(BATCH_WINDOW_MS);
    store.receive("demo", moved(2, "T-2", "done"), false, noTitle);
    store.leave("demo");
    vi.advanceTimersByTime(BATCH_WINDOW_MS);
    expect(store.toasts()).toEqual([]);
    changes.mockClear();
    store.leave("demo");
    store.leave("other");
    expect(changes).not.toHaveBeenCalled();
  });
});

describe("the list", () => {
  it("adds each move once, newest first, with the board's title", () => {
    const { store } = storeWith();
    const title = (task: string) => (task === "T-1" ? "Add the bell" : null);
    store.receive("demo", moved(1, "T-1", "review"), false, title);
    store.receive("demo", moved(2, "T-2", "done"), false, title);
    store.receive("demo", moved(2, "T-2", "done"), false, title);
    store.receive(
      "demo",
      { kind: "snapshot", seq: 2, workspaces: [] },
      false,
      title,
    );
    expect(
      store.feed("demo").map((e) => [e.task, e.to, e.title, e.unread]),
    ).toEqual([
      ["T-2", "done", null, true],
      ["T-1", "review", "Add the bell", true],
    ]);
  });

  it("keeps at most ten entries", () => {
    const { store } = storeWith();
    for (let seq = 1; seq <= FEED_LIMIT + 3; seq += 1) {
      store.receive(
        "demo",
        moved(seq, `T-${String(seq)}`, "done"),
        false,
        noTitle,
      );
    }
    const feed = store.feed("demo");
    expect(feed).toHaveLength(FEED_LIMIT);
    expect(feed[0]?.task).toBe("T-13");
    expect(feed.at(-1)?.task).toBe("T-4");
    store.receive("demo", moved(13, "T-13", "done"), false, noTitle);
    expect(store.feed("demo")).toBe(feed);
  });

  it("keeps one list per project", () => {
    const { store } = storeWith();
    store.receive("demo", moved(1, "T-1", "done"), false, noTitle);
    store.receive("other", moved(1, "T-1", "review"), false, noTitle);
    expect(store.feed("demo")[0]?.to).toBe("done");
    expect(store.feed("other")[0]?.to).toBe("review");
  });

  it("marks every entry read, clears one and clears all", () => {
    const { store, changes } = storeWith();
    store.receive("demo", moved(1, "T-1", "done"), false, noTitle);
    store.receive("demo", moved(2, "T-2", "done"), false, noTitle);
    store.markRead("demo");
    expect(store.feed("demo").every((e) => !e.unread)).toBe(true);
    changes.mockClear();
    store.markRead("demo");
    expect(changes).not.toHaveBeenCalled();
    const second = store.feed("demo")[1] as FeedEntry;
    store.clear("demo", entryKey(second));
    expect(store.feed("demo").map((e) => e.task)).toEqual(["T-2"]);
    store.clearAll("demo");
    expect(store.feed("demo")).toEqual([]);
    changes.mockClear();
    store.clearAll("demo");
    expect(changes).not.toHaveBeenCalled();
  });
});

describe("storage", () => {
  it("keeps the list in the browser across pages", () => {
    const storage = memoryStorage();
    const first = createNotificationStore({ storage: () => storage });
    first.receive("demo", moved(1, "T-1", "done"), false, noTitle);
    expect(storage.items.has(storageKey("demo"))).toBe(true);
    const second = createNotificationStore({ storage: () => storage });
    expect(second.feed("demo").map((e) => e.task)).toEqual(["T-1"]);
  });

  it("works in memory when storage refuses reads and writes or is missing", () => {
    const refusing = {
      getItem: () => {
        throw new Error("blocked");
      },
      setItem: () => {
        throw new Error("blocked");
      },
    };
    for (const storage of [refusing, null]) {
      const store = createNotificationStore({ storage: () => storage });
      expect(store.feed("demo")).toEqual([]);
      store.receive("demo", moved(1, "T-1", "done"), false, noTitle);
      expect(store.feed("demo")).toHaveLength(1);
    }
  });

  it("reaches no storage when the accessor throws", () => {
    const store = createNotificationStore({
      storage: () => {
        throw new Error("denied");
      },
    });
    store.receive("demo", moved(1, "T-1", "done"), false, noTitle);
    expect(store.feed("demo")).toHaveLength(1);
  });

  it("unsubscribes a listener", () => {
    const { store } = storeWith();
    const listener = vi.fn();
    const stop = store.subscribe(listener);
    stop();
    store.receive("demo", moved(1, "T-1", "done"), false, noTitle);
    expect(listener).not.toHaveBeenCalled();
  });
});
