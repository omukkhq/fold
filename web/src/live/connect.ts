import type { QueryClient } from "@tanstack/react-query";
import { applyEvent, invalidateProject } from "./apply";
import { isSnapshot } from "./envelope";
import type { LiveStatusStore } from "./status";
import { startStream, type StreamHandle, type Transport } from "./stream";

/** The newest stored event applied to each cache for each project, kept past the stream so that returning to a project resumes after it and replays what happened meanwhile. */
const cursors = new WeakMap<QueryClient, Map<string, number>>();

function cursorsOf(queryClient: QueryClient): Map<string, number> {
  let map = cursors.get(queryClient);
  if (!map) {
    map = new Map();
    cursors.set(queryClient, map);
  }
  return map;
}

/** Opens the stream of one project: events go to the Query cache and the connection state goes to the status store. `onAuthRequired` runs once when the daemon ends the stream with `auth_required`, and `onProjectGone` once when it answers `project_not_found`. */
export function connectProject(
  queryClient: QueryClient,
  slug: string,
  transport: Transport,
  status: LiveStatusStore,
  now: () => number = Date.now,
  onAuthRequired: () => void = () => undefined,
  onProjectGone: () => void = () => undefined,
): StreamHandle {
  status.reset();
  const saved = cursorsOf(queryClient);
  const handle = startStream({
    transport,
    lastSeq: saved.get(slug) ?? null,
    handlers: {
      onOpen: () => {
        status.set({ state: "live", retryAt: null });
      },
      onEvent: (event) => {
        applyEvent(queryClient, slug, event);
        // A worker step arrives many times a second and changes nothing a screen draws from this clock.
        if (event.kind === "worker_output") {
          return;
        }
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
      onAuthRequired,
      onProjectGone,
    },
  });
  return {
    ...handle,
    stop() {
      handle.stop();
      const seq = handle.seq();
      if (seq === null) {
        saved.delete(slug);
      } else {
        saved.set(slug, seq);
      }
    },
  };
}
