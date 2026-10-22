import { useSyncExternalStore } from "react";
import type { EventEnvelope } from "@/live/envelope";
import { createBatcher, type Batcher, type Timers } from "./batch";
import {
  addEntries,
  clearEntry,
  markRead,
  parseFeed,
  type FeedEntry,
} from "./feed";
import { createMoveReader, type CardMove, type MoveReader } from "./moves";
import { toastParts } from "./text";

/** At most this many toasts show at once; an older one closes when a newer one arrives. */
export const TOAST_LIMIT = 3;

export interface MoveToast {
  id: number;
  slug: string;
  /** One part per task, shown joined by a middle dot. */
  parts: string[];
}

type FeedStorage = Pick<Storage, "getItem" | "setItem">;

export interface NotificationOptions {
  /** The browser's storage, or null when it cannot be reached. A refused read starts an empty list and a refused write keeps the list in memory only. */
  storage: () => FeedStorage | null;
  timers?: Timers;
  windowMs?: number;
}

export interface NotificationStore {
  subscribe: (listener: () => void) => () => void;
  /** The project's list, newest first; the same array until it changes. */
  feed: (slug: string) => readonly FeedEntry[];
  toasts: () => readonly MoveToast[];
  /** Takes one frame of the project's stream. A move that `replay` marks happened while the stream was closed: it joins the list but shows no toast. */
  receive: (
    slug: string,
    event: EventEnvelope,
    replay: boolean,
    title: (task: string) => string | null,
  ) => void;
  /** The project's stream closed: the moves that wait for a toast are dropped and its toasts close. */
  leave: (slug: string) => void;
  markRead: (slug: string) => void;
  clear: (slug: string, key: string) => void;
  clearAll: (slug: string) => void;
  dismiss: (id: number) => void;
}

export function storageKey(slug: string): string {
  return `fold.notifications.${slug}`;
}

export function createNotificationStore(
  options: NotificationOptions,
): NotificationStore {
  const feeds = new Map<string, readonly FeedEntry[]>();
  const readers = new Map<string, MoveReader>();
  const batchers = new Map<string, Batcher<CardMove>>();
  const listeners = new Set<() => void>();
  let toasts: readonly MoveToast[] = [];
  let nextToast = 1;

  function emit(): void {
    for (const listener of [...listeners]) {
      listener();
    }
  }

  function feed(slug: string): readonly FeedEntry[] {
    let list = feeds.get(slug);
    if (list === undefined) {
      try {
        list = parseFeed(options.storage()?.getItem(storageKey(slug)) ?? null);
      } catch {
        list = [];
      }
      feeds.set(slug, list);
    }
    return list;
  }

  function setFeed(slug: string, list: readonly FeedEntry[]): void {
    if (list === feed(slug)) {
      return;
    }
    feeds.set(slug, list);
    try {
      options.storage()?.setItem(storageKey(slug), JSON.stringify(list));
    } catch {
      // The list stays in memory for this page.
    }
    emit();
  }

  function batcher(slug: string): Batcher<CardMove> {
    let found = batchers.get(slug);
    if (found === undefined) {
      found = createBatcher<CardMove>(
        (moves) => {
          toasts = [
            ...toasts,
            { id: nextToast++, slug, parts: toastParts(moves) },
          ].slice(-TOAST_LIMIT);
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

  return {
    subscribe(listener) {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
    feed,
    toasts: () => toasts,
    receive(slug, event, replay, title) {
      const move = reader(slug)(event);
      if (move === null) {
        return;
      }
      const before = feed(slug);
      const after = addEntries(before, [
        { ...move, title: title(move.task), unread: true },
      ]);
      if (after.length === before.length && after[0] === before[0]) {
        return;
      }
      setFeed(slug, after);
      if (!replay) {
        batcher(slug).add(move);
      }
    },
    leave(slug) {
      batchers.get(slug)?.cancel();
      const kept = toasts.filter((toast) => toast.slug !== slug);
      if (kept.length !== toasts.length) {
        toasts = kept;
        emit();
      }
    },
    markRead(slug) {
      setFeed(slug, markRead(feed(slug)));
    },
    clear(slug, key) {
      setFeed(slug, clearEntry(feed(slug), key));
    },
    clearAll(slug) {
      if (feed(slug).length > 0) {
        setFeed(slug, []);
      }
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
