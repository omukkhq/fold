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
import { agent } from "../../test/agents";
import type { Board } from "@/api/board";
import { keys } from "@/api/keys";
import { formatAsOf } from "@/format/time";
import { applyEvent } from "@/live/apply";
import { isEventEnvelope } from "@/live/envelope";
import { LiveRetryContext } from "@/live/retry";
import { liveStatusStore } from "@/live/status";
import { daemon, ok } from "@/test/daemon";
import { boardCard, boardView } from "@/test/payloads";
import { BoardScreen } from "./BoardScreen";
import { boardSearchSchema, cleanSearch } from "./search";

vi.mock("@/api/client", async (importOriginal) => {
  const real = await importOriginal<typeof import("@/api/client")>();
  const { daemon: fake } = await import("@/test/daemon");
  return { ...real, api: fake.api };
});

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
  daemon.reset();
  cleanup();
  liveStatusStore.reset();
});

const ui = { name: "ui", color: "sage" };
const auth = { name: "auth", color: "slate" };

/** A board as the daemon sends it: the Done column holds three of five completed tasks. */
function demoBoard(): Board {
  return boardView(
    [
      boardCard("T-1", {
        title: "Add the session cookie and sign-out",
        labels: [auth],
        worker: "frontend/ui",
        workspace: "ws-1",
      }),
      boardCard("T-2", {
        title: "Build the login form",
        status: "in_progress",
        labels: [ui],
        worker: "frontend/ui",
        messages: 2,
        checklist: {
          steps_done: 1,
          steps_total: 3,
          linked_done: 0,
          linked_total: 1,
        },
      }),
      boardCard("T-3", {
        title: "Review the gate",
        status: "review",
        worker: "backend",
        attention: [{ kind: "question" }],
      }),
      boardCard("T-4", {
        title: "Needs two others",
        status: "backlog",
        needs: ["T-2", "T-5"],
        blocked_by: ["T-2"],
      }),
      boardCard("T-5", {
        title: "Waits for the gate",
        status: "backlog",
        blocked: true,
        needs: ["T-3"],
        blocked_by: ["T-3"],
      }),
      boardCard("T-6", {
        title: "Needs a person",
        labels: [auth, ui],
        human_review: {
          reviewer: "tanim",
          reason: "Auth work",
          set_by: "planner",
        },
        attention: [{ kind: "human_review" }],
      }),
      boardCard("T-7", {
        title: "Quiet worker",
        status: "in_progress",
        worker: "backend/db",
        attention: [{ kind: "stalled" }, { kind: "stale_claim" }],
      }),
      boardCard("T-8", { status: "done", title: "Older", completed_at: 10 }),
      boardCard("T-9", { status: "done", title: "Newest", completed_at: 30 }),
      boardCard("T-10", { status: "done", title: "Middle", completed_at: 20 }),
    ],
    { done: 5 },
  );
}

