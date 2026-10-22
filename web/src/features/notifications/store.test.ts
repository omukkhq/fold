import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { TaskStatus } from "@/api/board";
import type { DurableEvent } from "@/api/events";
import type { EventEnvelope, StoredEvent } from "@/live/envelope";
import { BATCH_WINDOW_MS } from "./batch";
import { entryKey, FEED_LIMIT, type FeedEntry } from "./feed";
import {
  TOAST_LIMIT,
  createNotificationStore,
  storageKey,
  watchStorage,
  type Frame,
  type HistorySource,
  type NotificationOptions,
} from "./store";

function moved(
  seq: number,
  task: string,
  to: TaskStatus,
  from: TaskStatus = "in_progress",
): StoredEvent {
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

function frame(
  replay: boolean,
  title: (task: string) => string | null = noTitle,
  self: string | null = null,
): Frame {
  return { replay, title, self };
}

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
    store.receive("demo", moved(1, "T-12", "review"), frame(false, noTitle));
    vi.advanceTimersByTime(800);
    store.receive("demo", moved(2, "T-14", "done"), frame(false, noTitle));
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
    store.receive("demo", moved(1, "T-12", "review"), frame(true, noTitle));
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
        frame(false, noTitle),
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
    store.receive("demo", moved(1, "T-1", "done"), frame(false, noTitle));
    vi.advanceTimersByTime(BATCH_WINDOW_MS);
    store.receive("demo", moved(2, "T-2", "done"), frame(false, noTitle));
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
    store.receive("demo", moved(1, "T-1", "review"), frame(false, title));
    store.receive("demo", moved(2, "T-2", "done"), frame(false, title));
    store.receive("demo", moved(2, "T-2", "done"), frame(false, title));
    store.receive(
      "demo",
      { kind: "snapshot", seq: 2, workspaces: [] },
      frame(false, title),
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
        frame(false, noTitle),
      );
    }
    const feed = store.feed("demo");
    expect(feed).toHaveLength(FEED_LIMIT);
    expect(feed[0]?.task).toBe("T-13");
    expect(feed.at(-1)?.task).toBe("T-4");
    store.receive("demo", moved(13, "T-13", "done"), frame(false, noTitle));
    expect(store.feed("demo")).toBe(feed);
  });

  it("keeps one list per project", () => {
    const { store } = storeWith();
    store.receive("demo", moved(1, "T-1", "done"), frame(false, noTitle));
    store.receive("other", moved(1, "T-1", "review"), frame(false, noTitle));
    expect(store.feed("demo")[0]?.to).toBe("done");
    expect(store.feed("other")[0]?.to).toBe("review");
  });

  it("marks every entry read, clears one and clears all", () => {
    const { store, changes } = storeWith();
    store.receive("demo", moved(1, "T-1", "done"), frame(false, noTitle));
    store.receive("demo", moved(2, "T-2", "done"), frame(false, noTitle));
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
    first.receive("demo", moved(1, "T-1", "done"), frame(false, noTitle));
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
      store.receive("demo", moved(1, "T-1", "done"), frame(false, noTitle));
      expect(store.feed("demo")).toHaveLength(1);
    }
  });

  it("reaches no storage when the accessor throws", () => {
    const store = createNotificationStore({
      storage: () => {
        throw new Error("denied");
      },
    });
    store.receive("demo", moved(1, "T-1", "done"), frame(false, noTitle));
    expect(store.feed("demo")).toHaveLength(1);
  });

  it("unsubscribes a listener", () => {
    const { store } = storeWith();
    const listener = vi.fn();
    const stop = store.subscribe(listener);
    stop();
    store.receive("demo", moved(1, "T-1", "done"), frame(false, noTitle));
    expect(listener).not.toHaveBeenCalled();
  });
});

function stored(
  seq: number,
  type: DurableEvent["type"],
  task: string,
  actor = "fold",
  at = seq * 1_000,
): EventEnvelope {
  return {
    kind: "event",
    seq,
    type,
    actor,
    task,
    proposal: null,
    workspace: null,
    revision: null,
    data: {},
    created_at: at,
  };
}

