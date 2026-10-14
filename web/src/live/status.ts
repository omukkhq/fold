import { useSyncExternalStore } from "react";

export type ConnectionState = "connecting" | "live" | "reconnecting";

export interface LiveStatus {
  state: ConnectionState;
  /** When the last stored or live event arrived, in epoch milliseconds. */
  lastEventAt: number | null;
  /** When the last snapshot of the live workspace states arrived. */
  stateAsOf: number | null;
  /** While reconnecting, when the next attempt starts, from the stream's backoff delay. */
  retryAt: number | null;
}

export const INITIAL_STATUS: LiveStatus = {
  state: "connecting",
  lastEventAt: null,
  stateAsOf: null,
  retryAt: null,
};

export interface LiveStatusStore {
  getSnapshot: () => LiveStatus;
  subscribe: (listener: () => void) => () => void;
  set: (patch: Partial<LiveStatus>) => void;
  reset: () => void;
}

export function createLiveStatusStore(): LiveStatusStore {
  let current = INITIAL_STATUS;
  const listeners = new Set<() => void>();

  function replace(next: LiveStatus): void {
    current = next;
    for (const listener of [...listeners]) {
      listener();
    }
  }

  return {
    getSnapshot: () => current,
    subscribe(listener) {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
    set(patch) {
      const next = { ...current, ...patch };
      if (
        next.state !== current.state ||
        next.lastEventAt !== current.lastEventAt ||
        next.stateAsOf !== current.stateAsOf ||
        next.retryAt !== current.retryAt
      ) {
        replace(next);
      }
    },
    reset() {
      replace(INITIAL_STATUS);
    },
  };
}

/** One stream is open per project, so one store serves the whole application. */
export const liveStatusStore = createLiveStatusStore();

export function useLiveStatus(): LiveStatus {
  return useSyncExternalStore(
    liveStatusStore.subscribe,
    liveStatusStore.getSnapshot,
    liveStatusStore.getSnapshot,
  );
}
