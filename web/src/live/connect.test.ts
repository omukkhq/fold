import { QueryClient } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ApiError } from "../api/errors";
import { keys } from "../api/keys";
import { connectProject } from "./connect";
import { createLiveStatusStore } from "./status";
import type { Transport } from "./stream";

function workspace(summary: string) {
  return {
    workspace: "ws-1",
    task: "T-1",
    worker: "frontend/ui",
    base: "r1",
    files: [],
    summary,
    last_activity_at: 1_760_000_000_000,
    checkpoints: [],
    proposal: null,
    stalled: false,
  };
}

function stored(seq: number, type: string): string {
  return `data: ${JSON.stringify({
    kind: "event",
    seq,
    type,
    actor: "frontend/ui",
    task: null,
    proposal: null,
    workspace: null,
    revision: null,
    data: {},
    created_at: 1_760_000_000_000,
  })}\n\n`;
}

beforeEach(() => {
  vi.useFakeTimers();
});
afterEach(() => {
  vi.useRealTimers();
});

describe("connectProject", () => {
  it("applies events to the cache and tracks the connection state", async () => {
    const client = new QueryClient();
    const status = createLiveStatusStore();
    status.set({ state: "reconnecting" });
    let release: () => void = () => undefined;
    const transport: Transport = () =>
      Promise.resolve(
        (async function* () {
          await Promise.resolve();
          yield `data: ${JSON.stringify({ kind: "snapshot", seq: 1, workspaces: [workspace("one")] })}\n\n`;
          yield `data: ${JSON.stringify({ kind: "workspace_changed", ...workspace("two") })}\n\n`;
          await new Promise<void>((resolve) => {
            release = resolve;
          });
        })(),
      );
    expect(status.getSnapshot().state).toBe("reconnecting");
    const handle = connectProject(client, "demo", transport, status, () => 42);
    expect(status.getSnapshot().state).toBe("connecting");
    await vi.advanceTimersByTimeAsync(0);
    expect(status.getSnapshot()).toEqual({
      state: "live",
      lastEventAt: 42,
      stateAsOf: 42,
      retryAt: null,
    });
    expect(client.getQueryData(keys.workspaceLive("demo", "ws-1"))).toEqual(
      workspace("two"),
    );
    release();
    await vi.advanceTimersByTimeAsync(0);
    expect(status.getSnapshot().state).toBe("reconnecting");
    // The first delay after a healthy connection is between half a second and a second.
    const retryAt = status.getSnapshot().retryAt ?? 0;
    expect(retryAt).toBeGreaterThanOrEqual(42 + 500);
    expect(retryAt).toBeLessThanOrEqual(42 + 1000);
    handle.retry();
    await vi.advanceTimersByTimeAsync(0);
    expect(status.getSnapshot()).toMatchObject({
      state: "live",
      retryAt: null,
    });
    handle.stop();
  });

  it("uses the system clock by default", async () => {
    const status = createLiveStatusStore();
    const handle = connectProject(
      new QueryClient(),
      "demo",
      () =>
        Promise.resolve(
          (async function* () {
            await Promise.resolve();
            yield stored(1, "x.y");
          })(),
        ),
      status,
    );
    await vi.advanceTimersByTimeAsync(0);
    expect(status.getSnapshot().lastEventAt).not.toBeNull();
    handle.stop();
  });

  it("keeps no clock tick for a worker step", async () => {
    const status = createLiveStatusStore();
    const handle = connectProject(
      new QueryClient(),
      "demo",
      () =>
        Promise.resolve(
          (async function* () {
            await Promise.resolve();
            yield `data: ${JSON.stringify({ kind: "worker_output", agent: "web-worker", step: "tool_use", tool: "Edit", text_length: null })}\n\n`;
            await new Promise<void>(() => undefined);
          })(),
        ),
      status,
      () => 42,
    );
    await vi.advanceTimersByTimeAsync(0);
    expect(status.getSnapshot()).toMatchObject({
      state: "live",
      lastEventAt: null,
    });
    handle.stop();
  });

  it("resumes after the last stored event when the project opens again", async () => {
    const client = new QueryClient();
    const cursors: (number | null)[] = [];
    const transport: Transport = (lastSeq) => {
      cursors.push(lastSeq);
      return Promise.resolve(
        (async function* () {
          await Promise.resolve();
          yield stored(5, "task.created") + stored(6, "task.created");
          await new Promise<void>(() => undefined);
        })(),
      );
    };
    for (const slug of ["demo", "other", "demo"]) {
      const handle = connectProject(
        client,
        slug,
        transport,
        createLiveStatusStore(),
      );
      await vi.advanceTimersByTimeAsync(0);
      handle.stop();
    }
    expect(cursors).toEqual([null, null, 6]);
  });

  it("refetches the project when a stored event is skipped", async () => {
    const client = new QueryClient();
    const prefixes: unknown[] = [];
    const original = client.invalidateQueries.bind(client);
    client.invalidateQueries = (filters, options) => {
      prefixes.push(filters?.queryKey);
      return original(filters, options);
    };
    const handle = connectProject(
      client,
      "demo",
      () =>
        Promise.resolve(
          (async function* () {
            await Promise.resolve();
            yield stored(1, "task.created") + stored(3, "task.created");
            await new Promise<void>(() => undefined);
          })(),
        ),
      createLiveStatusStore(),
    );
    await vi.advanceTimersByTimeAsync(0);
    expect(
      prefixes.filter((key) => JSON.stringify(key) === '["demo"]'),
    ).toHaveLength(1);
    handle.stop();
  });

  it("hands auth_required to the sign-in callback once", async () => {
    const onAuthRequired = vi.fn();
    const handle = connectProject(
      new QueryClient(),
      "demo",
      () => Promise.reject(new ApiError("auth_required", "signed out")),
      createLiveStatusStore(),
      Date.now,
      onAuthRequired,
    );
    await vi.advanceTimersByTimeAsync(60_000);
    expect(onAuthRequired).toHaveBeenCalledOnce();
    handle.stop();
  });

  it("ignores auth_required without a sign-in callback", async () => {
    const status = createLiveStatusStore();
    const handle = connectProject(
      new QueryClient(),
      "demo",
      () => Promise.reject(new ApiError("auth_required", "signed out")),
      status,
    );
    await vi.advanceTimersByTimeAsync(60_000);
    expect(status.getSnapshot().state).not.toBe("live");
    handle.stop();
  });
});
