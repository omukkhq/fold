import { useSyncExternalStore } from "react";
import type { EventEnvelope } from "@/live/envelope";
import { createBatcher, type Batcher, type Timers } from "./batch";
import {
  EMPTY_FEED,
  addEntries,
  clearAll,
  clearEntry,
  clearTask,
  entryKey,
  keeps,
  markRead,
  mergeHistory,
  parseFeed,
  removeTask,
  type FeedEntry,
  type FeedState,
} from "./feed";
import { loadMoves, type PageReader } from "./history";
import {
  createMoveReader,
  deletedTask,
  type CardMove,
  type MoveReader,
} from "./moves";
import { toastText } from "./text";

/** At most this many toasts show at once; an older one closes when a newer one arrives. */
export const TOAST_LIMIT = 3;

export interface MoveToast {
  id: number;
  slug: string;
  text: string;
}

type FeedStorage = Pick<Storage, "getItem" | "setItem">;

export interface NotificationOptions {
  /** The browser's storage, or null when it cannot be reached. A refused read starts an empty list and a refused write keeps the list in memory only. */
  storage: () => FeedStorage | null;
  timers?: Timers;
  windowMs?: number;
}

/** What comes with one frame of a project's stream. */
export interface Frame {
  /** True for a stored event the stream sends before its snapshot, which happened while no stream was open: it joins the list but shows no toast. */
  replay: boolean;
  /** The task's title in the cached board, or null. */
  title: (task: string) => string | null;
}

/** Where a fill reads the daemon's history and what it adds to each move. */
export interface HistorySource {
  read: PageReader;
  title: (task: string) => string | null;
}

export interface NotificationStore {
  subscribe: (listener: () => void) => () => void;
  /** The project's list, newest first; the same array until it changes. */
  feed: (slug: string) => readonly FeedEntry[];
  toasts: () => readonly MoveToast[];
  /** Takes one frame of the project's stream. */
  receive: (slug: string, event: EventEnvelope, frame: Frame) => void;
  /** Fills the project's list from the daemon's history once per stream: the moves made while no page was open join it without a toast, and the stored entries the history does not hold leave. The source is kept, so {@link refill} can try again after a failure. */
  fill: (slug: string, source: HistorySource) => Promise<void>;
  /** Fills the list again from the last source when no fill succeeded since the stream opened. */
  refill: (slug: string) => Promise<void>;
  /** The project's stream closed: the moves that wait for a toast are dropped and its toasts close. */
  leave: (slug: string) => void;
  /** Another page of this browser stored a list: `key` and `value` of the `storage` event. */
  sync: (key: string | null, value: string | null) => void;
  markRead: (slug: string) => void;
  clear: (slug: string, key: string) => void;
  /** Clears every move of one task, the Inbox row that groups them. */
  clearTask: (slug: string, task: string) => void;
  clearAll: (slug: string) => void;
  dismiss: (id: number) => void;
}

type TitledMove = CardMove & { title: string | null };

const PREFIX = "fold.notifications.";

export function storageKey(slug: string): string {
  return `${PREFIX}${slug}`;
}

type FillState = "idle" | "loading" | "done";

interface Filler extends HistorySource {
  state: FillState;
}

