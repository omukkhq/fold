/** The moves that arrive within this time of the first one share one toast. */
export const BATCH_WINDOW_MS = 1_500;

export interface Timers {
  set: (run: () => void, ms: number) => unknown;
  clear: (handle: unknown) => void;
}

const browserTimers: Timers = {
  set: (run, ms) => setTimeout(run, ms),
  clear: (handle) => {
    clearTimeout(handle as ReturnType<typeof setTimeout>);
  },
};

export interface Batcher<T> {
  add: (item: T) => void;
  /** Drops the items that wait, without a flush. */
  cancel: () => void;
}

/** The first item opens a window of `windowMs`; when it ends, every item added meanwhile goes to `flush` at once. The window does not grow with each item, so a steady flow still flushes once per window. */
export function createBatcher<T>(
  flush: (items: T[]) => void,
  windowMs: number = BATCH_WINDOW_MS,
  timers: Timers = browserTimers,
): Batcher<T> {
  let waiting: T[] = [];
  let handle: unknown = null;
  return {
    add(item) {
      waiting.push(item);
      if (handle !== null) {
        return;
      }
      handle = timers.set(() => {
        handle = null;
        const items = waiting;
        waiting = [];
        flush(items);
      }, windowMs);
    },
    cancel() {
      if (handle !== null) {
        timers.clear(handle);
        handle = null;
      }
      waiting = [];
    },
  };
}
