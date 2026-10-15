// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  RouterProvider,
  createMemoryHistory,
  createRootRoute,
  createRoute,
  createRouter,
} from "@tanstack/react-router";
import {
  act,
  cleanup,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { agents } from "../../../fixtures/agentBar";
import { board } from "../../../fixtures/board";
import { fixtureEventTransport } from "../../../fixtures/events";
import type { Board } from "@/api/board";
import { keys } from "@/api/keys";
import { formatAsOf } from "@/format/time";
import { connectProject } from "@/live/connect";
import { LiveRetryContext } from "@/live/retry";
import { liveStatusStore } from "@/live/status";
import { BoardScreen } from "./BoardScreen";
import { boardSearchSchema, cleanSearch } from "./search";

beforeAll(() => {
  globalThis.ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
  Element.prototype.hasPointerCapture = () => false;
  Element.prototype.setPointerCapture = () => {};
  Element.prototype.releasePointerCapture = () => {};
  Element.prototype.scrollIntoView = () => {};
});

afterEach(() => {
  cleanup();
  liveStatusStore.reset();
});

function setup(
  data: Board | null = board("demo"),
  path = "/p/demo/board",
  retry: () => void = () => undefined,
): { client: QueryClient; router: ReturnType<typeof makeRouter> } {
  const client = new QueryClient({
    defaultOptions: { queries: { staleTime: Infinity, retry: false } },
  });
  if (data) {
    client.setQueryData(keys.board("demo"), data);
    client.setQueryData(keys.agentBar("demo"), agents("demo"));
  }
  const router = makeRouter(path);
  render(
    <QueryClientProvider client={client}>
      <LiveRetryContext value={retry}>
        <RouterProvider router={router} />
      </LiveRetryContext>
    </QueryClientProvider>,
  );
  return { client, router };
}

function makeRouter(path: string) {
  const root = createRootRoute();
  const boardRoute = createRoute({
    getParentRoute: () => root,
    path: "/p/$slug/board",
    validateSearch: boardSearchSchema,
    component: function BoardRoute() {
      const { slug } = boardRoute.useParams();
      const search = boardRoute.useSearch();
      const navigate = boardRoute.useNavigate();
      return (
        <BoardScreen
          slug={slug}
          search={search}
          onSearch={(next) => {
            void navigate({ search: cleanSearch(next), replace: true });
          }}
        />
      );
    },
  });
  const taskRoute = createRoute({
    getParentRoute: () => root,
    path: "/p/$slug/t/$task",
    component: () => <p>Task dialog</p>,
  });
  return createRouter({
    routeTree: root.addChildren([boardRoute, taskRoute]),
    history: createMemoryHistory({ initialEntries: [path] }),
  });
}

const column = (name: string) => screen.getByRole("region", { name });
const tasksIn = (name: string) =>
  within(column(name))
    .queryAllByRole("link")
    .map((link) => link.closest("[data-task]")?.getAttribute("data-task"));

describe("BoardScreen", () => {
  it("shows the five columns from Backlog to Done with their cards", async () => {
    setup();
    await screen.findByRole("region", { name: "Backlog" });
    const regions = screen
      .getAllByRole("region")
      .map((r) => r.getAttribute("aria-label"));
    expect(regions).toEqual([
      "Backlog",
      "Todo",
      "In Progress",
      "Review",
      "Done",
    ]);
    expect(tasksIn("Todo")).toEqual(["T-1", "T-8", "T-9"]);
    expect(screen.getByText("16 tasks · 3 ready")).toBeTruthy();
  });

  it("shows every card mark from the fixture", async () => {
    setup();
    await screen.findByRole("region", { name: "Backlog" });
    expect(screen.getByText("Blocked by T-3")).toBeTruthy();
    expect(screen.getByText("Question for you")).toBeTruthy();
    expect(screen.getByText("Your review")).toBeTruthy();
    expect(screen.getByText("Stalled · needs you")).toBeTruthy();
    expect(screen.getByText("Stale claim · needs you")).toBeTruthy();
    expect(screen.getByText("P-3 · changes requested")).toBeTruthy();
    expect(screen.getByText("P-6 · conflict")).toBeTruthy();
    expect(screen.getAllByText("Human review · tanim")).toHaveLength(2);
    expect(screen.getByText("needs T-2, T-4")).toBeTruthy();
    expect(screen.getByText("Editing src/routes/login.tsx")).toBeTruthy();
    expect(screen.getByText(/^No edits for \d+ min · blocked$/)).toBeTruthy();
    expect(
      screen.getByLabelText(
        "Needs you: Stalled: its worker made no edits for a while",
      ),
    ).toBeTruthy();
  });

  it("has nothing to drag and no control that moves a card", async () => {
    setup();
    await screen.findByRole("region", { name: "Backlog" });
    expect(document.body.querySelectorAll('[draggable="true"]')).toHaveLength(
      0,
    );
    for (const link of within(column("In Progress")).getAllByRole("link")) {
      expect(link.getAttribute("draggable")).toBe("false");
    }
    for (const name of ["Backlog", "Todo", "In Progress", "Review", "Done"]) {
      expect(within(column(name)).queryAllByRole("button")).toHaveLength(0);
      expect(within(column(name)).queryAllByRole("menu")).toHaveLength(0);
    }
  });

  it("opens a card from the keyboard", async () => {
    const { router } = setup();
    const link = await screen.findByRole("link", {
      name: "Add the session cookie and sign-out",
    });
    link.focus();
    await userEvent.keyboard("{Enter}");
    expect(await screen.findByText("Task dialog")).toBeTruthy();
    expect(router.state.location.pathname).toBe("/p/demo/t/T-1");
  });

  it("moves a card when the fixture stream sends a status event, with no reload", async () => {
    const { client } = setup();
    await screen.findByRole("region", { name: "Backlog" });
    expect(tasksIn("Todo")).toContain("T-1");
    const stream = connectProject(
      client,
      "demo",
      fixtureEventTransport("demo", 300, 60_000),
      liveStatusStore,
    );
    try {
      await waitFor(
        () => {
          expect(tasksIn("In Progress")[0]).toBe("T-1");
        },
        { timeout: 2000 },
      );
      expect(tasksIn("Todo")).not.toContain("T-1");
      await waitFor(
        () => {
          expect(tasksIn("Review")).toEqual(["T-1", "T-3"]);
        },
        { timeout: 2000 },
      );
      expect(tasksIn("In Progress")).not.toContain("T-1");
      // The status was patched into the cache; an unknown status would have invalidated and refetched the board.
      expect(client.getQueryState(keys.board("demo"))?.isInvalidated).toBe(
        false,
      );
    } finally {
      stream.stop();
    }
  });

  it("filters by blocked and Needs you, and the URL carries the filter", async () => {
    const { router } = setup();
    await screen.findByRole("region", { name: "Backlog" });
    await userEvent.click(screen.getByRole("button", { name: "Blocked only" }));
    expect(router.state.location.search).toEqual({ blocked: true });
    expect(router.state.location.href).toContain("blocked=true");
    expect(tasksIn("In Progress")).toEqual(["T-10"]);
    await userEvent.click(screen.getByRole("button", { name: "Blocked only" }));
    expect(router.state.location.search).toEqual({});

    await userEvent.click(screen.getByRole("button", { name: /^Needs you/ }));
    expect(router.state.location.search).toEqual({ needs: true });
    expect(tasksIn("Backlog")).toEqual(["T-6"]);
    expect(tasksIn("Review")).toEqual(["T-3"]);
  });

  it("filters by an agent from the Agent menu, and the toolbar draws no agent pills", async () => {
    const { router } = setup();
    await screen.findByRole("region", { name: "Backlog" });
    expect(screen.queryByRole("group", { name: "Agents" })).toBeNull();
    expect(
      screen.queryByRole("button", { name: "Show only backend's tasks" }),
    ).toBeNull();

    const trigger = screen.getByRole("button", { name: "Agent" });
    trigger.focus();
    await userEvent.keyboard("{Enter}");
    await userEvent.click(screen.getByRole("menuitem", { name: "frontend" }));
    expect(router.state.location.search).toEqual({ agent: "frontend" });
    expect(tasksIn("Review")).toEqual([]);
    screen.getByRole("button", { name: "Agent: frontend" }).focus();
    await userEvent.keyboard("{Enter}");
    await userEvent.click(screen.getByRole("menuitem", { name: "Any agent" }));
    expect(router.state.location.search).toEqual({});
  });

  it("filters by a label from the Labels menu", async () => {
    const { router } = setup();
    await screen.findByRole("region", { name: "Backlog" });
    screen.getByRole("button", { name: "Labels" }).focus();
    await userEvent.keyboard("{Enter}");
    await userEvent.click(screen.getByRole("menuitem", { name: "auth" }));
    expect(router.state.location.search).toEqual({ label: "auth" });
    expect(tasksIn("Todo")).toEqual(["T-1", "T-9"]);
    expect(tasksIn("Backlog")).toEqual([]);
  });

  it("reads the filters from a shared link and searches", async () => {
    const { router } = setup(
      board("demo"),
      "/p/demo/board?label=ui&agent=frontend",
    );
    await screen.findByRole("region", { name: "Backlog" });
    // T-8 carries both labels but nobody is responsible for it.
    expect(tasksIn("Todo")).toEqual([]);
    expect(tasksIn("In Progress")).toEqual(["T-2", "T-11"]);
    await userEvent.type(
      screen.getByRole("searchbox", { name: "Search tasks" }),
      "theme",
    );
    expect(router.state.location.search).toMatchObject({ q: "theme" });
    expect(tasksIn("In Progress")).toEqual(["T-11"]);
  });

  it("shows the empty state with the next step for an empty project", async () => {
    setup({ columns: [] });
    const panel = await screen.findByRole("region", { name: "No tasks yet" });
    expect(within(column("Todo")).getByText("No tasks yet")).toBeTruthy();
    expect(panel.textContent).toContain("claude --name frontend");
    expect(screen.getByText("0 tasks")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Needs you · 0" })).toBeTruthy();
  });

  it("shows skeleton columns while the board loads", async () => {
    setup(null);
    await screen.findByText("Loading the board…");
    expect(screen.getByRole("button", { name: /^Needs you/ }).textContent).toBe(
      "Needs you–",
    );
    expect(screen.getAllByRole("status", { name: "Loading" })).toHaveLength(15);
    expect(within(column("Done")).getByText("–")).toBeTruthy();
  });

  it("shows the error banner and loads the board again on Retry", async () => {
    const client = new QueryClient({
      defaultOptions: {
        queries: { staleTime: Infinity, retry: false, retryOnMount: false },
      },
    });
    client
      .getQueryCache()
      .build(client, { queryKey: keys.board("demo") })
      .setState({
        status: "error",
        error: new Error("down"),
        fetchStatus: "idle",
        errorUpdatedAt: 1,
      });
    client.setQueryData(keys.agentBar("demo"), []);
    render(
      <QueryClientProvider client={client}>
        <RouterProvider router={makeRouter("/p/demo/board")} />
      </QueryClientProvider>,
    );
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("The board could not load.");
    await userEvent.click(within(alert).getByRole("button", { name: "Retry" }));
    expect(
      await screen.findByRole(
        "link",
        { name: "Login flow" },
        { timeout: 2000 },
      ),
    ).toBeTruthy();
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("shows the reconnecting banner with the countdown, Retry and the time of the board shown", async () => {
    const retry = vi.fn();
    setup(board("demo"), "/p/demo/board", retry);
    await screen.findByRole("region", { name: "Backlog" });
    const asOf = Date.now();
    act(() => {
      liveStatusStore.set({
        state: "reconnecting",
        lastEventAt: asOf,
        retryAt: asOf + 4_500,
      });
    });
    const banner = screen.getByText(/Live updates paused/);
    expect(banner.textContent).toMatch(/Reconnecting in [45] s\./);
    expect(banner.textContent).toContain(
      `Showing the board as of ${formatAsOf(asOf, asOf)};`,
    );
    expect(column("Backlog").closest(".opacity-55")).not.toBeNull();
    await userEvent.click(screen.getByRole("button", { name: "Retry now" }));
    expect(retry).toHaveBeenCalledTimes(1);
    act(() => {
      liveStatusStore.set({ state: "live" });
    });
    expect(screen.queryByText(/Live updates paused/)).toBeNull();
    expect(column("Backlog").closest(".opacity-55")).toBeNull();
  });
});
