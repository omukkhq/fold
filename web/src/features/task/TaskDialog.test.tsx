// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  RouterProvider,
  createMemoryHistory,
  createRouter,
} from "@tanstack/react-router";
import {
  cleanup,
  configure,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import {
  afterAll,
  afterEach,
  beforeAll,
  beforeEach,
  describe,
  expect,
  it,
  vi,
} from "vitest";
import type { Board, BoardCard } from "@/api/board";
import type { components } from "@/api/schema";
import type { TaskDetail } from "@/api/tasks";
import { liveStatusStore } from "@/live/status";
import { agent, worker } from "@/test/agents";
import { daemon, noContent, ok, refuse, type Call } from "@/test/daemon";
import { boardCard, boardView, taskView } from "@/test/payloads";
import { routeTree } from "@/routeTree.gen";
import { ThemeProvider } from "@/theme/ThemeProvider";
import { Markdown } from "./Markdown";

vi.mock("@/api/client", async (importOriginal) => {
  const real = await importOriginal<typeof import("@/api/client")>();
  const { daemon: fake } = await import("@/test/daemon");
  return { ...real, api: fake.api };
});

// The fixtures answer without the development delay, so the whole app, board included, can be driven test after test.
vi.mock("@/api/fixture", async (importOriginal) => {
  const real = await importOriginal<typeof import("@/api/fixture")>();
  return {
    ...real,
    fromFixture: <M, T>(
      load: (() => Promise<M>) | undefined,
      pick: (module: M) => T,
      empty: T,
    ) => real.fromFixture(load, pick, empty, 0),
  };
});

type MessageView = components["schemas"]["MessageView"];

/** A message id written out reads as a spacing utility to the token guard. */
const messageId = (n: number) => `m-${String(n)}`;

const WAIT = { timeout: 4_000 };
const TITLE = "Build the login form with validation";
const P = "/api/v1/projects";

// The board stays mounted beneath the dialog, and the visibility check of every role query over it is slow in jsdom.
beforeAll(() => {
  configure({ defaultHidden: true });
});
afterAll(() => {
  configure({ defaultHidden: false });
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

const ui = { name: "ui", color: "sage" };
const auth = { name: "auth", color: "slate" };
const STEP = "Map invalid_credentials and rate_limited to messages";

/** The tasks of the project, kept by the fake daemon between requests. */
let tasks: Map<string, TaskDetail>;
let labels: { name: string; color: string }[];
let nextTask: number;
let archiveRefusal = false;
/** The messages linked to T-2, as the message route answers them. */
let linked: MessageView[];

function seed(): void {
  nextTask = 17;
  linked = [];
  labels = [ui, auth];
  tasks = new Map([
    [
      "T-2",
      taskView("T-2", {
        title: TITLE,
        status: "in_progress",
        description: "Build the form.",
        labels: [ui],
        workspace: "ws-2",
        claims: [
          {
            kind: "implementation",
            holder: "frontend/ui",
            claimed_at: 1_000,
            renewed_at: 2_000,
            stale: false,
          },
        ],
        checklists: [
          {
            name: "Subtasks",
            items: [
              { id: "ci-1", kind: "step", text: STEP, done: false },
              { id: "ci-2", kind: "step", text: "Wire the form", done: true },
              {
                id: "ci-5",
                kind: "linked",
                task: "T-13",
                title: "Form field components",
                status: "done",
                done: true,
              },
            ],
          },
        ],
        dependencies: [
          {
            task: "T-13",
            title: "Form field components",
            status: "done",
            direction: "needs",
            origin: "checklist",
          },
          {
            task: "T-1",
            title: "Session cookie",
            status: "todo",
            direction: "needs",
            origin: "direct",
          },
          {
            task: "T-5",
            title: "Login flow",
            status: "backlog",
            direction: "needed_by",
            origin: "direct",
          },
        ],
        human_review: null,
      }),
    ],
    ["T-1", taskView("T-1", { title: "Session cookie" })],
    ["T-5", taskView("T-5", { title: "Login flow", status: "backlog" })],
    [
      "T-13",
      taskView("T-13", { title: "Form field components", status: "done" }),
    ],
    ["T-16", taskView("T-16", { title: "Scaffold the web app" })],
  ]);
}

function cardOf(task: TaskDetail): BoardCard {
  return boardCard(task.task, {
    title: task.title,
    status: task.status,
    blocked: task.blocked,
    labels: task.labels,
    worker: task.claims[0]?.holder ?? null,
    workspace: task.workspace,
  });
}

function board(): Board {
  const all = [...tasks.values()].map(cardOf);
  return boardView(all);
}

function view(task: string): TaskDetail {
  const found = tasks.get(task);
  if (!found) throw new Error(`no task ${task}`);
  return found;
}

function patch(task: string, change: Partial<TaskDetail>): TaskDetail {
  const next = { ...view(task), ...change };
  tasks.set(task, next);
  return next;
}

function items(task: string) {
  return view(task).checklists[0]?.items ?? [];
}

function setItems(task: string, next: ReturnType<typeof items>) {
  patch(task, { checklists: [{ name: "Subtasks", items: next }] });
}

function registerDaemon(): void {
  daemon.latency = 30;
  daemon.shell(["demo", "ledger"]);
  const known = (call: Call) => tasks.has(call.params.id ?? "");
  const missing = refuse(404, "task_not_found");
  daemon.on(`GET ${P}/:slug/board`, () => ok(board()));
  daemon.on(`GET ${P}/:slug/tasks/:id`, (call) =>
    known(call) ? ok(view(call.params.id ?? "")) : missing,
  );
  daemon.on(`POST ${P}/:slug/tasks`, (call) => {
    const body = call.body as { title: string; description?: string };
    const id = `T-${String(nextTask++)}`;
    tasks.set(
      id,
      taskView(id, { title: body.title, description: body.description ?? "" }),
    );
    return ok(view(id), 201);
  });
  daemon.on(`PATCH ${P}/:slug/tasks/:id`, (call) => {
    const body = call.body as {
      title?: string;
      description?: string;
      labels?: string[];
    };
    const id = call.params.id ?? "";
    const change: Partial<TaskDetail> = {};
    if (body.title !== undefined) change.title = body.title;
    if (body.description !== undefined) change.description = body.description;
    if (body.labels !== undefined) {
      change.labels = body.labels.flatMap((name) => {
        const found = labels.find((l) => l.name === name);
        return found ? [found] : [];
      });
    }
    return ok(patch(id, change));
  });
  daemon.on(`GET ${P}/:slug/labels`, () => ok({ labels }));
  daemon.on(`POST ${P}/:slug/labels`, (call) => {
    const body = call.body as { name: string; color: string };
    if (labels.some((l) => l.name === body.name)) {
      return refuse(409, "label_exists");
    }
    labels = [...labels, body];
    return ok(body, 201);
  });
  daemon.on(`POST ${P}/:slug/tasks/:id/dependencies`, (call) => {
    const id = call.params.id ?? "";
    const { needs } = call.body as { needs: string };
    const task = view(id);
    if (!tasks.has(needs)) return refuse(404, "task_not_found");
    if (
      task.dependencies.some(
        (d) => d.task === needs && d.direction === "needed_by",
      )
    ) {
      return refuse(409, "dependency_cycle", { path: [id, needs, id] });
    }
    const other = view(needs);
    patch(id, {
      dependencies: [
        ...task.dependencies,
        {
          task: needs,
          title: other.title,
          status: other.status,
          direction: "needs",
          origin: "direct",
        },
      ],
    });
    return noContent();
  });
  daemon.on(`DELETE ${P}/:slug/tasks/:id/dependencies/:needs`, (call) => {
    const id = call.params.id ?? "";
    patch(id, {
      dependencies: view(id).dependencies.filter(
        (d) => d.task !== call.params.needs,
      ),
    });
    return noContent();
  });
  daemon.on(`POST ${P}/:slug/tasks/:id/checklist-items`, (call) => {
    const id = call.params.id ?? "";
    const body = call.body as { text?: string };
    setItems(id, [
      ...items(id),
      { id: "ci-9", kind: "step", text: body.text ?? "", done: false },
    ]);
    return ok({}, 201);
  });
  daemon.on(`PATCH ${P}/:slug/checklist-items/:id`, (call) => {
    const body = call.body as { done?: boolean };
    setItems(
      "T-2",
      items("T-2").map((item) =>
        item.id === call.params.id && body.done !== undefined
          ? { ...item, done: body.done }
          : item,
      ),
    );
    return ok({});
  });
  daemon.on(`DELETE ${P}/:slug/checklist-items/:id`, (call) => {
    setItems(
      "T-2",
      items("T-2").filter((item) => item.id !== call.params.id),
    );
    patch("T-2", {
      dependencies: view("T-2").dependencies.filter((d) => d.task !== "T-13"),
    });
    return noContent();
  });
  daemon.on(`GET ${P}/:slug/agents`, () =>
    ok({
      agents: [
        agent("frontend", {
          workers: [worker("frontend"), worker("frontend/ui")],
        }),
      ],
    }),
  );
  daemon.on(`GET ${P}/:slug/messages`, (call) =>
    ok({
      items: call.path.includes("task=T-2")
        ? linked.map((message) => ({
            id: message.thread,
            awaiting_human: false,
            last_at: message.sent_at,
            messages: [message],
            participants: [message.sender],
            proposal: null,
            task: "T-2",
          }))
        : [],
      next: null,
      total_awaiting_human: 0,
    }),
  );
  daemon.on(`POST ${P}/:slug/feedback`, (call) => {
    const body = call.body as { body: string };
    linked = [
      ...linked,
      {
        id: messageId(90 + linked.length),
        thread: messageId(90 + linked.length),
        sender: "human:tanim",
        recipients: [],
        body: body.body,
        proposal: null,
        reply_to: null,
        task: "T-2",
        sent_at: 5_000,
      },
    ];
    return ok({}, 201);
  });
  daemon.on(`POST ${P}/:slug/tasks/:id/archive`, (call) => {
    if (archiveRefusal) {
      return refuse(400, "invalid_request", { reason: "proposal_open" });
    }
    tasks.delete(call.params.id ?? "");
    return noContent();
  });
}

beforeEach(() => {
  seed();
  archiveRefusal = false;
  registerDaemon();
});

afterEach(() => {
  cleanup();
  daemon.reset();
  liveStatusStore.reset();
});

function setup(path: string) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const router = createRouter({
    routeTree,
    context: { queryClient },
    history: createMemoryHistory({ initialEntries: [path] }),
  });
  render(
    <ThemeProvider>
      <QueryClientProvider client={queryClient}>
        <RouterProvider router={router} />
      </QueryClientProvider>
    </ThemeProvider>,
  );
  return { router, queryClient };
}

async function openTask(task = "T-2", title = TITLE) {
  const { router, queryClient } = setup(`/p/demo/t/${task}`);
  const dialog = await screen.findByRole("dialog", {}, WAIT);
  await within(dialog).findByRole("heading", { name: title }, WAIT);
  return { router, queryClient, dialog };
}

const region = (dialog: HTMLElement, name: string) =>
  within(dialog).getByRole("region", { name });

describe("the task dialog route", () => {
  it("opens from a board card with the board's filters and shows the sections the task payload has data for", async () => {
    const { router } = setup("/p/demo/board?label=ui");
    const link = await screen.findByRole("link", { name: TITLE }, WAIT);
    expect(link.getAttribute("href")).toBe("/p/demo/t/T-2?label=ui");
    await userEvent.click(link);
    const dialog = await screen.findByRole("dialog", {}, WAIT);
    await within(dialog).findByRole("heading", { name: TITLE }, WAIT);
    expect(router.state.location.pathname).toBe("/p/demo/t/T-2");
    expect(router.state.location.search).toEqual({ label: "ui" });

    expect(within(dialog).getByText("In Progress")).toBeTruthy();
    for (const name of [
      "Description",
      "Subtasks",
      "Working changes",
      "Messages",
      "Responsible",
      "Workspace",
      "Checkpoints",
      "Human review",
      "Dependencies",
    ]) {
      expect(region(dialog, name), name).toBeTruthy();
    }
    // The task read carries no changesets or proposal yet, so these draw their empty states; History has none and is left out.
    expect(
      within(region(dialog, "Changesets")).getByText("No changesets yet."),
    ).toBeTruthy();
    expect(
      within(region(dialog, "Proposal")).getByText(/None yet\. It opens when/),
    ).toBeTruthy();
    expect(
      within(dialog).queryByRole("region", { name: "History" }),
    ).toBeNull();
    expect(
      await within(region(dialog, "Messages")).findByText(
        "0 linked to T-2",
        {},
        WAIT,
      ),
    ).toBeTruthy();
    expect(within(region(dialog, "Workspace")).getByText("ws-2")).toBeTruthy();

    // The development event stream reports the workspace; the task payload names only its id.
    const changes = region(dialog, "Working changes");
    const [fileLink] = await within(changes).findAllByRole(
      "link",
      { name: /\.ts/ },
      WAIT,
    );
    expect(fileLink?.getAttribute("href")).toMatch(
      /^\/p\/demo\/diff\?to=ws-2&path=/,
    );
    expect(
      within(changes)
        .getByRole("link", { name: "Open diff" })
        .getAttribute("href"),
    ).toBe("/p/demo/diff?to=ws-2");
    // Without the workspace details there is no main head to compare with.
    expect(
      within(changes).queryByRole("link", { name: "Compare with main" }),
    ).toBeNull();
    expect(within(dialog).getByText("is editing")).toBeTruthy();
    const deps = region(dialog, "Dependencies");
    expect(within(deps).getByText("Form field components")).toBeTruthy();
    expect(within(deps).getByText("Login flow")).toBeTruthy();
  }, 15_000);

  it("draws every section the task read carries once the daemon sends them", async () => {
    const at = new Date(2026, 9, 15, 13, 40).getTime();
    linked = [
      {
        id: messageId(31),
        thread: messageId(30),
        sender: "frontend/ui",
        recipients: [
          {
            name: "review",
            addressed_as: "agent",
            delivered_at: null,
            acknowledged_at: null,
          },
        ],
        body: "Ready for review",
        proposal: null,
        reply_to: null,
        task: "T-2",
        sent_at: at,
      },
    ];
    patch("T-2", {
      status_reason: "Calculated: workspace ws-2 is open, no proposal yet",
      workspace_info: {
        directory: "ws/T-2",
        base: "r7",
        main: "r8",
        behind: 1,
      },
      checkpoints: [
        // Newer than any checkpoint the development stream reports, so it leads the list.
        {
          id: "ck-31",
          label: null,
          automatic: true,
          created_at: Date.now() + 3_600_000,
        },
      ],
      changesets: [{ id: "C-18", message: "Add the form", created_at: at }],
      proposal: {
        id: "P-3",
        state: "open",
        version: 2,
        reviews: [{ reviewer: "review", verdict: "approve", version: 2, at }],
      },
      integration: { revision: "r9", proposal: "P-2", by: "backend", at },
      history: [
        {
          seq: 3,
          type: "claim.acquired",
          actor: "frontend/ui",
          created_at: at,
        },
        { seq: 2, type: "task.created", actor: "human:tanim", created_at: at },
        { seq: 1, type: "task.updated", actor: "human:tanim", created_at: at },
      ],
      human_review_note: { reason: "Auth work", set_by: "planner", set_at: at },
      human_review: { reviewer: "tanim" },
    });
    const { dialog } = await openTask();
    expect(
      within(dialog).getByText(
        "Calculated: workspace ws-2 is open, no proposal yet",
      ),
    ).toBeTruthy();
    const ws = region(dialog, "Workspace");
    expect(within(ws).getByText("ws/T-2")).toBeTruthy();
    expect(within(ws).getByText("r7 (main is r8)")).toBeTruthy();
    expect(within(ws).getByText(/Main moved 1 revision/)).toBeTruthy();
    expect(
      within(region(dialog, "Working changes"))
        .getByRole("link", { name: "Compare with main" })
        .getAttribute("href"),
    ).toBe("/p/demo/diff?from=r8&to=ws-2");
    expect(
      within(region(dialog, "Checkpoints")).getByText("ck-31"),
    ).toBeTruthy();
    expect(within(region(dialog, "Changesets")).getByText("C-18")).toBeTruthy();
    const proposal = region(dialog, "Proposal");
    expect(
      within(proposal).getByRole("link", { name: "P-3" }).getAttribute("href"),
    ).toBe("/p/demo/proposals/P-3");
    expect(within(proposal).getByText("review approved on v2")).toBeTruthy();
    expect(
      within(proposal).getByRole("link", { name: "r9" }).getAttribute("href"),
    ).toBe("/p/demo/repository?revision=r9");
    const messages = region(dialog, "Messages");
    expect(
      await within(messages).findByText("1 linked to T-2", {}, WAIT),
    ).toBeTruthy();
    expect(within(messages).getByText("Ready for review")).toBeTruthy();
    expect(
      within(messages)
        .getByRole("link", {
          name: new RegExp(`Open the thread of ${messageId(31)}`),
        })
        .getAttribute("href"),
    ).toBe(`/p/demo/messages?thread=${messageId(30)}`);
    const history = region(dialog, "History");
    expect(within(history).getByText(/Claimed by frontend\/ui/)).toBeTruthy();
    expect(within(history).queryByText(/Edited by/)).toBeNull();
    await userEvent.click(
      within(history).getByRole("button", { name: "Show 1 more" }),
    );
    expect(within(history).getByText(/Edited by human:tanim/)).toBeTruthy();
    const review = region(dialog, "Human review");
    expect(within(review).getByText("Required · tanim")).toBeTruthy();
    expect(within(review).getByText(/Set by planner/)).toBeTruthy();
    expect(within(review).getByText("Auth work")).toBeTruthy();
  }, 20_000);

  it("shows the review claim when the daemon sends one", async () => {
    patch("T-2", {
      claims: [
        ...view("T-2").claims,
        // The generated claim kind names the implementation claim only.
        {
          kind: "review" as never,
          holder: "review",
          claimed_at: 1_000,
          renewed_at: 2_000,
          stale: false,
        },
      ],
    });
    const { dialog } = await openTask();
    expect(
      within(region(dialog, "Responsible")).getByText(/Review claim:/),
    ).toBeTruthy();
  }, 15_000);

  it("closes back to the board with its filters", async () => {
    const router = setup("/p/demo/t/T-2?label=ui").router;
    const dialog = await screen.findByRole("dialog", {}, WAIT);
    await within(dialog).findByRole("heading", { name: TITLE }, WAIT);
    // A linked item and a dependency open their task over the same filtered board.
    expect(
      within(region(dialog, "Subtasks"))
        .getByRole("link", { name: /Form field components/ })
        .getAttribute("href"),
    ).toBe("/p/demo/t/T-13?label=ui");
    expect(
      within(region(dialog, "Dependencies"))
        .getByRole("link", { name: /Login flow/ })
        .getAttribute("href"),
    ).toBe("/p/demo/t/T-5?label=ui");
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Close" }),
    );
    await waitFor(() => {
      expect(router.state.location.pathname).toBe("/p/demo/board");
    });
    expect(router.state.location.search).toEqual({ label: "ui" });
    expect(screen.queryByRole("dialog")).toBeNull();
  }, 15_000);

  it("says a task that does not exist is not there", async () => {
    setup("/p/demo/t/T-99");
    const dialog = await screen.findByRole("dialog", {}, WAIT);
    expect(
      await within(dialog).findByRole(
        "heading",
        { name: "T-99 does not exist" },
        WAIT,
      ),
    ).toBeTruthy();
    expect(
      within(dialog).queryByRole("button", { name: "Copy link" }),
    ).toBeNull();
  }, 15_000);
});

describe("creating a task", () => {
  it("opens the create form from the tab row and opens the created task", async () => {
    const { router } = setup("/p/demo/board?label=ui");
    const nav = await screen.findByRole(
      "navigation",
      { name: "Project" },
      WAIT,
    );
    const link = within(nav).getByRole("link", { name: "New task" });
    expect(link.getAttribute("href")).toBe("/p/demo/t/new");
    await userEvent.click(link);
    const dialog = await screen.findByRole("dialog", {}, WAIT);
    expect(
      within(dialog).getByRole("heading", { name: "New task" }),
    ).toBeTruthy();
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Create task" }),
    );
    expect(await within(dialog).findByText("Write a title.")).toBeTruthy();
    await userEvent.type(
      within(dialog).getByRole("textbox", { name: "Title" }),
      "Password reset",
    );
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Create task" }),
    );
    expect(
      within(dialog)
        .getByRole("button", { name: "Create task" })
        .getAttribute("aria-busy"),
    ).toBe("true");
    expect(
      await screen.findByRole("heading", { name: "Password reset" }, WAIT),
    ).toBeTruthy();
    expect(router.state.location.pathname).toBe("/p/demo/t/T-17");
  }, 15_000);

  it("offers New task on the empty board and adds the task to it", async () => {
    tasks.clear();
    const router = setup("/p/ledger/board").router;
    const panel = await screen.findByRole(
      "region",
      { name: "No tasks yet" },
      WAIT,
    );
    expect(
      within(panel).getByText(/You can also add one yourself\./),
    ).toBeTruthy();
    await userEvent.click(
      within(panel).getByRole("link", { name: "New task" }),
    );
    const dialog = await screen.findByRole("dialog", {}, WAIT);
    await userEvent.type(
      within(dialog).getByRole("textbox", { name: "Title" }),
      "First task",
    );
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Create task" }),
    );
    await screen.findByRole("heading", { name: "First task" }, WAIT);
    expect(router.state.location.pathname).toBe("/p/ledger/t/T-17");
    await userEvent.click(screen.getByRole("button", { name: "Close" }));
    expect(
      await screen.findByRole("link", { name: "First task" }, WAIT),
    ).toBeTruthy();
  }, 15_000);

  it("closes the create form back to the board", async () => {
    const { router } = setup("/p/demo/t/new?blocked=true");
    const dialog = await screen.findByRole("dialog", {}, WAIT);
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Cancel" }),
    );
    await waitFor(() => {
      expect(router.state.location.pathname).toBe("/p/demo/board");
    });
    expect(router.state.location.search).toEqual({ blocked: true });
  }, 15_000);
});

