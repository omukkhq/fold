import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fixtureEventTransport } from "../../fixtures/events";
import { nextDelay } from "./backoff";
import type { EventEnvelope } from "./envelope";
import { ApiError } from "../api/errors";
import {
  DEAD_AFTER_MS,
  startStream,
  type StreamHandle,
  type Transport,
} from "./stream";

beforeEach(() => {
  vi.useFakeTimers();
});
afterEach(() => {
  vi.useRealTimers();
});

const frame = (seq: number, type = "task.created") =>
  `id: ${String(seq)}\ndata: ${JSON.stringify({
    kind: "event",
    seq,
    type,
    actor: "frontend/ui",
    task: "T-1",
    proposal: null,
    workspace: null,
    revision: null,
    data: {},
    created_at: 1_760_000_000_000,
  })}\n\n`;
/** A `workspace_changed` frame has no `seq` and no `id` line, because the daemon never stores it. */
const live = () =>
  `data: ${JSON.stringify({
    kind: "workspace_changed",
    workspace: "ws-1",
    task: "T-1",
    worker: "frontend/ui",
    base: "r1",
    files: [],
    summary: "no changes",
    last_activity_at: 1_760_000_000_000,
    checkpoints: [],
    proposal: null,
    stalled: false,
  })}\n\n`;
/** An `agent_presence_changed` frame is live too: no `seq`, no `id` line. */
const presence = () =>
  `data: ${JSON.stringify({
    kind: "agent_presence_changed",
    agent: "frontend",
    presence: "idle",
  })}\n\n`;
const snap = (seq: number) =>
  `data: ${JSON.stringify({ kind: "snapshot", seq, workspaces: [] })}\n\n`;
const label = (e: EventEnvelope) => {
  if (e.kind === "workspace_changed") {
    return "live";
  }
  if (e.kind === "agent_presence_changed") {
    return `presence ${e.agent} ${e.presence}`;
  }
  return `event ${String(e.seq)}`;
};

/** An in-memory connection the test feeds by hand. */
class Pipe {
  private queue: string[] = [];
  private waiting: ((r: IteratorResult<string>) => void) | null = null;
  private failure: ((e: Error) => void) | null = null;
  closed = false;

  push(chunk: string): void {
    if (this.waiting) {
      const resolve = this.waiting;
      this.waiting = null;
      this.failure = null;
      resolve({ done: false, value: chunk });
    } else {
      this.queue.push(chunk);
    }
  }

  end(): void {
    this.closed = true;
    this.waiting?.({ done: true, value: undefined });
    this.waiting = null;
  }

  fail(): void {
    this.failure?.(new Error("reset"));
    this.waiting = null;
  }

  iterable(): AsyncIterable<string> {
    return {
      [Symbol.asyncIterator]: () => ({
        next: () => {
          const chunk = this.queue.shift();
          if (chunk !== undefined) {
            return Promise.resolve({ done: false, value: chunk });
          }
          if (this.closed) {
            return Promise.resolve({ done: true, value: undefined });
          }
          return new Promise<IteratorResult<string>>((resolve, reject) => {
            this.waiting = resolve;
            this.failure = reject;
          });
        },
      }),
    };
  }
}

function harness(opts: { failOpens?: number; failWith?: Error } = {}) {
  const pipes: Pipe[] = [];
  const cursors: (number | null)[] = [];
  const log: string[] = [];
  const events: EventEnvelope[] = [];
  const delays: number[] = [];
  let failOpens = opts.failOpens ?? 0;
  const transport: Transport = (last) => {
    cursors.push(last);
    if (failOpens > 0) {
      failOpens -= 1;
      return Promise.reject(opts.failWith ?? new Error("refused"));
    }
    const pipe = new Pipe();
    pipes.push(pipe);
    return Promise.resolve(pipe.iterable());
  };
  const handle: StreamHandle = startStream({
    transport,
    random: () => 0,
    handlers: {
      onOpen: () => log.push("open"),
      onEvent: (e) => {
        events.push(e);
        log.push(label(e));
      },
      onDrop: (delayMs) => {
        log.push("drop");
        delays.push(delayMs);
      },
      onResync: () => log.push("resync"),
    },
  });
  return { pipes, cursors, log, events, delays, handle };
}

