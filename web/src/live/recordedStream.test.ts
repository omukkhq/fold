import { QueryClient } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { eventTransport } from "../api/events";
import { keys } from "../api/keys";
import { connectProject } from "./connect";
import { isEventEnvelope, isRecord, type EventEnvelope } from "./envelope";
import recorded from "./recorded/stream.json";
import { createLiveStatusStore } from "./status";

/** One frame as the daemon writes it: the SSE `id` of a stored event, the SSE `event` name and the `data` envelope. */
interface Frame {
  id?: number;
  event: string;
  data: unknown;
}

interface Connection {
  last_event_id: number | null;
  frames: Frame[];
}

function isFrame(value: unknown): value is Frame {
  return (
    isRecord(value) &&
    typeof value.event === "string" &&
    (value.id === undefined || typeof value.id === "number") &&
    "data" in value
  );
}

function connection(value: unknown): Connection {
  if (
    !isRecord(value) ||
    !Array.isArray(value.frames) ||
    !value.frames.every(isFrame) ||
    !(value.last_event_id === null || typeof value.last_event_id === "number")
  ) {
    throw new Error("not a recorded connection");
  }
  return { last_event_id: value.last_event_id, frames: value.frames };
}

const RECORDED: unknown = recorded;
const FIRST = connection(isRecord(RECORDED) ? RECORDED.first : null);
const SECOND = connection(isRecord(RECORDED) ? RECORDED.second : null);

/** The text of a frame on the wire, as axum's SSE writer lays it out. */
function wire(frame: Frame): string {
  const id = frame.id === undefined ? "" : `id: ${String(frame.id)}\n`;
  return `${id}event: ${frame.event}\ndata: ${JSON.stringify(frame.data)}\n\n`;
}

/** A response body the test feeds frame by frame and closes, as a daemon that stops would. */
class Body {
  private controller: ReadableStreamDefaultController<Uint8Array> | null = null;
  readonly stream = new ReadableStream<Uint8Array>({
    start: (controller) => {
      this.controller = controller;
    },
  });

  send(frames: readonly Frame[]): void {
    for (const frame of frames) {
      this.controller?.enqueue(new TextEncoder().encode(wire(frame)));
    }
  }

  close(): void {
    this.controller?.close();
  }
}

const BASE = "http://localhost/";
const SLUG = "demo";

function setUp() {
  const client = new QueryClient();
  const status = createLiveStatusStore();
  const requests: Request[] = [];
  const bodies: Body[] = [];
  const transport = eventTransport(
    SLUG,
    (input) => {
      const request = input as Request;
      requests.push(request);
      const body = new Body();
      bodies.push(body);
      return Promise.resolve(
        new Response(body.stream, {
          headers: { "Content-Type": "text/event-stream" },
        }),
      );
    },
    BASE,
  );
  let clock = 1_000;
  const handle = connectProject(client, SLUG, transport, status, () => clock);
  return {
    client,
    status,
    requests,
    bodies,
    handle,
    tick: (ms: number) => {
      clock += ms;
    },
  };
}

function liveFiles(client: QueryClient, workspace: string): unknown {
  const live = client.getQueryData(keys.workspaceLive(SLUG, workspace));
  return isRecord(live) && Array.isArray(live.files)
    ? live.files.map((f: unknown) => (isRecord(f) ? f.path : null))
    : undefined;
}

function storedSeqs(client: QueryClient): number[] {
  const events = client.getQueryData(keys.events(SLUG));
  return Array.isArray(events)
    ? events.map((e: unknown) => (isRecord(e) ? Number(e.seq) : -1))
    : [];
}

beforeEach(() => {
  vi.useFakeTimers();
});
afterEach(() => {
  vi.useRealTimers();
});

describe("the recorded stream", () => {
  it("holds only frames the generated envelope type accepts, each named as the daemon names it", () => {
    for (const frame of [...FIRST.frames, ...SECOND.frames]) {
      expect(isEventEnvelope(frame.data), frame.event).toBe(true);
      const envelope = frame.data as EventEnvelope;
      if (envelope.kind === "event") {
        expect(frame.event).toBe(envelope.type);
        expect(frame.id).toBe(envelope.seq);
      } else {
        expect(frame.id).toBeUndefined();
      }
    }
  });

  it("replays the gap after Last-Event-ID, then sends one snapshot, then live frames", () => {
    const kinds = SECOND.frames.map((f) => (f.data as EventEnvelope).kind);
    const snapshot = kinds.indexOf("snapshot");
    expect(kinds.slice(0, snapshot).every((k) => k === "event")).toBe(true);
    expect(kinds.filter((k) => k === "snapshot")).toHaveLength(1);
    const replayed = SECOND.frames
      .slice(0, snapshot)
      .map((f) => (f.data as EventEnvelope & { seq: number }).seq);
    expect(replayed[0]).toBe((SECOND.last_event_id ?? 0) + 1);
  });
});