describe("the human edits", () => {
  it("shows the pending state for a title edit, then the confirmed title", async () => {
    const { dialog } = await openTask();
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Edit the title" }),
    );
    const field = within(dialog).getByRole("textbox", { name: "Title" });
    await userEvent.clear(field);
    await userEvent.type(field, "Sign-in form");
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Save title" }),
    );
    const save = within(dialog).getByRole("button", { name: "Save title" });
    expect(save.getAttribute("aria-busy")).toBe("true");
    expect(
      within(dialog).queryByRole("heading", { name: "Sign-in form" }),
    ).toBeNull();
    expect(
      await within(dialog).findByRole(
        "heading",
        { name: "Sign-in form" },
        WAIT,
      ),
    ).toBeTruthy();
    expect(
      within(dialog).queryByRole("button", { name: "Save title" }),
    ).toBeNull();
  }, 15_000);

  it("refuses an empty title in the form without a write", async () => {
    const { dialog } = await openTask();
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Edit the title" }),
    );
    await userEvent.clear(
      within(dialog).getByRole("textbox", { name: "Title" }),
    );
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Save title" }),
    );
    expect(await within(dialog).findByText("Write a title.")).toBeTruthy();
    expect(daemon.made(`PATCH ${P}/demo/tasks/T-2`)).toHaveLength(0);
  }, 15_000);

  it("shows the pending state for a description edit, then the rendered Markdown", async () => {
    const { dialog } = await openTask();
    const section = region(dialog, "Description");
    await userEvent.click(
      within(section).getByRole("button", { name: "Edit the description" }),
    );
    const field = within(section).getByRole("textbox", { name: "Description" });
    await userEvent.clear(field);
    await userEvent.type(field, "Only **this** now");
    await userEvent.click(
      within(section).getByRole("button", { name: "Save description" }),
    );
    expect(
      within(section)
        .getByRole("button", { name: "Save description" })
        .getAttribute("aria-busy"),
    ).toBe("true");
    expect(
      await within(section).findByText("this", { selector: "strong" }, WAIT),
    ).toBeTruthy();
  }, 15_000);

  it("puts an existing label on the task, takes it off, and creates a new one", async () => {
    const { dialog } = await openTask();
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Label" }),
    );
    await waitFor(() => {
      expect(screen.getAllByRole("dialog")).toHaveLength(2);
    });
    const popover = screen.getAllByRole("dialog")[1] as HTMLElement;
    await userEvent.click(
      await within(popover).findByRole(
        "button",
        { name: "Add label auth" },
        WAIT,
      ),
    );
    expect(
      await within(popover).findByRole(
        "button",
        { name: "Remove label auth" },
        WAIT,
      ),
    ).toBeTruthy();
    await userEvent.click(
      within(popover).getByRole("button", { name: "Remove label auth" }),
    );
    await waitFor(() => {
      expect(
        within(popover).queryByRole("button", { name: "Remove label auth" }),
      ).toBeNull();
    }, WAIT);

    await userEvent.type(
      within(popover).getByRole("textbox", { name: "New label name" }),
      "copy",
    );
    await userEvent.selectOptions(
      within(popover).getByRole("combobox", { name: "Colour" }),
      "gold",
    );
    await userEvent.click(
      within(popover).getByRole("button", { name: "Create label" }),
    );
    expect(
      within(popover)
        .getByRole("button", { name: "Create label" })
        .getAttribute("aria-busy"),
    ).toBe("true");
    expect(
      await within(popover).findByRole(
        "button",
        { name: "Remove label copy" },
        WAIT,
      ),
    ).toBeTruthy();
    expect(labels.map((l) => l.name)).toContain("copy");
  }, 20_000);

  it("shows an existing label name on the name field", async () => {
    const { dialog } = await openTask();
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Label" }),
    );
    await waitFor(() => {
      expect(screen.getAllByRole("dialog")).toHaveLength(2);
    });
    const popover = screen.getAllByRole("dialog")[1] as HTMLElement;
    await userEvent.type(
      within(popover).getByRole("textbox", { name: "New label name" }),
      "auth",
    );
    await userEvent.click(
      within(popover).getByRole("button", { name: "Create label" }),
    );
    expect(
      await within(popover).findByText(
        "A label with this name exists.",
        {},
        WAIT,
      ),
    ).toBeTruthy();
  }, 15_000);

  it("ticks a plain step only after the daemon confirms it", async () => {
    const { dialog } = await openTask();
    const subtasks = region(dialog, "Subtasks");
    const box = within(subtasks).getByRole("checkbox", { name: STEP });
    expect(box.getAttribute("aria-checked")).toBe("false");
    await userEvent.click(box);
    expect(within(subtasks).getByText("Saving…")).toBeTruthy();
    expect(box.getAttribute("aria-checked")).toBe("false");
    await waitFor(() => {
      expect(
        within(subtasks)
          .getByRole("checkbox", { name: STEP })
          .getAttribute("aria-checked"),
      ).toBe("true");
    }, WAIT);
    expect(
      daemon.made(`PATCH ${P}/demo/checklist-items/ci-1`)[0]?.body,
    ).toEqual({ done: true });
    expect(within(subtasks).getByText("3/3")).toBeTruthy();
  }, 15_000);

  it("gives a linked item no checkbox and a plain step one", async () => {
    const { dialog } = await openTask();
    const subtasks = region(dialog, "Subtasks");
    expect(within(subtasks).getAllByRole("checkbox")).toHaveLength(2);
    const linked = subtasks.querySelector('[data-item="ci-5"]');
    expect(linked).not.toBeNull();
    expect(within(linked as HTMLElement).queryByRole("checkbox")).toBeNull();
    expect(
      within(linked as HTMLElement).getByText("Done · ticked by fold"),
    ).toBeTruthy();
    expect(
      within(linked as HTMLElement)
        .getByRole("link", { name: /T-13/ })
        .getAttribute("href"),
    ).toBe("/p/demo/t/T-13");
  }, 15_000);

  it("adds a plain step with a pending state", async () => {
    const { dialog } = await openTask();
    const subtasks = region(dialog, "Subtasks");
    await userEvent.click(
      within(subtasks).getByRole("button", {
        name: "Add a step or link a task",
      }),
    );
    await userEvent.type(
      within(subtasks).getByRole("textbox", { name: "Step" }),
      "Write the copy",
    );
    await userEvent.click(
      within(subtasks).getByRole("button", { name: "Add step" }),
    );
    expect(
      within(subtasks)
        .getByRole("button", { name: "Add step" })
        .getAttribute("aria-busy"),
    ).toBe("true");
    expect(
      await within(subtasks).findByRole(
        "checkbox",
        { name: "Write the copy" },
        WAIT,
      ),
    ).toBeTruthy();
  }, 15_000);

  it("says a link adds the dependency, and removing it removes the dependency", async () => {
    const { dialog } = await openTask();
    const subtasks = region(dialog, "Subtasks");
    await userEvent.click(
      within(subtasks).getByRole("button", {
        name: "Add a step or link a task",
      }),
    );
    await userEvent.selectOptions(
      within(subtasks).getByRole("combobox", { name: "Kind" }),
      "linked",
    );
    expect(within(subtasks).getByText(/also adds the dependency/)).toBeTruthy();
    await userEvent.click(
      within(subtasks).getByRole("button", { name: "Cancel" }),
    );

    await userEvent.click(
      within(subtasks).getByRole("button", {
        name: "Actions for T-13 Form field components",
      }),
    );
    await userEvent.click(
      await screen.findByRole("menuitem", { name: "Remove" }),
    );
    const confirm = within(subtasks).getByRole("group", {
      name: "Remove the link to T-13",
    });
    expect(
      within(confirm).getByText(
        "Removing the link also removes the dependency: T-2 will no longer need T-13.",
      ),
    ).toBeTruthy();
    await userEvent.click(
      within(confirm).getByRole("button", { name: "Remove link" }),
    );
    await waitFor(() => {
      expect(subtasks.querySelector('[data-item="ci-5"]')).toBeNull();
    }, WAIT);
    const needs = within(region(dialog, "Dependencies")).getByRole("list", {
      name: "Needs",
    });
    expect(within(needs).queryByText("Form field components")).toBeNull();
  }, 15_000);

  it("adds a dependency with a pending state, then removes it", async () => {
    const { dialog } = await openTask();
    const deps = region(dialog, "Dependencies");
    await userEvent.click(
      within(deps).getByRole("button", { name: "Add dependency" }),
    );
    await userEvent.type(
      within(deps).getByRole("textbox", { name: "T-2 needs" }),
      "T-16",
    );
    await userEvent.click(
      within(deps).getByRole("button", { name: "Add dependency" }),
    );
    expect(
      within(deps)
        .getByRole("button", { name: "Add dependency" })
        .getAttribute("aria-busy"),
    ).toBe("true");
    const needs = within(deps).getByRole("list", { name: "Needs" });
    expect(
      await within(needs).findByText("Scaffold the web app", {}, WAIT),
    ).toBeTruthy();
    await userEvent.click(
      within(needs).getByRole("button", {
        name: "Remove the dependency on T-16",
      }),
    );
    await waitFor(() => {
      expect(within(needs).queryByText("Scaffold the web app")).toBeNull();
    }, WAIT);
    expect(
      within(needs).queryByRole("button", {
        name: "Remove the dependency on T-13",
      }),
    ).toBeNull();
  }, 15_000);

  it("shows a cycle refusal as task ids and leaves the dependencies unchanged", async () => {
    const { dialog } = await openTask();
    const deps = region(dialog, "Dependencies");
    const needs = within(deps).getByRole("list", { name: "Needs" });
    const before = needs.textContent;
    await userEvent.click(
      within(deps).getByRole("button", { name: "Add dependency" }),
    );
    await userEvent.type(
      within(deps).getByRole("textbox", { name: "T-2 needs" }),
      "T-5",
    );
    await userEvent.click(
      within(deps).getByRole("button", { name: "Add dependency" }),
    );
    const alert = await within(deps).findByRole("alert", {}, WAIT);
    expect(alert.textContent).toBe(
      "The edge would close a cycle: T-2 → T-5 → T-2.",
    );
    expect(within(deps).getByRole("list", { name: "Needs" }).textContent).toBe(
      before,
    );
  }, 15_000);

  it("shows any other refusal in an error banner with the daemon's message", async () => {
    const { dialog } = await openTask();
    const deps = region(dialog, "Dependencies");
    await userEvent.click(
      within(deps).getByRole("button", { name: "Add dependency" }),
    );
    await userEvent.type(
      within(deps).getByRole("textbox", { name: "T-2 needs" }),
      "T-99",
    );
    await userEvent.click(
      within(deps).getByRole("button", { name: "Add dependency" }),
    );
    const alert = await within(deps).findByRole("alert", {}, WAIT);
    expect(alert.textContent).toContain("The change was refused.");
    expect(alert.textContent).toContain("refused: task_not_found");
  }, 15_000);

  it("shows no dependency editor on a Done task", async () => {
    const { dialog } = await openTask("T-13", "Form field components");
    const deps = region(dialog, "Dependencies");
    expect(
      within(deps).queryByRole("button", { name: "Add dependency" }),
    ).toBeNull();
    expect(within(dialog).getByText("Done", { selector: "span" })).toBeTruthy();
  }, 15_000);

  it("shows the human-review mark and offers no control for it", async () => {
    patch("T-2", { human_review: { reviewer: "tanim" } });
    const { dialog } = await openTask();
    const review = region(dialog, "Human review");
    expect(within(review).getByText("Required · tanim")).toBeTruthy();
    expect(within(review).queryAllByRole("button")).toHaveLength(0);
  }, 15_000);

  it("says human review is not required when the task has no mark", async () => {
    const { dialog } = await openTask();
    const review = region(dialog, "Human review");
    expect(within(review).getByText("Not required")).toBeTruthy();
    expect(within(review).queryAllByRole("button")).toHaveLength(0);
  }, 15_000);

  it("sends feedback to the claim holder with a pending state", async () => {
    const { dialog } = await openTask();
    const messages = region(dialog, "Messages");
    const box = within(messages).getByRole("textbox", { name: "Feedback" });
    expect(box.getAttribute("placeholder")).toBe(
      "Write feedback to frontend/ui…",
    );
    await userEvent.type(box, "Use the shorter copy");
    await userEvent.click(
      within(messages).getByRole("button", { name: "Send" }),
    );
    expect(
      within(messages)
        .getByRole("button", { name: "Send" })
        .getAttribute("aria-busy"),
    ).toBe("true");
    await waitFor(() => {
      expect(daemon.made(`POST ${P}/demo/feedback`)).toHaveLength(1);
    }, WAIT);
    expect(daemon.made(`POST ${P}/demo/feedback`)[0]?.body).toEqual({
      task: "T-2",
      body: "Use the shorter copy",
    });
    // The write ends once the message list has been read again, so the sent message is in it.
    await waitFor(() => {
      expect(within(messages).getByText("1 linked to T-2")).toBeTruthy();
    }, WAIT);
    expect(within(messages).getByText("Use the shorter copy")).toBeTruthy();
  }, 15_000);

  it("lets the feedback box address an agent from the agent list", async () => {
    const { dialog } = await openTask();
    const messages = region(dialog, "Messages");
    await waitFor(() => {
      expect(
        within(messages).getByRole("option", { name: "frontend/ui" }),
      ).toBeTruthy();
    }, WAIT);
  }, 15_000);
});