describe("startStream", () => {
  it("opens with no cursor, reports open on the first frame and applies events in order", async () => {
    const h = harness();
    await vi.advanceTimersByTimeAsync(0);
    expect(h.cursors).toEqual([null]);
    h.pipes[0]?.push(frame(1) + frame(2));
    await vi.advanceTimersByTimeAsync(0);
    expect(h.log).toEqual(["open", "event 1", "event 2"]);
    h.handle.stop();
  });

  it("reconnects with the last applied seq and applies no event twice", async () => {
    const h = harness();
    await vi.advanceTimersByTimeAsync(0);
    h.pipes[0]?.push(frame(4) + frame(5) + frame(5) + frame(3));
    await vi.advanceTimersByTimeAsync(0);
    h.pipes[0]?.fail();
    await vi.advanceTimersByTimeAsync(0);
    expect(h.log.at(-1)).toBe("drop");
    await vi.advanceTimersByTimeAsync(nextDelay(0, () => 0));
    expect(h.cursors).toEqual([null, 5]);
    h.pipes[1]?.push(frame(5) + frame(6));
    await vi.advanceTimersByTimeAsync(0);
    expect(h.events.map(label)).toEqual(["event 4", "event 5", "event 6"]);
    expect(h.log).toEqual([
      "open",
      "event 4",
      "event 5",
      "drop",
      "open",
      "event 6",
    ]);
    h.handle.stop();
  });

  it("reports each backoff delay with the drop, and retry starts the next attempt at once", async () => {
    const h = harness({ failOpens: 3 });
    await vi.advanceTimersByTimeAsync(0);
    expect(h.delays).toEqual([nextDelay(0, () => 0)]);
    await vi.advanceTimersByTimeAsync(nextDelay(0, () => 0));
    expect(h.delays).toEqual([nextDelay(0, () => 0), nextDelay(1, () => 0)]);
    h.handle.retry();
    await vi.advanceTimersByTimeAsync(0);
    expect(h.cursors).toHaveLength(3);
    expect(h.delays.at(-1)).toBe(nextDelay(2, () => 0));
    h.handle.retry();
    await vi.advanceTimersByTimeAsync(0);
    h.pipes[0]?.push(frame(1));
    await vi.advanceTimersByTimeAsync(0);
    expect(h.log.at(-1)).toBe("event 1");
    h.handle.retry();
    await vi.advanceTimersByTimeAsync(0);
    expect(h.cursors).toHaveLength(4);
    h.handle.stop();
  });

  it("reconnects when the server ends the stream", async () => {
    const h = harness();
    await vi.advanceTimersByTimeAsync(0);
    h.pipes[0]?.push(frame(1));
    h.pipes[0]?.end();
    await vi.advanceTimersByTimeAsync(1000);
    expect(h.cursors).toEqual([null, 1]);
    h.handle.stop();
  });

  it("treats 45 seconds of silence as a dead connection", async () => {
    const h = harness();
    await vi.advanceTimersByTimeAsync(0);
    h.pipes[0]?.push(": keep-alive\n\n");
    await vi.advanceTimersByTimeAsync(DEAD_AFTER_MS - 1);
    expect(h.log).toEqual(["open"]);
    await vi.advanceTimersByTimeAsync(1);
    expect(h.log).toEqual(["open", "drop"]);
    await vi.advanceTimersByTimeAsync(1000);
    expect(h.cursors).toHaveLength(2);
    h.handle.stop();
  });

  it("keeps a connection alive while keep-alive comments arrive", async () => {
    const h = harness();
    await vi.advanceTimersByTimeAsync(0);
    for (let i = 0; i < 6; i += 1) {
      h.pipes[0]?.push(": keep-alive\n\n");
      await vi.advanceTimersByTimeAsync(15_000);
    }
    expect(h.log).toEqual(["open"]);
    h.handle.stop();
  });

  it("backs off after failed opens and resets after a healthy connection", async () => {
    const h = harness({ failOpens: 2 });
    await vi.advanceTimersByTimeAsync(0);
    expect(h.cursors).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(499);
    expect(h.cursors).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(1);
    expect(h.cursors).toHaveLength(2);
    await vi.advanceTimersByTimeAsync(999);
    expect(h.cursors).toHaveLength(2);
    await vi.advanceTimersByTimeAsync(1);
    expect(h.cursors).toHaveLength(3);
    h.pipes[0]?.push(frame(1));
    h.pipes[0]?.end();
    await vi.advanceTimersByTimeAsync(499);
    expect(h.cursors).toHaveLength(3);
    await vi.advanceTimersByTimeAsync(1);
    expect(h.cursors).toHaveLength(4);
    h.handle.stop();
  });

  it("applies a snapshot every time and ignores garbage frames", async () => {
    const h = harness();
    await vi.advanceTimersByTimeAsync(0);
    h.pipes[0]?.push(
      'data: not json\n\ndata: {"seq":1}\n\ndata: {"seq":2,"type":"task.created"}\n\n',
    );
    h.pipes[0]?.push(frame(3) + snap(3) + snap(3));
    await vi.advanceTimersByTimeAsync(0);
    expect(h.events.map((e) => (e.kind === "event" ? e.type : e.kind))).toEqual(
      ["task.created", "snapshot", "snapshot"],
    );
    h.pipes[0]?.end();
    await vi.advanceTimersByTimeAsync(1000);
    expect(h.cursors.at(-1)).toBe(3);
    h.handle.stop();
  });

  it("stops during a connection and during the wait, and opens nothing more", async () => {
    const h = harness();
    await vi.advanceTimersByTimeAsync(0);
    h.pipes[0]?.push(frame(1));
    await vi.advanceTimersByTimeAsync(0);
    h.handle.stop();
    await vi.advanceTimersByTimeAsync(60_000);
    expect(h.cursors).toHaveLength(1);
    expect(h.log).toEqual(["open", "event 1"]);

    const w = harness();
    await vi.advanceTimersByTimeAsync(0);
    w.pipes[0]?.end();
    await vi.advanceTimersByTimeAsync(0);
    w.handle.stop();
    await vi.advanceTimersByTimeAsync(60_000);
    expect(w.cursors).toHaveLength(1);
  });

  it("gives up waiting on a transport that never answers", async () => {
    const cursors: (number | null)[] = [];
    const drops: string[] = [];
    const handle = startStream({
      transport: (last) => {
        cursors.push(last);
        return new Promise<AsyncIterable<string>>(() => undefined);
      },
      random: () => 0,
      handlers: {
        onOpen: () => undefined,
        onEvent: () => undefined,
        onDrop: () => drops.push("drop"),
        onResync: () => undefined,
      },
    });
    await vi.advanceTimersByTimeAsync(DEAD_AFTER_MS);
    expect(drops).toEqual(["drop"]);
    handle.stop();
  });

  it("uses Math.random for jitter by default", async () => {
    const handle = startStream({
      transport: () => Promise.reject(new Error("no")),
      handlers: {
        onOpen: () => undefined,
        onEvent: () => undefined,
        onDrop: () => undefined,
        onResync: () => undefined,
      },
    });
    await vi.advanceTimersByTimeAsync(0);
    handle.stop();
  });

  it("delivers every workspace_changed frame, which has no seq and never moves the cursor", async () => {
    const h = harness();
    await vi.advanceTimersByTimeAsync(0);
    h.pipes[0]?.push(live());
    await vi.advanceTimersByTimeAsync(0);
    h.pipes[0]?.end();
    await vi.advanceTimersByTimeAsync(1000);
    expect(h.cursors).toEqual([null, null]);
    h.pipes[1]?.push(frame(4) + live() + live() + frame(5));
    await vi.advanceTimersByTimeAsync(0);
    expect(h.log).toEqual([
      "open",
      "live",
      "drop",
      "open",
      "event 4",
      "live",
      "live",
      "event 5",
    ]);
    h.pipes[1]?.end();
    await vi.advanceTimersByTimeAsync(1000);
    expect(h.cursors).toEqual([null, null, 5]);
    h.handle.stop();
  });

  it("delivers every agent_presence_changed frame without moving the cursor", async () => {
    const h = harness();
    await vi.advanceTimersByTimeAsync(0);
    h.pipes[0]?.push(frame(4) + presence() + frame(5) + presence());
    await vi.advanceTimersByTimeAsync(0);
    expect(h.log).toEqual([
      "open",
      "event 4",
      "presence frontend idle",
      "event 5",
      "presence frontend idle",
    ]);
    h.pipes[0]?.end();
    await vi.advanceTimersByTimeAsync(1000);
    expect(h.cursors).toEqual([null, 5]);
    h.handle.stop();
  });

  it("asks for a resync when a stored seq is skipped, then applies the event", async () => {
    const h = harness();
    await vi.advanceTimersByTimeAsync(0);
    h.pipes[0]?.push(frame(1) + frame(2) + frame(5) + frame(6));
    await vi.advanceTimersByTimeAsync(0);
    expect(h.log).toEqual([
      "open",
      "event 1",
      "event 2",
      "resync",
      "event 5",
      "event 6",
    ]);
    h.handle.stop();
  });

  it("measures a gap from the cursor a snapshot set", async () => {
    const h = harness();
    await vi.advanceTimersByTimeAsync(0);
    h.pipes[0]?.push(snap(7) + frame(8) + frame(10));
    await vi.advanceTimersByTimeAsync(0);
    expect(h.log).toEqual(["open", "event 7", "event 8", "resync", "event 10"]);
    h.handle.stop();
  });

  it("drops the cursor and asks for a resync when the daemon refuses it", async () => {
    const h = harness({
      failOpens: 1,
      failWith: new ApiError("invalid_last_event_id", "unknown seq"),
    });
    await vi.advanceTimersByTimeAsync(0);
    expect(h.log).toEqual(["resync", "drop"]);
    await vi.advanceTimersByTimeAsync(1000);
    expect(h.cursors).toEqual([null, null]);
    h.handle.stop();
  });

  it("keeps the cursor and asks for no resync when an open fails for any other reason", async () => {
    const h = harness({
      failOpens: 1,
      failWith: new ApiError("project_unavailable", "down"),
    });
    await vi.advanceTimersByTimeAsync(0);
    expect(h.log).toEqual(["drop"]);
    h.handle.stop();
  });

  it("applies nothing from a chunk that settles as the stream stops", async () => {
    const h = harness();
    await vi.advanceTimersByTimeAsync(0);
    h.pipes[0]?.push(frame(1));
    await vi.advanceTimersByTimeAsync(0);
    h.pipes[0]?.push(frame(2));
    h.handle.stop();
    await vi.advanceTimersByTimeAsync(0);
    expect(h.log).toEqual(["open", "event 1"]);
  });
});

