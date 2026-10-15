import { EVENT_LIMIT } from "../live/eventList";
import type { Transport } from "../live/stream";
import { api, fetchEventStream } from "./client";
import { unwrap } from "./result";
import type { components } from "./schema";

/** The transport of one project's event stream on the daemon. `fetchImpl` and `base` let a test answer the requests. */
export function eventTransport(
  slug: string,
  fetchImpl?: typeof fetch,
  base?: string,
): Transport {
  return (lastSeq, signal) =>
    fetchEventStream(slug, lastSeq, signal, fetchImpl, base);
}

/** The transport of one project's event stream. */
export function openEventStream(slug: string): Transport {
  return eventTransport(slug);
}

/** A stored event as the stream and the event list carry it. */
export type DurableEvent = components["schemas"]["DurableEvent"];

/** The project's newest stored events, newest first, as many as the sidebar keeps; the stream's stored events extend this list in the cache. */
export async function listEvents(slug: string): Promise<DurableEvent[]> {
  const { events } = await unwrap(
    api.GET("/api/v1/projects/{slug}/events/history", {
      params: { path: { slug }, query: { limit: EVENT_LIMIT } },
    }),
  );
  return events;
}
