import { ApiError } from "./errors";

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

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

/** An `ApiError` when the body is the daemon's error envelope, otherwise a plain `Error`: a proxy or network failure has no code to branch on. */
async function streamFailure(response: Response): Promise<Error> {
  const body: unknown = await response.json().catch(() => null);
  if (
    isRecord(body) &&
    isRecord(body.error) &&
    typeof body.error.code === "string"
  ) {
    const message =
      typeof body.error.message === "string" ? body.error.message : "";
    return new ApiError(body.error.code, message);
  }
  return new Error(`the event stream answered ${String(response.status)}`);
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
