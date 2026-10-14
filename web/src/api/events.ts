import type { Transport } from "../live/stream";
import { fetchEventStream } from "./client";

interface EventFixture {
  fixtureEventTransport(slug: string): Transport;
}

const fixture = import.meta.env.DEV
  ? () => import("../../fixtures/events")
  : undefined;

/**
 * A development build streams the scripted events from web/fixtures; a production build holds no fixture data and opens the daemon's event stream.
 * `openEventStream` passes the loader written inline behind `import.meta.env.DEV`, because Vite drops the fixture module only where the condition is the literal flag.
 */
export function eventTransport(
  slug: string,
  load: (() => Promise<EventFixture>) | undefined,
  fetchImpl?: typeof fetch,
  base?: string,
): Transport {
  if (!load) {
    return (lastSeq, signal) =>
      fetchEventStream(slug, lastSeq, signal, fetchImpl, base);
  }
  return async (lastSeq, signal) =>
    (await load()).fixtureEventTransport(slug)(lastSeq, signal);
}

/** The transport of one project's event stream. */
export function openEventStream(slug: string): Transport {
  return eventTransport(slug, fixture);
}