export function createNotificationStore(
  options: NotificationOptions,
): NotificationStore {
  const feeds = new Map<string, FeedState>();
  const readers = new Map<string, MoveReader>();
  const batchers = new Map<string, Batcher<TitledMove>>();
  const fillers = new Map<string, Filler>();
  /** The keys of the entries the stream brought since the project's stream opened. */
  const lives = new Map<string, Set<string>>();
  const listeners = new Set<() => void>();
  let toasts: readonly MoveToast[] = [];
  let nextToast = 1;

  function emit(): void {
    for (const listener of [...listeners]) {
      listener();
    }
  }

  function state(slug: string): FeedState {
    let found = feeds.get(slug);
    if (found === undefined) {
      try {
        found = parseFeed(options.storage()?.getItem(storageKey(slug)) ?? null);
      } catch {
        found = EMPTY_FEED;
      }
      feeds.set(slug, found);
    }
    return found;
  }

  function setState(slug: string, next: FeedState): void {
    if (next === state(slug)) {
      return;
    }
    feeds.set(slug, next);
    try {
      options.storage()?.setItem(storageKey(slug), JSON.stringify(next));
    } catch {
      // The list stays in memory for this page.
    }
    emit();
  }

  function live(slug: string): Set<string> {
    let found = lives.get(slug);
    if (found === undefined) {
      found = new Set();
      lives.set(slug, found);
    }
    return found;
  }

  function batcher(slug: string): Batcher<TitledMove> {
    let found = batchers.get(slug);
    if (found === undefined) {
      found = createBatcher<TitledMove>(
        (moves) => {
          const text = toastText(moves);
          if (text === "") {
            return;
          }
          toasts = [...toasts, { id: nextToast++, slug, text }].slice(
            -TOAST_LIMIT,
          );
          emit();
        },
        options.windowMs,
        options.timers,
      );
      batchers.set(slug, found);
    }
    return found;
  }

  function reader(slug: string): MoveReader {
    let found = readers.get(slug);
    if (found === undefined) {
      found = createMoveReader();
      readers.set(slug, found);
    }
    return found;
  }

  async function run(slug: string, filler: Filler): Promise<void> {
    filler.state = "loading";
    let moves: CardMove[];
    try {
      moves = await loadMoves(filler.read, (move) => keeps(state(slug), move));
    } catch {
      if (fillers.get(slug) === filler) {
        filler.state = "idle";
      }
      return;
    }
    if (fillers.get(slug) !== filler) {
      return;
    }
    filler.state = "done";
    setState(
      slug,
      mergeHistory(
        state(slug),
        moves.map((move) => ({ ...move, title: filler.title(move.task) })),
        live(slug),
      ),
    );
  }

  return {
    subscribe(listener) {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
    feed: (slug) => state(slug).entries,
    toasts: () => toasts,
    receive(slug, event, frame) {
      const gone = deletedTask(event);
      if (gone !== null) {
        setState(slug, removeTask(state(slug), gone));
      }
      const move = reader(slug)(event);
      if (move === null) {
        return;
      }
      const entry: TitledMove = { ...move, title: frame.title(move.task) };
      const before = state(slug);
      const after = addEntries(before, [entry]);
      if (after === before) {
        return;
      }
      live(slug).add(entryKey(move));
      setState(slug, after);
      if (!frame.replay) {
        batcher(slug).add(entry);
      }
    },
    fill(slug, source) {
      const filler: Filler = { ...source, state: "idle" };
      fillers.set(slug, filler);
      return run(slug, filler);
    },
    async refill(slug) {
      const filler = fillers.get(slug);
      if (filler?.state === "idle") {
        await run(slug, filler);
      }
    },
    leave(slug) {
      batchers.get(slug)?.cancel();
      fillers.delete(slug);
      lives.delete(slug);
      const kept = toasts.filter((toast) => toast.slug !== slug);
      if (kept.length !== toasts.length) {
        toasts = kept;
        emit();
      }
    },
    sync(key, value) {
      if (key === null) {
        if (feeds.size > 0) {
          feeds.clear();
          emit();
        }
        return;
      }
      if (!key.startsWith(PREFIX)) {
        return;
      }
      feeds.set(key.slice(PREFIX.length), parseFeed(value));
      emit();
    },
    markRead(slug) {
      setState(slug, markRead(state(slug)));
    },
    clear(slug, key) {
      setState(slug, clearEntry(state(slug), key));
    },
    clearTask(slug, task) {
      setState(slug, clearTask(state(slug), task));
    },
    clearAll(slug) {
      setState(slug, clearAll(state(slug)));
    },
    dismiss(id) {
      const kept = toasts.filter((toast) => toast.id !== id);
      if (kept.length !== toasts.length) {
        toasts = kept;
        emit();
      }
    },
  };
}

export interface StorageEvents {
  addEventListener(
    type: "storage",
    listener: (event: Pick<StorageEvent, "key" | "newValue">) => void,
  ): void;
}

/** Keeps the store in step with the other pages of this browser, which share its storage. */
export function watchStorage(
  store: NotificationStore,
  target: StorageEvents,
): void {
  target.addEventListener("storage", (event) => {
    store.sync(event.key, event.newValue);
  });
}

function browserStorage(): FeedStorage | null {
  try {
    return window.localStorage;
  } catch {
    return null;
  }
}

/** One stream is open at a time, so one store serves the whole application. */
export const notificationStore = createNotificationStore({
  storage: browserStorage,
});

if (typeof window !== "undefined") {
  watchStorage(notificationStore, window);
}

export function useFeed(slug: string): readonly FeedEntry[] {
  const read = () => notificationStore.feed(slug);
  return useSyncExternalStore(notificationStore.subscribe, read, read);
}

export function useMoveToasts(): readonly MoveToast[] {
  return useSyncExternalStore(
    notificationStore.subscribe,
    notificationStore.toasts,
    notificationStore.toasts,
  );
}
