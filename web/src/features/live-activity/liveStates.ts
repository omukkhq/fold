import { matchQuery, type QueryClient } from "@tanstack/react-query";
import type { LiveStatus } from "../../live/status";
import { keys } from "../../api/keys";

export interface LiveStatesStore {
  subscribe: (listener: () => void) => () => void;
  /** Changes whenever a live workspace entry of the project is set or removed. */
  getVersion: () => number;
  read: (workspace: string) => unknown;
}

/**
 * A query observer misses `removeQueries`, which is how a snapshot drops a workspace, so the screen listens to the cache itself.
 */
export function createLiveStatesStore(
  queryClient: QueryClient,
  slug: string,
): LiveStatesStore {
  let version = 0;
  const prefix = keys.workspaceLiveAll(slug);
  return {
    subscribe(listener) {
      return queryClient.getQueryCache().subscribe((event) => {
        if (matchQuery({ queryKey: prefix }, event.query)) {
          version += 1;
          listener();
        }
      });
    },
    getVersion: () => version,
    read: (workspace) =>
      queryClient.getQueryData(keys.workspaceLive(slug, workspace)),
  };
}

/** The time of the state on screen while the stream is down: the last snapshot, else the last event, else the time of the render. */
export function shownAsOf(
  live: Pick<LiveStatus, "stateAsOf" | "lastEventAt">,
  now: number,
): number {
  return live.stateAsOf ?? live.lastEventAt ?? now;
}