describe("archiving from the dialog head", () => {
  async function confirmArchive() {
    const { dialog, router } = await openTask();
    await userEvent.click(
      within(dialog).getByRole("button", { name: "More actions" }),
    );
    await userEvent.click(
      await screen.findByRole("menuitem", { name: "Archive" }),
    );
    const confirm = await screen.findByRole("dialog", { name: "Archive T-2?" });
    return { confirm, router };
  }

  it("asks first, stays pending, then closes onto the board", async () => {
    const { confirm, router } = await confirmArchive();
    expect(daemon.made(`POST ${P}/demo/tasks/T-2/archive`)).toHaveLength(0);
    await userEvent.click(
      within(confirm).getByRole("button", { name: "Archive task" }),
    );
    expect(
      within(confirm)
        .getByRole("button", { name: "Archive task" })
        .getAttribute("aria-busy"),
    ).toBe("true");
    await waitFor(() => {
      expect(router.state.location.pathname).toBe("/p/demo/board");
    }, WAIT);
    expect(daemon.made(`POST ${P}/demo/tasks/T-2/archive`)).toHaveLength(1);
  }, 15_000);

  it("shows the open-proposal refusal by its reason", async () => {
    archiveRefusal = true;
    const { confirm } = await confirmArchive();
    await userEvent.click(
      within(confirm).getByRole("button", { name: "Archive task" }),
    );
    const alert = await within(confirm).findByRole("alert", {}, WAIT);
    expect(alert.textContent).toBe(
      "A proposal is open for this task. Close it before you archive the task.",
    );
  }, 15_000);

  it("leaves the task alone when the human cancels", async () => {
    const { confirm } = await confirmArchive();
    await userEvent.click(
      within(confirm).getByRole("button", { name: "Cancel" }),
    );
    await waitFor(() => {
      expect(screen.queryByRole("dialog", { name: "Archive T-2?" })).toBeNull();
    });
    expect(daemon.made(`POST ${P}/demo/tasks/T-2/archive`)).toHaveLength(0);
  }, 15_000);
});

