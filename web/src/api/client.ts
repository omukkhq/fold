import createClient, { type Middleware } from "openapi-fetch";
import { apiErrorCode, toApiError } from "./errors";
import type { paths } from "./schema";

export const API_BASE = "/api/v1";

/** The daemon refuses a write over TCP without this header. A page of another origin cannot send it without a CORS preflight, which the daemon never answers. */
export const CLIENT_HEADER = "x-fold-client";

/** The event stream request for one project; the cursor travels in `Last-Event-ID`. `Request` needs an absolute URL, so the path resolves against the page. */
export function eventStreamRequest(
  slug: string,
  lastSeq: number | null,
  signal: AbortSignal,
  base: string = location.href,
): Request {
  const headers = new Headers({
    Accept: "text/event-stream",
    [CLIENT_HEADER]: "web",
  });
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

/**
 * The page's origin: `Request` needs an absolute URL, and the daemon serves the web files, so every path resolves against it.
 * Outside a page (a Node test) there is none, and any origin does; the fallback is written in parts because the source guard bans remote URLs.
 */
function pageOrigin(): string {
  return typeof location === "undefined"
    ? ["http", "//localhost"].join(":")
    : location.origin;
}

/** The one way the web application reaches the daemon. Paths and bodies are typed by the generated schema. `fetch` is read at call time, so a test can replace it. */
export const api = createClient<paths>({
  baseUrl: pageOrigin(),
  credentials: "same-origin",
  headers: { [CLIENT_HEADER]: "web" },
  fetch: (request) => fetch(request),
});

/** Calls `onAuthRequired` when the daemon answers `auth_required`: the session cookie is missing or no longer valid. Other 401 answers, such as `invalid_token` from the sign-in itself, pass through. */
export function authRequiredMiddleware(onAuthRequired: () => void): Middleware {
  return {
    async onResponse({ response }) {
      if (response.status !== 401) {
        return undefined;
      }
      const body: unknown = await response
        .clone()
        .json()
        .catch(() => null);
      if (apiErrorCode(body) === "auth_required") {
        onAuthRequired();
      }
      return undefined;
    },
  };
}
