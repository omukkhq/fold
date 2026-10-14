import type { QueryClient } from "@tanstack/react-query";
import { applyEvent, invalidateProject } from "./apply";
import { isSnapshot } from "./envelope";
import type { LiveStatusStore } from "./status";
import { startStream, type StreamHandle, type Transport } from "./stream";

/** Opens the stream of one project: events go to the Query cache and the connection state goes to the status store. */
export function connectProject(
  queryClient: QueryClient,
  slug: string,
  transport: Transport,
  status: LiveStatusStore,
  now: () => number = Date.now,
): StreamHandle {
  status.reset();
  return startStream({
    transport,
    handlers: {
      onOpen: () => {
        status.set({ state: "live", retryAt: null });
      },
      onEvent: (event) => {
        applyEvent(queryClient, slug, event);
        const at = now();
        status.set(
          isSnapshot(event)
            ? { lastEventAt: at, stateAsOf: at }
            : { lastEventAt: at },
        );
      },
      onDrop: (delayMs) => {
        status.set({ state: "reconnecting", retryAt: now() + delayMs });
      },
      onResync: () => {
        invalidateProject(queryClient, slug);
      },
    },
  });
}
