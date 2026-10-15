import { QueryClient } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FIXTURE_DELAY_MS } from "./fixture";
import { agentBarQuery, getProjectAgents } from "./agentBar";
import { keys } from "./keys";

beforeEach(() => {
  vi.useFakeTimers();
});
afterEach(() => {
  vi.useRealTimers();
});

describe("agent bar", () => {
  it("answers the agents of the development project and none for another", async () => {
    const demo = getProjectAgents("demo");
    const other = getProjectAgents("other");
    await vi.advanceTimersByTimeAsync(FIXTURE_DELAY_MS);
    expect((await demo).map((a) => a.name)).toEqual([
      "frontend",
      "backend",
      "review",
    ]);
    expect(await other).toEqual([]);
  });

  it("sits under the agents key, so agent events refresh it", async () => {
    const query = agentBarQuery("demo");
    expect(query.queryKey.slice(0, 2)).toEqual(keys.agents("demo"));
    const client = new QueryClient();
    const fetched = client.query(query);
    await vi.advanceTimersByTimeAsync(FIXTURE_DELAY_MS);
    expect(await fetched).toHaveLength(3);
  });
});
