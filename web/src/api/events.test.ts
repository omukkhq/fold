import { afterEach, describe, expect, it, vi } from "vitest";
import { errorReply, stubFetch } from "@/test/stubFetch";
import { EVENT_LIMIT } from "../live/eventList";
import { ApiError, isApiError } from "./errors";
import {
  eventTransport,
  listEventPage,
  listEvents,
  openEventStream,
} from "./events";

describe("eventTransport", () => {
  it("opens the daemon's event stream of the project", async () => {
    const urls: string[] = [];
    const transport = eventTransport(
      "demo",
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
});

const BASE = "http://localhost/";
const signal = () => new AbortController().signal;

function daemon(respond: (request: Request) => Response) {
  const requests: Request[] = [];
  const transport = eventTransport(
    "demo",
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
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("reads the daemon's stream through fetch", async () => {
    const requests: Request[] = [];
    vi.stubGlobal("location", new URL("http://localhost/p/demo/activity"));
    vi.stubGlobal("fetch", (request: Request) => {
      requests.push(request);
      return Promise.resolve(new Response("data: live\n\n"));
    });
    const chunks: string[] = [];
    for await (const chunk of await openEventStream("demo")(7, signal())) {
      chunks.push(chunk);
    }
    expect(chunks.join("")).toBe("data: live\n\n");
    expect(new URL(requests[0]?.url ?? "").pathname).toBe(
      "/api/v1/projects/demo/events",
    );
    expect(requests[0]?.headers.get("Last-Event-ID")).toBe("7");
  });
});

describe("listEvents", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("reads the newest stored events, as many as the sidebar keeps", async () => {
    const event = {
      seq: 9,
      type: "task.created",
      actor: "frontend",
      task: "T-1",
      proposal: null,
      workspace: null,
      revision: null,
      data: {},
      created_at: 1,
    };
    const calls = stubFetch({
      "GET /api/v1/projects/demo/events/history": {
        body: { events: [event], before: null },
      },
    });
    expect(await listEvents("demo")).toEqual([event]);
    expect(calls[0]?.search.get("limit")).toBe(String(EVENT_LIMIT));
  });

  it("rejects with the daemon's code", async () => {
    stubFetch({
      "GET /api/v1/projects/demo/events/history": errorReply(
        404,
        "project_not_found",
      ),
    });
    await expect(listEvents("demo")).rejects.toSatisfy((e: unknown) =>
      isApiError(e, "project_not_found"),
    );
  });
});

describe("listEventPage", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("reads the newest page, then the page before a seq", async () => {
    const calls = stubFetch({
      "GET /api/v1/projects/demo/events/history": {
        body: { events: [], before: null },
      },
    });
    expect(await listEventPage("demo", null, 500)).toEqual({
      events: [],
      before: null,
    });
    await listEventPage("demo", 40, 500);
    expect(
      calls.map((c) => [c.search.get("before"), c.search.get("limit")]),
    ).toEqual([
      [null, "500"],
      ["40", "500"],
    ]);
  });
});
