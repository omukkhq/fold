import { QueryClient } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { keys } from "../api/keys";
import { connectProject } from "./connect";
import { createLiveStatusStore } from "./status";
import type { Transport } from "./stream";

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
          yield `data: ${JSON.stringify({ seq: 1, type: "snapshot", workspaces: [{ workspace: 1, state: { a: 1 } }] })}\n\n`;
          yield `data: ${JSON.stringify({ seq: 2, type: "workspace.changed", workspace: 1, data: { state: { a: 2 } } })}\n\n`;
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
    });
    expect(client.getQueryData(keys.workspaceLive("demo", 1))).toEqual({
      a: 2,
    });
    release();
    await vi.advanceTimersByTimeAsync(0);
    expect(status.getSnapshot().state).toBe("reconnecting");
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
            yield `data: {"seq":1,"type":"x.y"}\n\n`;
          })(),
        ),
      status,
    );
    await vi.advanceTimersByTimeAsync(0);
    expect(status.getSnapshot().lastEventAt).not.toBeNull();
    handle.stop();
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
            yield `data: {"seq":1,"type":"task.created"}\n\ndata: {"seq":3,"type":"task.created"}\n\n`;
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
});
