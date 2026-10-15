import createClient from "openapi-fetch";
import { toApiError } from "./errors";
import type { paths } from "./schema";

export const API_BASE = "/api/v1";

/** The event stream request for one project; the cursor travels in `Last-Event-ID`. `Request` needs an absolute URL, so the path resolves against the page. */
export function eventStreamRequest(
  slug: string,
  lastSeq: number | null,
  signal: AbortSignal,
  base: string = location.href,
): Request {
  const headers = new Headers({ Accept: "text/event-stream" });
  if (lastSeq !== null) {
    headers.set("Last-Event-ID", String(lastSeq));
  }
  const url = new URL(
    `${API_BASE}/projects/${encodeURIComponent(slug)}/events`,
    base,
  );
  return new Request(url, { headers, signal });
}

/** An `ApiError` when the body is the daemon's error envelope, otherwise a plain `Error`: a proxy or network failure has no code to branch on. */
async function streamFailure(response: Response): Promise<Error> {
  const body: unknown = await response.json().catch(() => null);
  return (
    toApiError(body) ??
    new Error(`the daemon answered ${String(response.status)}`)
  );
}

async function* decodeBody(
  body: ReadableStream<Uint8Array>,
): AsyncGenerator<string> {
  const reader = body.getReader();
  const decoder = new TextDecoder();
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) {
        return;
      }
      yield decoder.decode(value, { stream: true });
    }
  } finally {
    await reader.cancel().catch(() => undefined);
  }
}

/** Opens the daemon's event stream with `fetch`, so the header and the retry timing stay under the caller's control, and yields its text. */
export async function fetchEventStream(
  slug: string,
  lastSeq: number | null,
  signal: AbortSignal,
  fetchImpl: typeof fetch = (request) => fetch(request),
  base?: string,
): Promise<AsyncIterable<string>> {
  const response = await fetchImpl(
    eventStreamRequest(slug, lastSeq, signal, base),
  );
  if (!response.ok) {
    throw await streamFailure(response);
  }
  if (response.body === null) {
    throw new Error("the event stream has no body");
  }
  return decodeBody(response.body);
}

/** A write to a route the generated schema does not describe yet: a JSON body out, nothing read back. Failures throw an `ApiError` when the body is the daemon's error envelope. */
export async function postJson(
  path: string,
  body: unknown,
  fetchImpl: typeof fetch = (input, init) => fetch(input, init),
): Promise<void> {
  const response = await fetchImpl(path, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    credentials: "same-origin",
    body: JSON.stringify(body),
  });
  if (!response.ok) {
    throw await streamFailure(response);
  }
}

/** The one way the web application reaches the daemon. Paths and bodies are typed by the generated schema. */
export const api = createClient<paths>({ baseUrl: "/" });