describe("Markdown", () => {
  it("renders tables and task lists and drops raw HTML", () => {
    const { container } = render(
      <Markdown>
        {[
          "| Code | Message |",
          "| --- | --- |",
          "| `rate_limited` | Try later. |",
          "",
          "- [x] Agree the copy",
          "- [ ] Check it",
          "",
          "Some <b>bold</b> words",
          "",
          "<script>alert(1)</script>",
          "",
          '<div class="x">block</div>',
          "",
          "[run](javascript:alert(2)) and [board](/p/demo/board)",
        ].join("\n")}
      </Markdown>,
    );
    expect(screen.getByRole("table")).toBeTruthy();
    expect(
      screen.getAllByRole("columnheader").map((h) => h.textContent),
    ).toEqual(["Code", "Message"]);
    const boxes = screen.getAllByRole<HTMLInputElement>("checkbox");
    expect(boxes.map((b) => b.checked)).toEqual([true, false]);
    expect(boxes.every((b) => b.disabled)).toBe(true);
    expect(container.querySelector("b, script, div.x")).toBeNull();
    expect(container.textContent).not.toContain("<b>");
    expect(container.textContent).not.toContain("alert(1)");
    expect(container.textContent).not.toContain("block");
    expect(
      [...container.querySelectorAll("a")].map((a) => a.getAttribute("href")),
    ).toEqual(["", "/p/demo/board"]);
  });
});