describe("fixture transport", () => {
  it("sends a snapshot, then events after the cursor, then keep-alives, and stops on abort", async () => {
    const controller = new AbortController();
    const transport = fixtureEventTransport("demo", 10, 100);
    const chunks: string[] = [];
    const reading = (async () => {
      for await (const chunk of await transport(1, controller.signal)) {
        chunks.push(chunk);
      }
    })();
    await vi.advanceTimersByTimeAsync(20);
    expect(chunks).toHaveLength(3);
    expect(chunks[0]).toContain('"kind":"snapshot"');
    expect(chunks[1]).toContain('"kind":"workspace_changed"');
    expect(chunks[1]).not.toContain("id:");
    expect(chunks[2]).toContain('"seq":2');
    expect(chunks[2]).toContain("id: 2");
    await vi.advanceTimersByTimeAsync(100);
    expect(chunks[3]).toBe(": keep-alive\n\n");
    controller.abort();
    await vi.advanceTimersByTimeAsync(0);
    await reading;
  });

  it("returns early when aborted before or between steps", async () => {
    const aborted = new AbortController();
    aborted.abort();
    const chunks: string[] = [];
    for await (const chunk of await fixtureEventTransport(
      "demo",
      10,
      100,
    )(null, aborted.signal)) {
      chunks.push(chunk);
    }
    expect(chunks).toHaveLength(1);
  });
});
