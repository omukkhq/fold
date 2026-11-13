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
import type { Board } from "@/api/board";
import { keys } from "@/api/keys";
import { formatAsOf } from "@/format/time";
import { applyEvent } from "@/live/apply";
import { isEventEnvelope } from "@/live/envelope";
import { LiveRetryContext } from "@/live/retry";
import { liveStatusStore } from "@/live/status";
import { daemon, ok, refuse } from "@/test/daemon";
import { boardCard, boardView } from "@/test/payloads";
import { areaEntry, workerList } from "@/test/workers";
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
        attention: [{ kind: "question", closable: true }],
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
        attention: [{ kind: "human_review", closable: false }],
      }),
      boardCard("T-7", {
        title: "Quiet worker",
        status: "in_progress",
        worker: "backend/db",
        attention: [
          { kind: "stalled", closable: false },
          { kind: "stale_claim", closable: false },
        ],
      }),
      boardCard("T-8", { status: "done", title: "Older", completed_at: 10 }),
      boardCard("T-9", { status: "done", title: "Newest", completed_at: 30 }),
      boardCard("T-10", { status: "done", title: "Middle", completed_at: 20 }),
    ],
    { done: 5 },
  );
}

/** Two areas over the board's labels: `web` owns the `ui` tasks and `api` the `auth` tasks. */
function demoWorkers() {
  return workerList({
    areas: [
      areaEntry("api", "auth", { lead: "api-lead" }),
      areaEntry("web", "ui", { lead: "web-lead" }),
    ],
  });
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
    client.setQueryData(keys.workers("demo"), demoWorkers());
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

// An open menu hides the page from the accessibility tree, and the cards behind a menu of tick boxes still change.
const column = (name: string) =>
  screen.getByRole("region", { name, hidden: true });
const tasksIn = (name: string) =>
  within(column(name))
    .queryAllByRole("link", { hidden: true })
    .map((link) => link.closest("[data-task]")?.getAttribute("data-task"))
    .filter((task) => task !== undefined);

const cardOf = (task: string): HTMLElement => {
  const card = document.querySelector<HTMLElement>(`[data-task="${task}"]`);
  if (card === null) throw new Error(`no card ${task}`);
  return card;
};

/** Focuses the card's title, which opens its detail panel at once. */
async function openPanel(title: string): Promise<HTMLElement> {
  act(() => {
    screen.getByRole("link", { name: title }).focus();
  });
  return (await screen.findByText("Click or press Enter to open the task"))
    .parentElement as HTMLElement;
}

const rowText = (panel: HTMLElement, name: string) =>
  within(panel).getByText(name, { selector: "dt" }).nextElementSibling
    ?.textContent;

/** Finds the innermost element whose whole text is `text`, as a chip whose ids sit in their own spans so they never break. */
function wholeText(text: string) {
  return (_content: string, element: Element | null) =>
    element?.textContent === text &&
    ![...element.children].some((child) => child.textContent === text);
}

describe("BoardScreen", () => {
  it("shows the five columns from Backlog to Done with their cards", async () => {
    setup();
    await screen.findByRole("region", { name: "Backlog" });
    const regions = screen
      .getAllByRole("region")
      .map((r) => r.getAttribute("aria-label"));
    expect(regions).toEqual([
      "Backlog",
      "To do",
      "In progress",
      "Review",
      "Done",
    ]);
    expect(tasksIn("To do")).toEqual(["T-1", "T-6"]);
    expect(screen.getByText("12 tasks · 2 ready")).toBeTruthy();
  });

  it("lists the Done column newest first with its total, its limit hint and the link to the Repository", async () => {
    setup();
    await screen.findByRole("region", { name: "Done" });
    expect(tasksIn("Done")).toEqual(["T-9", "T-10", "T-8"]);
    // The column's hint is a tooltip on its head.
    expect(screen.queryByText(/Integrated into main/)).toBeNull();
    act(() => {
      within(column("Done"))
        .getByRole("heading", { name: "Done" })
        .parentElement?.focus();
    });
    expect(
      (await screen.findAllByText("Integrated into main · latest 3")).length,
    ).toBeGreaterThan(0);
    const more = within(column("Done")).getByRole("link", {
      name: "2 more in Repository",
    });
    expect(more.getAttribute("href")).toBe("/p/demo/repository");
    expect(within(column("Done")).getByText("5")).toBeTruthy();
    expect(
      within(column("To do")).queryByRole("link", {
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
        review_plan: null,
        review_claim: null,
      };
      first.activity = { path: "src/routes/login.tsx", at: Date.now() };
    }
    setup(data);
    await screen.findByRole("region", { name: "Backlog" });
    // The chip names the proposal; the activity line waits in the detail panel.
    expect(screen.getByText(wholeText("P-3 changes requested"))).toBeTruthy();
    expect(screen.queryByText("Editing src/routes/login.tsx")).toBeNull();
    const panel = await openPanel("Add the session cookie and sign-out");
    expect(
      within(panel).getByText("Editing src/routes/login.tsx"),
    ).toBeTruthy();
    expect(within(panel).getByText("P-3 · changes requested")).toBeTruthy();
  });

  it("draws no proposal chip while the payload has none", async () => {
    setup();
    await screen.findByRole("region", { name: "Backlog" });
    expect(screen.queryByText(/^P-\d+ /)).toBeNull();
  });

  it("keeps the fact line of a task that waits for many others to one line and lists them all in the panel", async () => {
    const ids = Array.from(
      { length: 43 },
      (_, index) => `T-${String(148 + index)}`,
    );
    setup(
      boardView([
        boardCard("T-151", {
          title: "Run the controlled web pilot",
          status: "backlog",
          needs: ids,
          blocked_by: ids,
        }),
      ]),
    );
    await screen.findByRole("region", { name: "Backlog" });
    const line = cardOf("T-151").querySelector("p");
    expect(line?.textContent).toBe("T-151·needs T-148, T-149, T-150 +40 more");
    expect(line?.className).toContain("whitespace-nowrap");
    expect(line?.className).not.toContain("flex-wrap");
    const panel = await openPanel("Run the controlled web pilot");
    expect(rowText(panel, "Needs")).toBe(ids.join(", "));
  });

  it("keeps the panel of a long report inside the board area: it is at most as tall as the room, scrolls, and clamps the rendered report with a link to the task", async () => {
    const report = Array.from(
      { length: 30 },
      (_, index) =>
        `- Line ${String(index + 1)} changed \`design/export/copy-${String(index)}.svg\``,
    ).join("\n");
    setup(
      boardView([
        boardCard("T-283", {
          title: "Correct the approved copy lines",
          status: "review",
          kind: "no_proposal",
          report,
          human_review: { reviewer: null, reason: "Copy", set_by: "lead" },
        }),
      ]),
    );
    await screen.findByRole("region", { name: "Review" });
    const panel = await openPanel("Correct the approved copy lines");
    const content = screen.getByRole("dialog", { name: "T-283 details" });
    expect(content.className).toContain(
      "max-h-(--radix-popover-content-available-height)",
    );
    expect(content.className).toContain("overflow-y-auto");
    const text = within(panel)
      .getByText("Report", { selector: "dt" })
      .nextElementSibling?.querySelector("div");
    expect(text?.className).toContain("line-clamp-4");
    expect(text?.querySelectorAll("li")).toHaveLength(30);
    expect(text?.querySelector("code")?.textContent).toBe(
      "design/export/copy-0.svg",
    );
    expect(
      within(panel)
        .getByRole("link", { name: "Open the task to read the full report" })
        .getAttribute("href"),
    ).toBe("/p/demo/t/T-283");
    expect(rowText(panel, "Human review")).toBe("any human");
  });

  it("says a proposal is being reviewed while a reviewer holds its claim, and waits for review otherwise", async () => {
    const open = {
      state: "open" as const,
      version: 1,
      checks_failed: false,
      revision: null,
      review_plan: null,
    };
    setup(
      boardView([
        boardCard("T-1", {
          title: "Held by a reviewer",
          status: "review",
          proposal: { ...open, id: "P-1", review_claim: "review-2" },
        }),
        boardCard("T-2", {
          title: "Waits for a reviewer",
          status: "review",
          proposal: { ...open, id: "P-2", review_claim: null },
        }),
      ]),
    );
    await screen.findByRole("region", { name: "Review" });
    expect(
      within(cardOf("T-1")).getByText(wholeText("P-1 being reviewed")),
    ).toBeTruthy();
    expect(
      within(cardOf("T-2")).getByText(wholeText("P-2 waits for review")),
    ).toBeTruthy();
    const panel = await openPanel("Held by a reviewer");
    expect(rowText(panel, "Proposal")).toBe(
      "P-1 · being reviewed by @review-2",
    );
  });

  it("gives a card its title, one line of facts and the rest in the detail panel", async () => {
    setup();
    await screen.findByRole("region", { name: "Backlog" });
    const card = cardOf("T-2");
    expect(within(card).getByText("Build the login form")).toBeTruthy();
    expect(card.querySelector("p")?.textContent).toBe("T-2·ui·@frontend/ui");
    expect(card.textContent).not.toContain("messages");
    const panel = await openPanel("Build the login form");
    expect(within(panel).getByText("T-2 · In progress")).toBeTruthy();
    expect(rowText(panel, "Messages")).toBe("2");
    expect(rowText(panel, "Checklist")).toBe("1/4");
    expect(rowText(panel, "Worker")).toBe("@frontend/ui");
    expect(
      within(panel).getByText("Click or press Enter to open the task"),
    ).toBeTruthy();
  });

  it("opens the detail panel once the pointer rests on a card, and closes it when the pointer leaves", async () => {
    setup();
    await screen.findByRole("region", { name: "Backlog" });
    const card = cardOf("T-1");
    await userEvent.hover(card);
    expect(
      await screen.findByText("T-1 · To do", {}, { timeout: 1_000 }),
    ).toBeTruthy();
    await userEvent.unhover(card);
    await waitFor(() => {
      expect(screen.queryByText("T-1 · To do")).toBeNull();
    });
  });

  it("renders every attention kind of the payload and nothing else", async () => {
    setup();
    await screen.findByRole("region", { name: "Backlog" });
    expect(within(cardOf("T-3")).getByText("Question for you")).toBeTruthy();
    expect(within(cardOf("T-6")).getByText("Your review")).toBeTruthy();
    expect(within(cardOf("T-7")).getByText("Stalled · needs you")).toBeTruthy();
    expect(
      within(cardOf("T-5")).getByText(wholeText("Blocked by T-3")),
    ).toBeTruthy();
    expect(
      within(cardOf("T-4")).getByText(wholeText("needs T-2")),
    ).toBeTruthy();
    // One chip per card: the stale claim and the human review mark wait in the panel.
    expect(screen.queryByText("Stale claim · needs you")).toBeNull();
    const quiet = await openPanel("Quiet worker");
    expect(
      within(quiet).getByText("Stale claim: the worker's lease ran out."),
    ).toBeTruthy();
    const review = await openPanel("Needs a person");
    expect(rowText(review, "Human review")).toBe("@tanim");
    // A card without an item has no chip, however quiet its worker is.
    expect(cardOf("T-1").textContent).not.toContain("needs you");
  });

  it("has nothing to drag and no control that moves a card", async () => {
    setup();
    await screen.findByRole("region", { name: "Backlog" });
    expect(document.body.querySelectorAll('[draggable="true"]')).toHaveLength(
      0,
    );
    for (const link of within(column("In progress")).getAllByRole("link")) {
      expect(link.getAttribute("draggable")).toBe("false");
    }
    for (const name of ["Backlog", "To do", "In progress", "Review", "Done"]) {
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
    expect(
      within(review).queryByRole("button", { name: "Mark answered" }),
    ).toBeNull();
    // Mark answered is the panel's one button, and Tab reaches it from the card's title.
    screen.getByRole("link", { name: "Review the gate" }).focus();
    await userEvent.tab();
    const button = within(review).getByRole("button", {
      name: "Mark answered",
    });
    expect(document.activeElement).toBe(button);
    await userEvent.click(button);
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
    expect(tasksIn("To do")).toContain("T-1");
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
      expect(tasksIn("In progress")).toContain("T-1");
    });
    expect(tasksIn("To do")).not.toContain("T-1");
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
    expect(tasksIn("In progress")).toEqual([]);
    await userEvent.click(screen.getByRole("button", { name: "Blocked only" }));
    expect(router.state.location.search).toEqual({});

    await userEvent.click(screen.getByRole("button", { name: /^Attention/ }));
    expect(router.state.location.search).toEqual({ needs: true });
    expect(tasksIn("To do")).toEqual(["T-6"]);
    expect(tasksIn("Review")).toEqual(["T-3"]);
    expect(tasksIn("In progress")).toEqual(["T-7"]);
  });

  it("filters by owner from the Owner menu, and the toolbar draws no agent pills", async () => {
    const { router } = setup();
    await screen.findByRole("region", { name: "Backlog" });
    expect(screen.queryByRole("group", { name: "Agents" })).toBeNull();
    expect(screen.queryByRole("button", { name: /^Agent/ })).toBeNull();

    screen.getByRole("button", { name: "Owner" }).focus();
    await userEvent.keyboard("{Enter}");
    expect(
      screen.getAllByRole("menuitemcheckbox").map((item) => item.textContent),
    ).toEqual(["Human", "Main agent", "api-lead api", "web-lead web"]);
    // Clear leads the list from the start, so a tick moves no row under the pointer.
    expect(
      screen
        .getByRole("menuitem", { name: "Clear" })
        .getAttribute("aria-disabled"),
    ).toBe("true");
    await userEvent.click(
      screen.getByRole("menuitemcheckbox", { name: "web-lead web" }),
    );
    expect(router.state.location.search).toEqual({ owner: ["area:web"] });
    expect(tasksIn("In progress")).toEqual(["T-2"]);
    expect(tasksIn("To do")).toEqual(["T-6"]);
    expect(
      screen.getByRole("button", { name: "Owner: web-lead", hidden: true }),
    ).toBeTruthy();
    // The menu stays open, and Clear resets the filter.
    await userEvent.click(screen.getByRole("menuitem", { name: "Clear" }));
    expect(router.state.location.search).toEqual({});
    expect(screen.getByRole("button", { name: "Owner" })).toBeTruthy();
  });

  it("ticks two labels and two owners: any tick inside a menu, every filter together, and the link opens the same board", async () => {
    const board = demoBoard();
    const human = boardCard("T-11", {
      title: "Sign the release",
      kind: "human",
      labels: [ui],
    });
    const mainTask = boardCard("T-12", {
      title: "Route the backlog",
      labels: [{ name: "main", color: "ink" }],
    });
    board.columns[1]?.cards.push(human, mainTask);
    const { router } = setup(board);
    await screen.findByRole("region", { name: "Backlog" });

    screen.getByRole("button", { name: "Labels" }).focus();
    await userEvent.keyboard("{Enter}");
    await userEvent.click(screen.getByRole("menuitemcheckbox", { name: "ui" }));
    await userEvent.click(
      screen.getByRole("menuitemcheckbox", { name: "main" }),
    );
    expect(router.state.location.search).toEqual({ label: ["ui", "main"] });
    expect(tasksIn("To do")).toEqual(["T-6", "T-11", "T-12"]);
    expect(tasksIn("In progress")).toEqual(["T-2"]);
    await userEvent.keyboard("{Escape}");
    expect(screen.getByRole("button", { name: "Labels: 2" })).toBeTruthy();

    screen.getByRole("button", { name: "Owner" }).focus();
    await userEvent.keyboard("{Enter}");
    await userEvent.click(
      screen.getByRole("menuitemcheckbox", { name: "Human" }),
    );
    await userEvent.click(
      screen.getByRole("menuitemcheckbox", { name: "api-lead api" }),
    );
    expect(router.state.location.search).toEqual({
      label: ["ui", "main"],
      owner: ["human", "area:api"],
    });
    // T-6 carries ui and auth, T-11 is a human task with ui; T-12 and T-2 have no ticked owner, T-1 no ticked label.
    expect(tasksIn("To do")).toEqual(["T-6", "T-11"]);
    expect(tasksIn("In progress")).toEqual([]);
    await userEvent.keyboard("{Escape}");
    expect(screen.getByRole("button", { name: "Owners: 2" })).toBeTruthy();

    const href = router.state.location.href;
    cleanup();
    setup(board, href);
    await screen.findByRole("region", { name: "Backlog" });
    expect(tasksIn("To do")).toEqual(["T-6", "T-11"]);
    expect(tasksIn("In progress")).toEqual([]);
    expect(screen.getByRole("button", { name: "Labels: 2" })).toBeTruthy();
  });

  it("opens a link with one bare label and an agent param, and searches", async () => {
    const { router } = setup(
      demoBoard(),
      "/p/demo/board?label=ui&agent=frontend",
    );
    await screen.findByRole("region", { name: "Backlog" });
    expect(tasksIn("To do")).toEqual(["T-6"]);
    expect(tasksIn("In progress")).toEqual(["T-2"]);
    expect(screen.getByRole("button", { name: "Label: ui" })).toBeTruthy();
    await userEvent.type(
      screen.getByRole("searchbox", { name: "Search tasks" }),
      "nothing",
    );
    // The next change of a filter writes the link again without the agent.
    expect(router.state.location.search).toEqual({
      label: ["ui"],
      q: "nothing",
    });
    expect(tasksIn("In progress")).toEqual([]);
  });

  it("waits for the workers answer before it filters by a lead, and says when it fails", async () => {
    const client = new QueryClient({
      defaultOptions: { queries: { staleTime: Infinity, retry: false } },
    });
    client.setQueryData(keys.board("demo"), demoBoard());
    let answer: () => void = () => undefined;
    daemon.on(
      "GET /api/v1/projects/:slug/workers",
      () =>
        new Promise((resolve) => {
          answer = () => {
            resolve(ok(demoWorkers()));
          };
        }),
    );
    render(
      <QueryClientProvider client={client}>
        <RouterProvider router={makeRouter("/p/demo/board?owner=area:web")} />
      </QueryClientProvider>,
    );
    expect(
      await screen.findAllByRole("status", { name: "Loading" }),
    ).not.toHaveLength(0);
    expect(screen.getByText("Loading the board…")).toBeTruthy();
    act(() => {
      answer();
    });
    await waitFor(() => {
      expect(tasksIn("To do")).toEqual(["T-6"]);
    });
    // A failed read after the owners arrived keeps them, so the filter still tells the lead's tasks and no banner shows.
    daemon.on("GET /api/v1/projects/:slug/workers", refuse(500, "internal"));
    await act(() => client.refetchQueries({ queryKey: keys.workers("demo") }));
    await waitFor(() => {
      expect(client.getQueryState(keys.workers("demo"))?.status).toBe("error");
    });
    expect(screen.queryByText("The owners could not load.")).toBeNull();
    expect(tasksIn("To do")).toEqual(["T-6"]);
    cleanup();

    daemon.reset();
    daemon.on("GET /api/v1/projects/:slug/workers", refuse(500, "internal"));
    const failing = new QueryClient({
      defaultOptions: { queries: { staleTime: Infinity, retry: false } },
    });
    failing.setQueryData(keys.board("demo"), demoBoard());
    render(
      <QueryClientProvider client={failing}>
        <RouterProvider
          router={makeRouter('/p/demo/board?owner=["human","main"]')}
        />
      </QueryClientProvider>,
    );
    expect(await screen.findByText("The owners could not load.")).toBeTruthy();
  });

  it("shows the empty state with the next step for an empty project", async () => {
    setup(boardView([]));
    const panel = await screen.findByRole("region", { name: "No tasks yet" });
    expect(within(column("To do")).getByText("No tasks yet")).toBeTruthy();
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

  it("shows one column at a time on a phone, opening on the first that needs you", async () => {
    vi.stubGlobal(
      "matchMedia",
      vi.fn((query: string) => ({
        matches: query.includes("max-width"),
        addEventListener: vi.fn(),
        removeEventListener: vi.fn(),
      })),
    );
    try {
      const { router } = setup();
      const switcher = await screen.findByRole("tablist", { name: "Column" });
      const tabs = within(switcher)
        .getAllByRole("tab")
        .map((tab) => tab.textContent);
      expect(tabs).toEqual([
        "Backlog 2",
        "To do 2",
        "In progress 2",
        "Review 1",
        "Done 5",
      ]);
      expect(
        screen.getAllByRole("region").map((r) => r.getAttribute("aria-label")),
      ).toEqual(["To do"]);
      expect(screen.getByText("To do: ready to start")).toBeTruthy();
      expect(screen.getByText("2 tasks")).toBeTruthy();
      expect(tasksIn("To do")).toEqual(["T-1", "T-6"]);
      await userEvent.click(
        within(switcher).getByRole("tab", { name: "Done 5" }),
      );
      expect(tasksIn("Done")).toEqual(["T-9", "T-10", "T-8"]);
      // A phone has no hover: focus opens no panel.
      act(() => {
        screen.getByRole("link", { name: "Newest" }).focus();
      });
      expect(
        screen.queryByText("Click or press Enter to open the task"),
      ).toBeNull();

      expect(screen.queryByRole("button", { name: "Blocked only" })).toBeNull();
      await userEvent.click(
        screen.getByRole("button", {
          name: "Filters: labels, owner, blocked only",
        }),
      );
      await userEvent.click(
        await screen.findByRole("button", { name: "Blocked only" }),
      );
      expect(router.state.location.search).toEqual({ blocked: true });
      expect(
        screen.getByRole("button", {
          name: "Filters: labels, owner, blocked only; a filter is set",
        }),
      ).toBeTruthy();
    } finally {
      vi.unstubAllGlobals();
    }
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