function setup(
  data: Board | null = demoBoard(),
  path = "/p/demo/board",
  retry: () => void = () => undefined,
): { client: QueryClient; router: ReturnType<typeof makeRouter> } {
  const client = new QueryClient({
    defaultOptions: { queries: { staleTime: Infinity, retry: false } },
  });
  if (data) {
    client.setQueryData(keys.board("demo"), data);
    client.setQueryData(keys.agentBar("demo"), [
      agent("frontend"),
      agent("backend"),
      agent("review", { presence: "idle" }),
    ]);
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
    .map((link) => link.closest("[data-task]")?.getAttribute("data-task"))
    .filter((task) => task !== undefined);

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
    expect(tasksIn("Todo")).toEqual(["T-1", "T-6"]);
    expect(screen.getByText("12 tasks · 2 ready")).toBeTruthy();
  });

  it("lists the Done column newest first with its total, its limit hint and the link to the Repository", async () => {
    setup();
    await screen.findByRole("region", { name: "Done" });
    expect(tasksIn("Done")).toEqual(["T-9", "T-10", "T-8"]);
    expect(
      within(column("Done")).getByText("Integrated into main · latest 3"),
    ).toBeTruthy();
    const more = within(column("Done")).getByRole("link", {
      name: "2 more in Repository",
    });
    expect(more.getAttribute("href")).toBe("/p/demo/repository");
    expect(within(column("Done")).getByText("5")).toBeTruthy();
    expect(
      within(column("Todo")).queryByRole("link", {
        name: /more in Repository/,
      }),
    ).toBeNull();
  });

  it("draws the proposal pill and the activity line once the payload carries them", async () => {
    const data = demoBoard();
    const todo = data.columns.find((c) => c.status === "todo");
    const first = todo?.cards[0];
    if (first) {
      first.proposal = {
        id: "P-3",
        state: "changes_requested",
        version: 1,
        checks_failed: false,
        revision: null,
      };
      first.activity = { path: "src/routes/login.tsx", at: Date.now() };
    }
    setup(data);
    await screen.findByRole("region", { name: "Backlog" });
    expect(screen.getByText("P-3 · changes requested")).toBeTruthy();
    expect(screen.getByText("Editing src/routes/login.tsx")).toBeTruthy();
  });

  it("draws no proposal pill while the payload has none", async () => {
    setup();
    await screen.findByRole("region", { name: "Backlog" });
    expect(screen.queryByText(/^P-\d+ ·/)).toBeNull();
  });

  it("shows the message count on a card", async () => {
    setup();
    await screen.findByRole("region", { name: "Backlog" });
    const card = column("In Progress").querySelector('[data-task="T-2"]');
    expect(card?.textContent).toContain("2messages");
  });

  it("renders every attention kind of the payload and nothing else", async () => {
    setup();
    await screen.findByRole("region", { name: "Backlog" });
    expect(screen.getByText("Question for you")).toBeTruthy();
    expect(screen.getByText("Your review")).toBeTruthy();
    expect(screen.getByText("Stalled · needs you")).toBeTruthy();
    expect(screen.getByText("Human review").textContent).toContain("tanim");
    expect(screen.getByText("Blocked by T-3")).toBeTruthy();
    expect(screen.getByText("needs T-2")).toBeTruthy();
    expect(
      screen.getByLabelText(
        "Needs you: Stalled: its worker made no edits for a while · Stale claim: the worker's lease ran out",
      ),
    ).toBeTruthy();
    expect(screen.getAllByLabelText(/^Needs you:/)).toHaveLength(3);
    // A card without an item has no mark, however quiet its worker is.
    expect(
      column("Todo").querySelector('[data-task="T-1"]')?.textContent,
    ).not.toContain("needs you");
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
      // Mark answered closes a question for you; it moves no card.
      const buttons = within(column(name))
        .queryAllByRole("button")
        .map((button) => button.textContent);
      expect(buttons.filter((text) => text !== "Mark answered")).toEqual([]);
      expect(within(column(name)).queryAllByRole("menu")).toHaveLength(0);
    }
  });

  it("marks the asks of a card's task answered, pending until the daemon answers", async () => {
    setup();
    daemon.latency = 30;
    daemon.on(
      "POST /api/v1/projects/demo/asks/answered",
      ok({ answered: ["m-4"] }),
    );
    daemon.on("GET /api/v1/projects/demo/board", () => ok(demoBoard()));
    const review = await screen.findByRole("region", { name: "Review" });
    const buttons = within(review).getAllByRole("button", {
      name: "Mark answered",
    });
    expect(buttons).toHaveLength(1);
    await userEvent.click(buttons[0] as HTMLElement);
    expect(within(review).getByText("Marking…")).toBeTruthy();
    await waitFor(() => {
      expect(within(review).queryByText("Marking…")).toBeNull();
    });
    expect(
      daemon.made("POST /api/v1/projects/demo/asks/answered")[0]?.body,
    ).toEqual({ task: "T-3" });
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

  it("moves a card when the recorded status event arrives, with no reload", async () => {
    const { client } = setup();
    await screen.findByRole("region", { name: "Backlog" });
    expect(tasksIn("Todo")).toContain("T-1");
    const frame = {
      kind: "event",
      seq: 9,
      type: "task.status_changed",
      actor: "fold",
      task: "T-1",
      proposal: null,
      workspace: null,
      revision: null,
      data: { from: "todo", to: "in_progress" },
      created_at: 1,
    };
    if (!isEventEnvelope(frame)) throw new Error("not a frame");
    act(() => {
      applyEvent(client, "demo", frame);
    });
    await waitFor(() => {
      expect(tasksIn("In Progress")).toContain("T-1");
    });
    expect(tasksIn("Todo")).not.toContain("T-1");
    // The status was patched into the cache; an unknown status would have invalidated and refetched the board.
    expect(client.getQueryState(keys.board("demo"))?.isInvalidated).toBe(false);
  });

  it("filters by blocked and Attention, and the URL carries the filter", async () => {
    const { router } = setup();
    await screen.findByRole("region", { name: "Backlog" });
    await userEvent.click(screen.getByRole("button", { name: "Blocked only" }));
    expect(router.state.location.search).toEqual({ blocked: true });
    expect(router.state.location.href).toContain("blocked=true");
    expect(tasksIn("Backlog")).toEqual(["T-5"]);
    expect(tasksIn("In Progress")).toEqual([]);
    await userEvent.click(screen.getByRole("button", { name: "Blocked only" }));
    expect(router.state.location.search).toEqual({});

    await userEvent.click(screen.getByRole("button", { name: /^Attention/ }));
    expect(router.state.location.search).toEqual({ needs: true });
    expect(tasksIn("Todo")).toEqual(["T-6"]);
    expect(tasksIn("Review")).toEqual(["T-3"]);
    expect(tasksIn("In Progress")).toEqual(["T-7"]);
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
    expect(tasksIn("In Progress")).toEqual(["T-2"]);
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
    expect(tasksIn("Todo")).toEqual(["T-1", "T-6"]);
    expect(tasksIn("Backlog")).toEqual([]);
  });

  it("reads the filters from a shared link and searches", async () => {
    const { router } = setup(
      demoBoard(),
      "/p/demo/board?label=ui&agent=frontend",
    );
    await screen.findByRole("region", { name: "Backlog" });
    // T-6 carries the label but nobody is responsible for it.
    expect(tasksIn("Todo")).toEqual([]);
    expect(tasksIn("In Progress")).toEqual(["T-2"]);
    await userEvent.type(
      screen.getByRole("searchbox", { name: "Search tasks" }),
      "nothing",
    );
    expect(router.state.location.search).toMatchObject({ q: "nothing" });
    expect(tasksIn("In Progress")).toEqual([]);
  });

  it("shows the empty state with the next step for an empty project", async () => {
    setup(boardView([]));
    const panel = await screen.findByRole("region", { name: "No tasks yet" });
    expect(within(column("Todo")).getByText("No tasks yet")).toBeTruthy();
    expect(panel.textContent).toContain("fld claude");
    expect(screen.getByText("0 tasks")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Attention · 0" })).toBeTruthy();
  });

  it("shows skeleton columns while the board loads", async () => {
    setup(null);
    await screen.findByText("Loading the board…");
    expect(screen.getByRole("button", { name: /^Attention/ }).textContent).toBe(
      "Attention–",
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
    daemon.on("GET /api/v1/projects/demo/board", () => ok(demoBoard()));
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("The board could not load.");
    await userEvent.click(within(alert).getByRole("button", { name: "Retry" }));
    expect(
      await screen.findByRole("link", { name: "Build the login form" }),
    ).toBeTruthy();
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("shows the reconnecting banner with the countdown, Retry and the time of the board shown", async () => {
    const retry = vi.fn();
    setup(demoBoard(), "/p/demo/board", retry);
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