describe("own moves", () => {
  it("gives no entry and no toast for a move the signed-in human caused", () => {
    const { store } = storeWith();
    const me = frame(false, noTitle, "human:ana");
    store.receive(
      "demo",
      stored(1, "integration.succeeded", "T-1", "human:ana"),
      me,
    );
    store.receive(
      "demo",
      { ...moved(2, "T-1", "done"), created_at: 1_000 },
      me,
    );
    vi.advanceTimersByTime(BATCH_WINDOW_MS);
    expect(store.feed("demo")).toEqual([]);
    expect(store.toasts()).toEqual([]);
  });

  it("shows another human's move", () => {
    const { store } = storeWith();
    const me = frame(false, noTitle, "human:ana");
    store.receive(
      "demo",
      stored(1, "integration.succeeded", "T-1", "human:bo"),
      me,
    );
    store.receive(
      "demo",
      { ...moved(2, "T-1", "done"), created_at: 1_000 },
      me,
    );
    vi.advanceTimersByTime(BATCH_WINDOW_MS);
    expect(store.feed("demo").map((e) => e.by)).toEqual(["human:bo"]);
    expect(store.toasts()).toHaveLength(1);
  });
});

describe("toast text", () => {
  it("names the title and shows no toast for a task back where it started", () => {
    const { store } = storeWith();
    const title = (task: string) => (task === "T-1" ? "Add the bell" : null);
    store.receive("demo", moved(1, "T-1", "review"), frame(false, title));
    vi.advanceTimersByTime(BATCH_WINDOW_MS);
    expect(store.toasts()[0]?.parts).toEqual([
      "T-1 “Add the bell” moved to Review",
    ]);
    store.receive("demo", moved(2, "T-2", "todo", "backlog"), frame(false));
    store.receive("demo", moved(3, "T-2", "backlog", "todo"), frame(false));
    vi.advanceTimersByTime(BATCH_WINDOW_MS);
    expect(store.toasts()).toHaveLength(1);
    expect(store.feed("demo")).toHaveLength(3);
  });
});

describe("deleted tasks", () => {
  it("drops the entries of a task the stream deletes", () => {
    const { store } = storeWith();
    store.receive("demo", moved(1, "T-1", "done"), frame(true));
    store.receive("demo", moved(2, "T-2", "done"), frame(true));
    store.receive("demo", stored(3, "task.deleted", "T-1"), frame(false));
    expect(store.feed("demo").map((e) => e.task)).toEqual(["T-2"]);
  });
});

function historyEvent(seq: number, task: string, actor = "fold"): DurableEvent {
  return {
    seq,
    type: "task.status_changed",
    actor,
    task,
    proposal: null,
    workspace: null,
    revision: null,
    data: { from: "review", to: "done" },
    created_at: seq * 1_000,
  };
}

function source(
  events: DurableEvent[],
  self: string | null = null,
): HistorySource & { read: ReturnType<typeof vi.fn> } {
  return {
    read: vi.fn(() => Promise.resolve({ events, before: null })),
    title: (task: string) => (task === "T-5" ? "From history" : null),
    self: () => self,
  };
}