describe("the stream client against the recorded stream", () => {
  it("shows a working file within the first connection, without a reload", async () => {
    const { client, status, requests, bodies, handle } = setUp();
    await vi.advanceTimersByTimeAsync(0);
    expect(requests[0]?.headers.has("Last-Event-ID")).toBe(false);
    expect(status.getSnapshot().state).toBe("connecting");
    bodies[0]?.send(FIRST.frames);
    await vi.advanceTimersByTimeAsync(0);
    expect(status.getSnapshot().state).toBe("live");
    expect(liveFiles(client, "ws-1")).toEqual(["src/login.tsx"]);
    expect(liveFiles(client, "ws-2")).toEqual(["api/src/lib.rs"]);
    handle.stop();
  });

  it("reconnects with Last-Event-ID, stays reconnecting until the first frame, and applies the missed events once each", async () => {
    const { client, status, requests, bodies, handle, tick } = setUp();
    client.setQueryData(keys.events(SLUG), []);
    await vi.advanceTimersByTimeAsync(0);
    bodies[0]?.send(FIRST.frames);
    await vi.advanceTimersByTimeAsync(0);
    expect(storedSeqs(client)).toEqual([42]);

    tick(10);
    bodies[0]?.close();
    await vi.advanceTimersByTimeAsync(0);
    expect(status.getSnapshot().state).toBe("reconnecting");
    // A connection that delivered frames retries after at most the one-second base delay.
    await vi.advanceTimersByTimeAsync(1_000);
    expect(requests).toHaveLength(2);
    expect(requests[1]?.headers.get("Last-Event-ID")).toBe(
      String(SECOND.last_event_id),
    );
    // The connection is open, but nothing proves the daemon is back until its first frame.
    expect(status.getSnapshot().state).toBe("reconnecting");

    const snapshot = SECOND.frames.findIndex(
      (f) => (f.data as EventEnvelope).kind === "snapshot",
    );
    bodies[1]?.send(SECOND.frames.slice(0, 1));
    await vi.advanceTimersByTimeAsync(0);
    expect(status.getSnapshot().state).toBe("live");
    bodies[1]?.send(SECOND.frames.slice(1, snapshot));
    await vi.advanceTimersByTimeAsync(0);
    expect(storedSeqs(client)).toEqual([44, 43, 42]);
    // The workspace archived while the client was away keeps its state only until the snapshot.
    expect(liveFiles(client, "ws-2")).toBeUndefined();

    bodies[1]?.send(SECOND.frames.slice(snapshot));
    await vi.advanceTimersByTimeAsync(0);
    expect(storedSeqs(client)).toEqual([45, 44, 43, 42]);
    expect(liveFiles(client, "ws-1")).toEqual([
      "src/login.tsx",
      "src/forms.ts",
    ]);
    expect(status.getSnapshot().stateAsOf).not.toBeNull();
    handle.stop();
  });

  it("drops a workspace the snapshot no longer holds", async () => {
    const { client, bodies, handle } = setUp();
    await vi.advanceTimersByTimeAsync(0);
    bodies[0]?.send(FIRST.frames);
    await vi.advanceTimersByTimeAsync(0);
    expect(liveFiles(client, "ws-2")).toBeDefined();
    const snapshot = SECOND.frames.find(
      (f) => (f.data as EventEnvelope).kind === "snapshot",
    );
    bodies[0]?.send(snapshot ? [snapshot] : []);
    await vi.advanceTimersByTimeAsync(0);
    expect(liveFiles(client, "ws-2")).toBeUndefined();
    expect(liveFiles(client, "ws-3")).toEqual([]);
    handle.stop();
  });

  it("keeps a workspace ended by a stored event gone when a late change frame follows", async () => {
    const { client, bodies, handle } = setUp();
    await vi.advanceTimersByTimeAsync(0);
    bodies[0]?.send([...FIRST.frames, ...SECOND.frames]);
    await vi.advanceTimersByTimeAsync(0);
    const last = SECOND.frames.at(-1)?.data as EventEnvelope;
    expect(last).toMatchObject({
      kind: "workspace_changed",
      workspace: "ws-3",
    });
    expect(liveFiles(client, "ws-3")).toBeUndefined();
    handle.stop();
  });

  it("reads an open working diff again when its workspace changes, and leaves other diffs alone", async () => {
    const { client, bodies, handle } = setUp();
    client.setQueryData(keys.comparison(SLUG, "r7", "ws-1"), { files: [] });
    client.setQueryData(keys.fileDiff(SLUG, "r7", "ws-1", "src/login.tsx"), {
      hunks: [],
    });
    client.setQueryData(keys.comparison(SLUG, "r7", "ws-9"), { files: [] });
    client.setQueryData(keys.comparison(SLUG, "r6", "r7"), { files: [] });
    await vi.advanceTimersByTimeAsync(0);
    const changed = FIRST.frames.filter(
      (f) => (f.data as EventEnvelope).kind === "workspace_changed",
    );
    bodies[0]?.send(changed);
    await vi.advanceTimersByTimeAsync(0);
    const invalid = (key: readonly unknown[]) =>
      client.getQueryState(key)?.isInvalidated;
    expect(invalid(keys.comparison(SLUG, "r7", "ws-1"))).toBe(true);
    expect(invalid(keys.fileDiff(SLUG, "r7", "ws-1", "src/login.tsx"))).toBe(
      true,
    );
    expect(invalid(keys.comparison(SLUG, "r7", "ws-9"))).toBe(false);
    expect(invalid(keys.comparison(SLUG, "r6", "r7"))).toBe(false);
    handle.stop();
  });
});
