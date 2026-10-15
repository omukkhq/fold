import { describe, expect, it } from "vitest";
import { ApiError } from "./errors";
import { eventTransport, listEvents, openEventStream } from "./events";

describe("eventTransport", () => {
  it("opens the daemon stream when no fixture loader is given", async () => {
    const urls: string[] = [];
    const transport = eventTransport(
      "demo",
      undefined,
      (input) => {
        urls.push((input as Request).url);
        return Promise.resolve(new Response("data: x\n\n"));
      },
      "http://localhost/",
    );
    const out: string[] = [];
    for await (const chunk of await transport(
      null,
      new AbortController().signal,
    )) {
      out.push(chunk);
    }
    expect(urls).toEqual(["http://localhost/api/v1/projects/demo/events"]);
    expect(out.join("")).toBe("data: x\n\n");
  });

  it("streams the fixture script for the slug when a loader is given", async () => {
    const calls: [string, number | null][] = [];
    const transport = eventTransport("demo", () =>
      Promise.resolve({
        fixtureEventTransport: (slug: string) => (lastSeq: number | null) => {
          calls.push([slug, lastSeq]);
          return Promise.resolve(
            (async function* () {
              await Promise.resolve();
              yield "data: fixture\n\n";
            })(),
          );
        },
      }),
    );
    const out: string[] = [];
    for await (const chunk of await transport(
      7,
      new AbortController().signal,
    )) {
      out.push(chunk);
    }
    expect(calls).toEqual([["demo", 7]]);
    expect(out).toEqual(["data: fixture\n\n"]);
  });
});

const BASE = "http://localhost/";
const signal = () => new AbortController().signal;

function daemon(respond: (request: Request) => Response) {
  const requests: Request[] = [];
  const transport = eventTransport(
    "demo",
    undefined,
    (input) => {
      const request = input as Request;
      requests.push(request);
      return Promise.resolve(respond(request));
    },
    BASE,
  );
  return { requests, transport };
}

describe("the daemon transport", () => {
  it("sends Last-Event-ID and decodes split multi-byte text", async () => {
    const bytes = new TextEncoder().encode("data: é\n\n");
    const { requests, transport } = daemon(
      () =>
        new Response(
          new ReadableStream<Uint8Array>({
            start(c) {
              c.enqueue(bytes.slice(0, 7));
              c.enqueue(bytes.slice(7));
              c.close();
            },
          }),
        ),
    );
    const out: string[] = [];
    for await (const chunk of await transport(12, signal())) {
      out.push(chunk);
    }
    expect(out.join("")).toBe("data: é\n\n");
    expect(requests[0]?.headers.get("Last-Event-ID")).toBe("12");
    expect(requests[0]?.headers.get("Accept")).toBe("text/event-stream");
  });

  it("omits Last-Event-ID on the first request", async () => {
    const { requests, transport } = daemon(() => new Response(""));
    await transport(null, signal());
    expect(requests[0]?.headers.has("Last-Event-ID")).toBe(false);
  });

  it("rejects with the code of the daemon's error envelope", async () => {
    const { transport } = daemon(() =>
      Response.json(
        {
          error: {
            code: "invalid_last_event_id",
            message: "no such seq",
            details: {},
          },
        },
        { status: 400 },
      ),
    );
    const failure = transport(99, signal());
    await expect(failure).rejects.toBeInstanceOf(ApiError);
    await expect(failure).rejects.toMatchObject({
      code: "invalid_last_event_id",
      message: "no such seq",
    });
    const bare = daemon(() =>
      Response.json({ error: { code: "auth_required" } }, { status: 401 }),
    );
    await expect(bare.transport(null, signal())).rejects.toMatchObject({
      code: "auth_required",
      message: "",
    });
  });

  it("rejects with a plain error when the answer has no envelope or no body", async () => {
    for (const response of [
      new Response("Bad Gateway", { status: 502 }),
      Response.json({ error: "down" }, { status: 503 }),
      Response.json({ error: { code: 5 } }, { status: 500 }),
      new Response(null),
    ]) {
      const { transport } = daemon(() => response);
      const failure = transport(null, signal());
      await expect(failure).rejects.toBeInstanceOf(Error);
      await expect(failure).rejects.not.toBeInstanceOf(ApiError);
    }
  });
});

describe("openEventStream", () => {
  it("streams the fixture script in a development build", async () => {
    const aborted = new AbortController();
    aborted.abort();
    const chunks: string[] = [];
    for await (const chunk of await openEventStream("demo")(
      null,
      aborted.signal,
    )) {
      chunks.push(chunk);
    }
    expect(chunks).toHaveLength(1);
    expect(chunks[0]).toContain('"kind":"snapshot"');
  });
});

describe("listEvents", () => {
  it("answers empty where no fixture is loaded and for another slug", async () => {
    expect(await listEvents("other")).toEqual([]);
  });

  it("lists the fixture history of the demo project newest first", async () => {
    const events = await listEvents("demo");
    expect(events.length).toBeGreaterThan(0);
    expect(events.map((e) => e.seq)).toEqual(
      [...events.map((e) => e.seq)].sort((a, b) => b - a),
    );
  });
});