describe("fill", () => {
  it("adds the moves made while no page was open, without a toast", async () => {
    const { store } = storeWith();
    store.receive("demo", moved(9, "T-9", "done"), frame(false));
    await store.fill(
      "demo",
      source([historyEvent(5, "T-5"), historyEvent(4, "T-4")]),
    );
    vi.advanceTimersByTime(BATCH_WINDOW_MS);
    expect(store.feed("demo").map((e) => [e.task, e.title, e.unread])).toEqual([
      ["T-9", null, true],
      ["T-5", "From history", true],
      ["T-4", null, true],
    ]);
    expect(store.toasts()).toHaveLength(1);
  });

  it("drops a stored entry the history does not hold", async () => {
    const storage = memoryStorage();
    const old = createNotificationStore({ storage: () => storage });
    old.receive("demo", moved(40, "T-40", "done"), frame(true));
    const store = createNotificationStore({ storage: () => storage });
    expect(store.feed("demo")).toHaveLength(1);
    await store.fill("demo", source([historyEvent(2, "T-2")]));
    expect(store.feed("demo").map((e) => e.task)).toEqual(["T-2"]);
  });

  it("leaves out the history moves the signed-in human caused", async () => {
    const { store } = storeWith();
    const integrated: DurableEvent = {
      ...historyEvent(1, "T-1", "human:ana"),
      type: "integration.succeeded",
      data: {},
      created_at: 2_000,
    };
    await store.fill(
      "demo",
      source(
        [historyEvent(3, "T-3"), historyEvent(2, "T-1"), integrated],
        "human:ana",
      ),
    );
    expect(store.feed("demo").map((e) => e.task)).toEqual(["T-3"]);
  });

  it("tries again on refill after a failure, and only then", async () => {
    const { store } = storeWith();
    const failing = source([historyEvent(1, "T-1")]);
    failing.read.mockReturnValueOnce(Promise.reject(new Error("offline")));
    await store.fill("demo", failing);
    expect(store.feed("demo")).toEqual([]);
    await store.refill("demo");
    expect(store.feed("demo")).toHaveLength(1);
    await store.refill("demo");
    await store.refill("other");
    expect(failing.read).toHaveBeenCalledTimes(2);
  });

  it("drops the answer of a fill whose stream closed", async () => {
    const { store } = storeWith();
    let answer: (page: { events: DurableEvent[]; before: null }) => void = () =>
      undefined;
    const pending = store.fill("demo", {
      ...source([]),
      read: () =>
        new Promise((resolve) => {
          answer = resolve;
        }),
    });
    store.leave("demo");
    answer({ events: [historyEvent(1, "T-1")], before: null });
    await pending;
    expect(store.feed("demo")).toEqual([]);
    await store.refill("demo");
  });

  it("forgets a failed fill whose stream closed", async () => {
    const { store } = storeWith();
    const failing = source([]);
    failing.read.mockReturnValueOnce(Promise.reject(new Error("offline")));
    const pending = store.fill("demo", failing);
    store.leave("demo");
    await pending;
    await store.refill("demo");
    expect(failing.read).toHaveBeenCalledTimes(1);
  });
});

describe("other pages", () => {
  it("takes the list another page stored", () => {
    const { store, changes } = storeWith();
    const other = memoryStorage();
    const page = createNotificationStore({ storage: () => other });
    page.receive("demo", moved(1, "T-1", "done"), frame(true));
    page.markRead("demo");
    store.sync(storageKey("demo"), other.getItem(storageKey("demo")));
    expect(store.feed("demo").map((e) => [e.task, e.unread])).toEqual([
      ["T-1", false],
    ]);
    changes.mockClear();
    store.sync("theme", "dark");
    expect(changes).not.toHaveBeenCalled();
  });

  it("starts over when another page clears the storage", () => {
    const { store, storage, changes } = storeWith();
    store.receive("demo", moved(1, "T-1", "done"), frame(true));
    storage.items.clear();
    changes.mockClear();
    store.sync(null, null);
    expect(store.feed("demo")).toEqual([]);
    expect(changes).toHaveBeenCalledTimes(1);
    changes.mockClear();
    const empty = createNotificationStore({ storage: () => null });
    empty.sync(null, null);
  });

  it("listens to the storage events of a window", () => {
    const { store } = storeWith();
    let listener: (event: Pick<StorageEvent, "key" | "newValue">) => void =
      () => undefined;
    watchStorage(store, {
      addEventListener: (_type, added) => {
        listener = added;
      },
    });
    const value = JSON.stringify({
      entries: [],
      read: { seq: 4, at: 4_000 },
      cleared: { seq: 4, at: 4_000 },
      hidden: [],
    });
    listener({ key: storageKey("demo"), newValue: value });
    store.receive("demo", moved(3, "T-3", "done"), frame(true));
    expect(store.feed("demo")).toEqual([]);
  });
});
