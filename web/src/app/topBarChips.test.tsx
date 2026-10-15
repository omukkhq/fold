// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  Outlet,
  RouterProvider,
  createMemoryHistory,
  createRootRoute,
  createRoute,
  createRouter,
} from "@tanstack/react-router";
import { act, cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ReactNode } from "react";
import { afterEach, describe, expect, it } from "vitest";
import type { ProjectAgent } from "../api/agentBar";
import { keys } from "../api/keys";
import { agent, claim, worker } from "../test/agents";
import { liveStatusStore } from "../live/status";
import { AgentChips } from "./AgentChips";
import { StreamStatus } from "./StreamStatus";

afterEach(() => {
  cleanup();
  liveStatusStore.reset();
});

/** A client that never refetches on its own, so the chips can only change through the cache. */
function cacheOnly(): QueryClient {
  return new QueryClient({
    defaultOptions: { queries: { staleTime: Infinity, retry: false } },
  });
}

function wrap(client: QueryClient) {
  return ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}>{children}</QueryClientProvider>
  );
}

/** The chips under a root route, with a board route that shows the agent filter it received. */
async function renderChips(client: QueryClient) {
  const root = createRootRoute({
    component: () => (
      <>
        <AgentChips slug="demo" />
        <Outlet />
      </>
    ),
  });
  const index = createRoute({
    getParentRoute: () => root,
    path: "/",
    component: () => null,
  });
  const board = createRoute({
    getParentRoute: () => root,
    path: "/p/$slug/board",
    component: function Board() {
      const search: { agent?: string } = board.useSearch();
      return <p>Board for {search.agent}</p>;
    },
  });
  const router = createRouter({
    routeTree: root.addChildren([index, board]),
    history: createMemoryHistory({ initialEntries: ["/"] }),
  });
  render(<RouterProvider router={router} />, { wrapper: wrap(client) });
  await screen.findByRole("list", { name: "Agents" });
  return router;
}

/** An agent whose worker holds `working` implementation claims, `stalled` of their tasks stalled. */
function chip(
  name: string,
  presence: ProjectAgent["presence"],
  working: number,
  stalled = 0,
): ProjectAgent {
  return {
    ...agent(name, { presence }),
    workers: [
      worker(name, {
        claims: Array.from({ length: working }, (_, i) =>
          claim({ task: `T-${String(i + 1)}` }),
        ),
      }),
    ],
    attention: Array.from({ length: stalled }, (_, i) => ({
      kind: "stalled" as const,
      task: `T-${String(i + 1)}`,
    })),
  };
}

const frontend = chip("frontend", "active", 2);

describe("AgentChips", () => {
  it("draws nothing for a project with no agents", () => {
    const client = cacheOnly();
    client.setQueryData(keys.agentBar("demo"), []);
    const { container } = render(<AgentChips slug="demo" />, {
      wrapper: wrap(client),
    });
    expect(container.textContent).toBe("");
  });

  it("draws a chip per agent and follows the cache without a refetch", async () => {
    liveStatusStore.set({ state: "live" });
    const client = cacheOnly();
    client.setQueryData(keys.agentBar("demo"), [frontend]);
    await renderChips(client);
    expect(screen.getByText("2 working")).toBeTruthy();
    expect(screen.getByRole("img", { name: "Active" })).toBeTruthy();
    act(() => {
      client.setQueryData(keys.agentBar("demo"), [
        chip("frontend", "idle", 1, 1),
        chip("review", "offline", 0),
      ]);
    });
    expect(await screen.findByText("1 working · 1 stalled")).toBeTruthy();
    expect(screen.getByText("not connected")).toBeTruthy();
    expect(screen.getAllByRole("listitem")).toHaveLength(2);
  });

  it("shows when the counts were true while the stream reconnects", async () => {
    const synced = new Date(2026, 9, 15, 14, 12, 31).getTime();
    liveStatusStore.set({ state: "live", lastEventAt: synced });
    const client = cacheOnly();
    client.setQueryData(keys.agentBar("demo"), [frontend]);
    await renderChips(client);
    act(() => {
      liveStatusStore.set({ state: "reconnecting" });
    });
    expect(screen.getByRole("listitem").textContent).toContain("as of ");
    expect(screen.queryByText("2 working")).toBeNull();
  });

  it("opens the board filtered to the agent of the chip", async () => {
    liveStatusStore.set({ state: "live" });
    const client = cacheOnly();
    client.setQueryData(keys.agentBar("demo"), [frontend]);
    const router = await renderChips(client);
    const chip = screen.getByRole("link", { name: /frontend/ });
    expect(chip.getAttribute("href")).toBe("/p/demo/board?agent=frontend");
    await userEvent.click(chip);
    expect(await screen.findByText("Board for frontend")).toBeTruthy();
    expect(router.state.location.pathname).toBe("/p/demo/board");
  });
});

describe("StreamStatus", () => {
  it("follows the connection state and announces only the state", () => {
    render(<StreamStatus />);
    const state = screen.getByRole("status");
    expect(state.textContent).toBe("Connecting…");
    act(() => {
      liveStatusStore.set({ state: "live", lastEventAt: Date.now() });
    });
    expect(screen.getByRole("status").textContent).toBe("Live");
    expect(screen.getByRole("status").parentElement?.textContent).toMatch(
      /^Live · synced \d+ s ago$/,
    );
    act(() => {
      liveStatusStore.set({ state: "reconnecting" });
    });
    expect(screen.getByRole("status").parentElement?.textContent).toBe(
      "Reconnecting…",
    );
  });
});
