// @vitest-environment jsdom
import { byContent } from "@/test/content";
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
import { keys } from "@/api/keys";
import type { components } from "@/api/schema";
import type { TaskDetail } from "@/api/tasks";
import { applyEvent } from "@/live/apply";
import { liveStatusStore } from "@/live/status";
import { agent, worker } from "@/test/agents";
import { requestDetail, requestSummary } from "@/test/requests";
import { areaEntry, workerList } from "@/test/workers";
import { daemon, noContent, ok, refuse, type Call } from "@/test/daemon";
import { expectMentionPopover } from "@/test/mentionBox";
import { mentionRoutes } from "@/test/mentionRoutes";
import { boardCard, boardView, liveWorkspace, taskView } from "@/test/payloads";
import { routeTree } from "@/routeTree.gen";
import { ThemeProvider } from "@/theme/ThemeProvider";
import { Markdown } from "./Markdown";

vi.mock("@/api/client", async (importOriginal) => {
  const real = await importOriginal<typeof import("@/api/client")>();
  const { daemon: fake } = await import("@/test/daemon");
  return { ...real, api: fake.api };
});

// The event stream stays open and silent; a test that needs live workspace state applies the frame the daemon would send.
vi.mock("@/api/events", async (importOriginal) => {
  const real = await importOriginal<typeof import("@/api/events")>();
  return {
    ...real,
    openEventStream: () => (_lastSeq: number | null, signal: AbortSignal) =>
      Promise.resolve({
        async *[Symbol.asyncIterator]() {
          await new Promise((resolve) => {
            signal.addEventListener("abort", resolve);
          });
          yield* [];
        },
      }),
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
const server = { name: "server", color: "gold" };
const main = { name: "main", color: "ink" };
/** `ui` and `server` are area labels, `main` is the label of the built-in area Main, and `auth` is a plain label. */
const AREAS = [
  areaEntry("api", "server"),
  areaEntry("web", "ui", { lead: "web-lead" }),
];
const STEP = "Map invalid_credentials and rate_limited to messages";
const DESIGN = "Update the board screens in Pencil";
const REPORT = "Drew the five board screens.";

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
  labels = [auth, main, server, ui];
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
    [
      "T-7",
      taskView("T-7", {
        title: DESIGN,
        kind: "no_proposal",
        status: "review",
        claims: [
          {
            kind: "implementation",
            holder: "design-worker",
            claimed_at: 1_000,
            renewed_at: 2_000,
            stale: false,
          },
        ],
        reports: [
          {
            by: "design-worker",
            note: REPORT,
            at: 2_000,
            resolution: null,
            resolved_by: null,
            resolved_at: null,
            send_back_note: null,
          },
        ],
      }),
    ],
  ]);
}

/** The lead check the fake daemon writes for T-7: the first report answered `resolution`, and the status that follows. */
function check(
  resolution: "completed" | "sent_back",
  note: string | null,
): TaskDetail {
  const task = view("T-7");
  const [first, ...rest] = task.reports;
  if (!first) throw new Error("T-7 has no report");
  return patch("T-7", {
    status: resolution === "completed" ? "done" : "in_progress",
    claims: resolution === "completed" ? [] : task.claims,
    reports: [
      {
        ...first,
        resolution,
        resolved_by: "human:tanim",
        resolved_at: 3_000,
        send_back_note: note,
      },
      ...rest,
    ],
  });
}

function cardOf(task: TaskDetail): BoardCard {
  return boardCard(task.task, {
    title: task.title,
    status: task.status,
    blocked: task.blocked,
    labels: task.labels,
    worker: task.claims[0]?.holder ?? null,
    workspace: task.workspace,
    kind: task.kind,
    report:
      task.kind === "no_proposal" && task.status === "review"
        ? (task.reports[0]?.note ?? null)
        : null,
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
    const body = call.body as {
      title: string;
      description?: string;
      labels?: string[];
    };
    const id = `T-${String(nextTask++)}`;
    tasks.set(
      id,
      taskView(id, {
        title: body.title,
        description: body.description ?? "",
        labels: labels.filter((l) => body.labels?.includes(l.name)),
      }),
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
  daemon.on(`PUT ${P}/:slug/tasks/:id/labels/:name`, (call) => {
    const id = call.params.id ?? "";
    const found = labels.find((l) => l.name === call.params.name);
    const current = view(id).labels;
    if (!found) return refuse(404, "label_not_found");
    return ok(
      patch(id, {
        labels: current.some((l) => l.name === found.name)
          ? current
          : [...current, found],
      }),
    );
  });
  daemon.on(`DELETE ${P}/:slug/tasks/:id/labels/:name`, (call) => {
    const id = call.params.id ?? "";
    return ok(
      patch(id, {
        labels: view(id).labels.filter((l) => l.name !== call.params.name),
      }),
    );
  });
  daemon.on(`GET ${P}/:slug/labels`, () => ok({ labels }));
  daemon.on(`GET ${P}/:slug/workers`, () => ok(workerList({ areas: AREAS })));
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
  daemon.on(`GET ${P}/:slug/tasks/:id/readers`, (call) => {
    const task = view(call.params.id ?? "");
    const holder = task.claims[0]?.holder;
    const lead = AREAS.find((area) =>
      task.labels.some((label) => label.name === area.label),
    )?.lead;
    return ok({
      readers: [
        ...(holder ? [{ name: holder, role: "claim_holder" }] : []),
        ...(lead ? [{ name: lead, role: "area_lead" }] : []),
        { name: "main", role: "main_agent" },
      ],
    });
  });
  // The daemon keeps the threads the signed-in human takes part in; every message of `linked` is in one, and a thread waits while one of its messages asks the human.
  daemon.on(`GET ${P}/:slug/messages`, (call) => {
    const mine =
      call.path.includes("task=T-2") && call.path.includes("mine=true");
    const threads = [...new Set(linked.map((m) => m.thread))].map((id) => {
      const messages = linked.filter((m) => m.thread === id);
      return {
        id,
        awaiting_human: messages.some((m) => m.asks_human),
        closable: false,
        last_at: messages.at(-1)?.sent_at ?? 0,
        messages,
        participants: messages.map((m) => m.sender),
        proposal: null,
        request: null,
        task: "T-2",
      };
    });
    return ok({
      items: mine ? threads.reverse() : [],
      next: null,
      total_awaiting_human: 0,
    });
  });
  daemon.on(`POST ${P}/:slug/feedback`, (call) => {
    const body = call.body as { body: string };
    linked = [
      ...linked,
      {
        id: messageId(90 + linked.length),
        thread: messageId(90 + linked.length),
        sender: "human:tanim",
        choices: [],
        header: null,
        asks_human: false,
        lines: null,
        recipients: [],
        body: body.body,
        proposal: null,
        request: null,
        reply_to: null,
        task: "T-2",
        sent_at: 5_000,
      },
    ];
    return ok({}, 201);
  });
  daemon.on(`POST ${P}/:slug/tasks/:id/complete`, () =>
    ok(check("completed", null)),
  );
  daemon.on(`POST ${P}/:slug/tasks/:id/send-back`, (call) => {
    const { note } = call.body as { note: string };
    return ok(check("sent_back", note));
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

/** The Messages section opens on a click on its head. */
async function openMessages(dialog: HTMLElement): Promise<HTMLElement> {
  const messages = region(dialog, "Messages");
  await userEvent.click(
    within(messages).getByRole("button", { name: "Messages" }),
  );
  return messages;
}

/** Opens the task once the side column has read the areas, which decide its Area and Labels groups. */
async function openLabels(task = "T-2", title = TITLE) {
  const opened = await openTask(task, title);
  await within(opened.dialog).findByRole("region", { name: "Labels" }, WAIT);
  return opened;
}

describe("the task dialog route", () => {
  it("opens from a board card with the board's filters and shows the sections the task payload has data for", async () => {
    const { router, queryClient } = setup("/p/demo/board?label=ui");
    const link = await screen.findByRole("link", { name: TITLE }, WAIT);
    expect(link.getAttribute("href")).toBe("/p/demo/t/T-2?label=ui");
    await userEvent.click(link);
    const dialog = await screen.findByRole("dialog", {}, WAIT);
    await within(dialog).findByRole("heading", { name: TITLE }, WAIT);
    expect(router.state.location.pathname).toBe("/p/demo/t/T-2");
    expect(router.state.location.search).toEqual({ label: "ui" });

    expect(within(dialog).getByText("In progress")).toBeTruthy();
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
        "0 threads",
        {},
        WAIT,
      ),
    ).toBeTruthy();
    expect(within(region(dialog, "Workspace")).getByText("ws-2")).toBeTruthy();

    // The event stream reports the workspace; the task payload names only its id.
    applyEvent(queryClient, "demo", {
      kind: "workspace_changed",
      ...liveWorkspace("ws-2", "T-2", {
        files: [
          {
            path: "src/routes/login.tsx",
            previous_path: null,
            change: "modified",
            added_lines: 3,
            removed_lines: 1,
            binary: false,
          },
        ],
      }),
    });
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
        choices: [],
        header: null,
        asks_human: false,
        lines: null,
        recipients: [
          {
            name: "review",
            addressed_as: "agent",
            delivered_at: null,
            acknowledged_at: null,
            pushed_at: null,
            reachable: null,
            forwarded_to: null,
          },
        ],
        body: "Ready for review",
        proposal: null,
        request: null,
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
        title: "Add the form",
        review_claim: null,
        files: 3,
        state: "open",
        version: 2,
        checks_failed: false,
        reviews: [{ reviewer: "review", verdict: "approve", version: 2, at }],
      },
      integration: {
        revision: "r9",
        proposal: "P-2",
        by: "backend",
        at,
        commit: null,
      },
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
      human_review: {
        reviewer: "tanim",
        reason: "Auth work",
        set_by: "planner",
      },
      human_review_set_at: at,
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
    expect(
      within(proposal).getByText("approved on v2", { exact: false }),
    ).toBeTruthy();
    expect(
      within(proposal).getByRole("link", { name: "r9" }).getAttribute("href"),
    ).toBe("/p/demo/repository?revision=r9");
    const messages = await openMessages(dialog);
    expect(
      await within(messages).findByText("1 thread", {}, WAIT),
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
    expect(
      within(history).getByText(byContent(/Claimed by @frontend\/ui/)),
    ).toBeTruthy();
    expect(within(history).queryByText(/Edited by/)).toBeNull();
    await userEvent.click(
      within(history).getByRole("button", { name: "Show 1 more" }),
    );
    expect(
      within(history).getByText(byContent(/Edited by @tanim/)),
    ).toBeTruthy();
    const review = region(dialog, "Human review");
    expect(within(review).getByText(/^Required ·/).textContent).toBe(
      "Required ·@tanim",
    );
    expect(within(review).getByText(/^Set by/).textContent).toBe(
      "Set by @planner · 13:40",
    );
    expect(within(review).getByText("Auth work")).toBeTruthy();
  }, 20_000);

  it("lists the checkpoints of a task whose workspace is not live", async () => {
    patch("T-2", {
      workspace: null,
      checkpoints: [
        {
          id: "ck-7",
          label: "before the merge",
          automatic: false,
          created_at: 5_000,
        },
      ],
    });
    const { dialog } = await openTask();
    const checkpoints = region(dialog, "Checkpoints");
    expect(within(checkpoints).getByText("ck-7")).toBeTruthy();
    expect(within(checkpoints).queryByText("No checkpoints yet.")).toBeNull();
  });

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

  it("shows the next task's own title in a closed editor after following a link", async () => {
    const { dialog } = await openTask();
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Edit the title" }),
    );
    expect(
      await within(dialog).findByRole("button", { name: "Save title" }),
    ).toBeTruthy();
    await userEvent.click(
      within(region(dialog, "Dependencies")).getByRole("link", {
        name: /Session cookie/,
      }),
    );
    // The keyed dialog is a new element, so the next task is read from the screen.
    const next = await screen.findByRole("dialog", {}, WAIT);
    expect(
      await within(next).findByRole(
        "heading",
        { name: "Session cookie" },
        WAIT,
      ),
    ).toBeTruthy();
    expect(
      within(next).queryByRole("button", { name: "Save title" }),
    ).toBeNull();
  }, 15_000);

  it("says so when the clipboard refuses the link, and resets the button on blur", async () => {
    const writeText = vi.fn().mockRejectedValue(new Error("refused"));
    Object.defineProperty(navigator, "clipboard", {
      value: { writeText },
      configurable: true,
    });
    const { dialog } = await openTask();
    const copy = within(dialog).getByRole("button", { name: "Copy link" });
    await userEvent.click(copy);
    expect(
      await within(dialog).findByRole("button", { name: "Not copied" }),
    ).toBeTruthy();
    await userEvent.tab();
    expect(
      await within(dialog).findByRole("button", { name: "Copy link" }),
    ).toBeTruthy();
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

  it("names its two columns, and each one takes keyboard focus so the keys scroll it", async () => {
    const { dialog } = await openTask();
    expect(region(dialog, "Task").tabIndex).toBe(0);
    expect(
      within(dialog).getByRole("complementary", { name: "Task details" })
        .tabIndex,
    ).toBe(0);
  }, 15_000);

  it("keeps the task on screen when a read again fails, with a line that reads it again on Retry", async () => {
    const { dialog, queryClient } = await openTask();
    const column = region(dialog, "Task");
    let failing = true;
    daemon.on(`GET ${P}/:slug/tasks/:id`, (call) =>
      failing ? refuse(500, "internal_error") : ok(view(call.params.id ?? "")),
    );
    await queryClient.refetchQueries({ queryKey: keys.task("demo", "T-2") });

    const alert = await within(dialog).findByRole("alert", {}, WAIT);
    expect(alert.textContent).toContain("The task could not be read again.");
    expect(alert.textContent).toContain("The daemon failed. Try again.");
    // The same column stays mounted, so the reader keeps the scroll position.
    expect(region(dialog, "Task")).toBe(column);
    expect(within(dialog).getByRole("heading", { name: TITLE })).toBeTruthy();
    expect(
      within(dialog).getByRole("button", { name: "Copy link" }),
    ).toBeTruthy();

    failing = false;
    await userEvent.click(within(alert).getByRole("button", { name: "Retry" }));
    await waitFor(() => {
      expect(
        within(dialog).queryByText("The task could not be read again."),
      ).toBeNull();
    }, WAIT);
    expect(region(dialog, "Task")).toBe(column);
  }, 15_000);
});

describe("creating a task", () => {
  it("opens the create form from the top bar and opens the created task", async () => {
    const { router } = setup("/p/demo/board?label=ui");
    const [bar] = await screen.findAllByRole("banner", {}, WAIT);
    const link = within(bar ?? document.body).getByRole("link", {
      name: "New task",
    });
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
        .getByRole("button", { name: "Creating task…" })
        .getAttribute("aria-busy"),
    ).toBe("true");
    expect(
      await screen.findByRole("heading", { name: "Password reset" }, WAIT),
    ).toBeTruthy();
    expect(router.state.location.pathname).toBe("/p/demo/t/T-17");
  }, 15_000);

  it("suggests items and files in the new task description", async () => {
    mentionRoutes();
    setup("/p/demo/t/new");
    const dialog = await screen.findByRole("dialog", {}, WAIT);
    await expectMentionPopover(
      within(dialog).getByRole("combobox", { name: "Description" }),
    );
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

  it("creates a task with one area and other labels", async () => {
    setup("/p/demo/t/new");
    const dialog = await screen.findByRole("dialog", {}, WAIT);
    expect(
      within(dialog).getByText(
        "fold calculates its status. Add checklists and dependencies once it exists.",
      ),
    ).toBeTruthy();
    await userEvent.type(
      within(dialog).getByRole("textbox", { name: "Title" }),
      "Show the export date",
    );
    const area = await within(dialog).findByRole(
      "button",
      { name: "Area Decide automatically" },
      WAIT,
    );
    await waitFor(() => {
      expect(area.hasAttribute("disabled")).toBe(false);
    }, WAIT);
    await userEvent.click(area);
    await userEvent.click(
      await screen.findByRole("menuitemradio", { name: /web-lead/ }, WAIT),
    );
    expect(
      within(dialog).getByRole("button", { name: /^Area ui/ }),
    ).toBeTruthy();

    const box = within(dialog).getByRole("combobox", { name: "Labels" });
    await userEvent.click(box);
    const list = await screen.findByRole("listbox", { name: "Labels" }, WAIT);
    // Only labels that are no area's label are offered.
    expect(
      within(list)
        .getAllByRole("option")
        .map((o) => o.textContent),
    ).toEqual(["auth"]);
    await userEvent.type(box, "zz");
    expect(await screen.findByText("No label matches.", {}, WAIT)).toBeTruthy();
    await userEvent.clear(box);
    await userEvent.type(box, "AU{ArrowDown}{Enter}");
    expect(
      within(dialog).getByRole("button", { name: "Remove auth" }),
    ).toBeTruthy();
    expect(
      await screen.findByText("No more labels that are not areas.", {}, WAIT),
    ).toBeTruthy();
    await userEvent.type(box, "{Backspace}");
    expect(
      within(dialog).queryByRole("button", { name: "Remove auth" }),
    ).toBeNull();
    await userEvent.click(
      within(
        await screen.findByRole("listbox", { name: "Labels" }, WAIT),
      ).getByRole("option", { name: "auth" }),
    );

    await userEvent.click(
      within(dialog).getByRole("button", { name: "Create task" }),
    );
    await screen.findByRole("heading", { name: "Show the export date" }, WAIT);
    expect(daemon.made(`POST ${P}/demo/tasks`)[0]?.body).toEqual({
      title: "Show the export date",
      description: "",
      labels: ["ui", "auth"],
    });
  }, 20_000);

  it("points to a request and opens the request it makes", async () => {
    // jsdom's FormData does not reach the runtime's multipart encoder, so the request uses the runtime's own class.
    const probe = await new Response(new URLSearchParams("a=b")).formData();
    vi.stubGlobal("FormData", probe.constructor);
    daemon.on(`POST ${P}/:slug/requests`, () =>
      ok(requestDetail(requestSummary("RQ-8")), 201),
    );
    const { router } = setup("/p/demo/t/new");
    const dialog = await screen.findByRole("dialog", {}, WAIT);
    expect(
      within(dialog).getByText("Usually a request works better"),
    ).toBeTruthy();
    await userEvent.click(
      within(dialog).getByRole("button", { name: "New request" }),
    );
    const request = await screen.findByRole(
      "dialog",
      { name: "New request" },
      WAIT,
    );
    await userEvent.type(
      within(request).getByLabelText("What do you need?"),
      "Export the weekly report",
    );
    await userEvent.click(
      within(request).getByRole("button", { name: "Create request" }),
    );
    await waitFor(() => {
      expect(router.state.location.pathname).toBe("/p/demo/requests/RQ-8");
    }, WAIT);
    expect(daemon.made(`POST ${P}/demo/tasks`)).toHaveLength(0);
    vi.unstubAllGlobals();
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
    const save = within(dialog).getByRole("button", {
      name: "Saving title…",
    });
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
    const field = within(section).getByRole("combobox", {
      name: "Description",
    });
    await userEvent.clear(field);
    await userEvent.type(field, "Only **this** now");
    await userEvent.click(
      within(section).getByRole("button", { name: "Save description" }),
    );
    expect(
      within(section)
        .getByRole("button", { name: "Saving description…" })
        .getAttribute("aria-busy"),
    ).toBe("true");
    expect(
      await within(section).findByText("this", { selector: "strong" }, WAIT),
    ).toBeTruthy();
  }, 15_000);

  it("suggests items and files in the description editor and links them once saved", async () => {
    mentionRoutes();
    const { dialog } = await openTask();
    const section = region(dialog, "Description");
    await userEvent.click(
      within(section).getByRole("button", { name: "Edit the description" }),
    );
    const field = within(section).getByRole("combobox", {
      name: "Description",
    });
    await userEvent.clear(field);
    await expectMentionPopover(field);
    await userEvent.type(field, "report.tsx");
    await userEvent.click(
      within(section).getByRole("button", { name: "Save description" }),
    );
    const link = await within(section).findByRole(
      "link",
      { name: "T-9" },
      WAIT,
    );
    expect(link.getAttribute("href")).toBe("/p/demo/t/T-9");
    expect(
      within(section)
        .getByRole("link", { name: "web/report.tsx" })
        .getAttribute("href"),
    ).toBe("/p/demo/repository?path=web%2Freport.tsx");
  }, 15_000);

  it("shows the area with its lead and changes it in one write with a pending state", async () => {
    const { dialog } = await openLabels();
    const area = region(dialog, "Area");
    expect(within(area).getByText("ui")).toBeTruthy();
    expect(within(area).getByText("lead: web-lead")).toBeTruthy();
    expect(within(region(dialog, "Labels")).queryByText("ui")).toBeNull();
    await userEvent.click(within(area).getByRole("button", { name: "Change" }));
    const menu = await screen.findByRole("menu", {}, WAIT);
    expect(
      within(menu)
        .getAllByRole("menuitemradio")
        .map((item) => item.textContent),
    ).toEqual([
      "Decide automatically",
      "mainMain agent",
      "serverno lead yet",
      "uiweb-lead",
    ]);
    expect(
      within(menu)
        .getByRole("menuitemradio", { name: /web-lead/ })
        .getAttribute("aria-checked"),
    ).toBe("true");
    await userEvent.click(
      within(menu).getByRole("menuitemradio", { name: /server/ }),
    );
    expect(
      within(area)
        .getByRole("button", { name: "Changing area…" })
        .getAttribute("aria-busy"),
    ).toBe("true");
    expect(
      await within(area).findByText("lead: no lead yet", {}, WAIT),
    ).toBeTruthy();
    expect(daemon.made(`PATCH ${P}/demo/tasks/T-2`)[0]?.body).toEqual({
      labels: ["server"],
    });
  }, 15_000);

  it("says a task without an area waits for the main agent, and gives it one", async () => {
    const { dialog } = await openLabels("T-1", "Session cookie");
    const area = region(dialog, "Area");
    expect(
      within(area).getByText(
        "No area yet: the main agent chooses one. Pick an area to give it to that lead at once.",
      ),
    ).toBeTruthy();
    await userEvent.click(within(area).getByRole("button", { name: "Change" }));
    const menu = await screen.findByRole("menu", {}, WAIT);
    // The marked choice changes nothing, so it sends no write.
    await userEvent.click(
      within(menu).getByRole("menuitemradio", {
        name: "Decide automatically",
      }),
    );
    expect(daemon.made(`PATCH ${P}/demo/tasks/T-1`)).toHaveLength(0);
    await userEvent.click(within(area).getByRole("button", { name: "Change" }));
    await userEvent.click(
      await screen.findByRole("menuitemradio", { name: /web-lead/ }, WAIT),
    );
    expect(
      await within(area).findByText("lead: web-lead", {}, WAIT),
    ).toBeTruthy();
    expect(daemon.made(`PATCH ${P}/demo/tasks/T-1`)[0]?.body).toEqual({
      labels: ["ui"],
    });
  }, 15_000);

  it("gives a task to the main agent, and shows the main agent instead of the no-area warning", async () => {
    const { dialog } = await openLabels("T-1", "Session cookie");
    const area = region(dialog, "Area");
    await userEvent.click(within(area).getByRole("button", { name: "Change" }));
    await userEvent.click(
      await screen.findByRole("menuitemradio", { name: /Main agent/ }, WAIT),
    );
    expect(await within(area).findByText("Main agent", {}, WAIT)).toBeTruthy();
    expect(within(area).queryByText(/No area yet/)).toBeNull();
    expect(daemon.made(`PATCH ${P}/demo/tasks/T-1`)[0]?.body).toEqual({
      labels: ["main"],
    });
  }, 15_000);

  it("warns about a task with two areas and keeps the one the human picks", async () => {
    tasks.set(
      "T-3",
      taskView("T-3", {
        title: "Shared login copy",
        labels: [auth, server, ui],
        areas_conflict: true,
      }),
    );
    const { dialog } = await openLabels("T-3", "Shared login copy");
    const area = region(dialog, "Area");
    expect(within(area).getByRole("note").textContent).toBe(
      "This task has the labels of two areas, but a task belongs to one. Use Change to keep one area; fold refuses other label changes until then.",
    );
    expect(within(area).getByText("lead: no lead yet")).toBeTruthy();
    expect(within(area).getByText("lead: web-lead")).toBeTruthy();
    // The daemon refuses every other label change until one area is left.
    const group = region(dialog, "Labels");
    const labelButtons = () =>
      ["Remove auth", "Add label"].map(
        (name) =>
          within(group).getByRole<HTMLButtonElement>("button", { name })
            .disabled,
      );
    expect(labelButtons()).toEqual([true, true]);
    await userEvent.click(within(area).getByRole("button", { name: "Change" }));
    const menu = await screen.findByRole("menu", {}, WAIT);
    // A task with two areas marks neither, so either pick writes.
    expect(
      within(menu)
        .getAllByRole("menuitemradio")
        .map((item) => item.getAttribute("aria-checked")),
    ).toEqual(["false", "false", "false", "false"]);
    daemon.on(`PATCH ${P}/:slug/tasks/:id`, (call) => {
      const body = call.body as { labels: string[] };
      return ok(
        patch("T-3", {
          labels: labels.filter((l) => body.labels.includes(l.name)),
          areas_conflict: false,
        }),
      );
    });
    await userEvent.click(
      within(menu).getByRole("menuitemradio", { name: /web-lead/ }),
    );
    await waitFor(() => {
      expect(within(area).queryByRole("note")).toBeNull();
    }, WAIT);
    expect(daemon.made(`PATCH ${P}/demo/tasks/T-3`)[0]?.body).toEqual({
      labels: ["auth", "ui"],
    });
    expect(within(area).queryByText("lead: no lead yet")).toBeNull();
    expect(labelButtons()).toEqual([false, false]);
  }, 15_000);

  it("holds the label controls while the area write runs, so it never undoes a label change", async () => {
    tasks.set(
      "T-3",
      taskView("T-3", { title: "Shared login copy", labels: [auth, ui] }),
    );
    let release = () => {};
    const held = new Promise<void>((resolve) => {
      release = resolve;
    });
    daemon.on(`PATCH ${P}/:slug/tasks/:id`, async (call) => {
      await held;
      const body = call.body as { labels: string[] };
      return ok(
        patch("T-3", {
          labels: labels.filter((l) => body.labels.includes(l.name)),
        }),
      );
    });
    const { dialog } = await openLabels("T-3", "Shared login copy");
    const area = region(dialog, "Area");
    const group = region(dialog, "Labels");
    const remove = () =>
      within(group).getByRole<HTMLButtonElement>("button", {
        name: "Remove auth",
      });
    const add = () =>
      within(group).getByRole<HTMLButtonElement>("button", {
        name: "Add label",
      });
    await userEvent.click(within(area).getByRole("button", { name: "Change" }));
    await userEvent.click(
      await screen.findByRole("menuitemradio", { name: /server/ }, WAIT),
    );
    expect(
      within(area)
        .getByRole("button", { name: "Changing area…" })
        .getAttribute("aria-busy"),
    ).toBe("true");
    expect([remove().disabled, add().disabled]).toEqual([true, true]);
    release();
    expect(
      await within(area).findByText("lead: no lead yet", {}, WAIT),
    ).toBeTruthy();
    await waitFor(() => {
      expect([remove().disabled, add().disabled]).toEqual([false, false]);
    }, WAIT);
    expect(daemon.made(`PATCH ${P}/demo/tasks/T-3`)[0]?.body).toEqual({
      labels: ["auth", "server"],
    });
  }, 15_000);

  it("says when the areas could not load, and reads them again on Retry", async () => {
    daemon.on(`GET ${P}/:slug/workers`, refuse(500, "internal"));
    const { dialog } = await openTask();
    const area = await within(dialog).findByRole(
      "region",
      { name: "Area" },
      WAIT,
    );
    expect(
      (await within(area).findByRole("alert", {}, WAIT)).textContent,
    ).toContain("The areas could not load.");
    // Without the area list no label is known to be an area, so the Labels group waits.
    expect(within(dialog).queryByRole("region", { name: "Labels" })).toBeNull();
    daemon.on(`GET ${P}/:slug/workers`, () => ok(workerList({ areas: AREAS })));
    await userEvent.click(within(area).getByRole("button", { name: "Retry" }));
    expect(
      await within(dialog).findByText("lead: web-lead", {}, WAIT),
    ).toBeTruthy();
    expect(region(dialog, "Labels")).toBeTruthy();
  }, 15_000);

  it("names the one-area rule when the daemon refuses a second area", async () => {
    daemon.on(
      `PATCH ${P}/:slug/tasks/:id`,
      refuse(400, "invalid_request", {
        reason: "two_areas",
        areas: ["api", "web"],
      }),
    );
    const { dialog } = await openLabels();
    const area = region(dialog, "Area");
    await userEvent.click(within(area).getByRole("button", { name: "Change" }));
    await userEvent.click(
      await screen.findByRole("menuitemradio", { name: /server/ }, WAIT),
    );
    expect(
      await within(area).findByText(
        "A task can have only one area label. Keep one area, or let the main agent choose it.",
        {},
        WAIT,
      ),
    ).toBeTruthy();
    expect(within(area).getByText("lead: web-lead")).toBeTruthy();
  }, 15_000);

  it("puts a label on the task, takes it off, and creates a new one", async () => {
    const { dialog } = await openLabels();
    const group = region(dialog, "Labels");
    await userEvent.click(
      within(group).getByRole("button", { name: "Add label" }),
    );
    await waitFor(() => {
      expect(screen.getAllByRole("dialog")).toHaveLength(2);
    });
    const popover = screen.getAllByRole("dialog")[1] as HTMLElement;
    // Area labels change only through the area.
    expect(
      within(popover)
        .getAllByRole("button", { name: /^Add label/ })
        .map((b) => b.getAttribute("aria-label")),
    ).toEqual(["Add label auth"]);
    await userEvent.click(
      within(popover).getByRole("button", { name: "Add label auth" }),
    );
    const remove = await within(group).findByRole(
      "button",
      { name: "Remove auth" },
      WAIT,
    );
    await userEvent.click(remove);
    expect(
      within(group)
        .getByRole("button", { name: "Removing auth…" })
        .getAttribute("aria-busy"),
    ).toBe("true");
    await waitFor(() => {
      expect(within(group).queryByRole("button", { name: /auth/ })).toBeNull();
    }, WAIT);

    // The remove click outside closed the picker.
    await userEvent.click(
      within(group).getByRole("button", { name: "Add label" }),
    );
    await waitFor(() => {
      expect(screen.getAllByRole("dialog")).toHaveLength(2);
    });
    const picker = screen.getAllByRole("dialog")[1] as HTMLElement;
    await userEvent.type(
      within(picker).getByRole("textbox", { name: "New label name" }),
      "copy",
    );
    await userEvent.selectOptions(
      within(picker).getByRole("combobox", { name: "Colour" }),
      "gold",
    );
    await userEvent.click(
      within(picker).getByRole("button", { name: "Create label" }),
    );
    expect(
      within(picker)
        .getByRole("button", { name: "Creating label…" })
        .getAttribute("aria-busy"),
    ).toBe("true");
    expect(
      await within(group).findByRole("button", { name: "Remove copy" }, WAIT),
    ).toBeTruthy();
    expect(labels.map((l) => l.name)).toContain("copy");
  }, 20_000);

  it("shows an existing label name on the name field", async () => {
    const { dialog } = await openLabels();
    await userEvent.click(
      within(region(dialog, "Labels")).getByRole("button", {
        name: "Add label",
      }),
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
        .getByRole("button", { name: "Adding step…" })
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
        .getByRole("button", { name: "Adding dependency…" })
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
    patch("T-2", {
      human_review: { reviewer: "tanim", reason: "auth", set_by: "planner" },
    });
    const { dialog } = await openTask();
    const review = region(dialog, "Human review");
    expect(within(review).getByText(/^Required ·/).textContent).toBe(
      "Required ·@tanim",
    );
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
    const messages = await openMessages(dialog);
    const box = within(messages).getByRole("combobox", { name: "Feedback" });
    // The readers arrive after the box, and the claim holder is the default.
    await waitFor(() => {
      expect(box.getAttribute("placeholder")).toBe(
        "Write feedback to @frontend/ui…",
      );
    }, WAIT);
    await userEvent.type(box, "Use the shorter copy");
    await userEvent.click(
      within(messages).getByRole("button", { name: "Send" }),
    );
    expect(
      within(messages)
        .getByRole("button", { name: "Sending…" })
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
      expect(within(messages).getByText("1 thread")).toBeTruthy();
    }, WAIT);
    expect(
      within(messages).getByText("Use the shorter copy", { selector: "p" }),
    ).toBeTruthy();
  }, 15_000);

  it("suggests items and files in the feedback box and links them once sent", async () => {
    mentionRoutes();
    const { dialog } = await openTask();
    const messages = await openMessages(dialog);
    const box = within(messages).getByRole("combobox", { name: "Feedback" });
    await expectMentionPopover(box);
    await userEvent.type(box, "report.tsx");
    await userEvent.click(
      within(messages).getByRole("button", { name: "Send" }),
    );
    const item = await within(messages).findByRole(
      "link",
      { name: "T-9" },
      WAIT,
    );
    expect(item.getAttribute("href")).toBe("/p/demo/t/T-9");
    expect(
      within(messages)
        .getByRole("link", { name: "web/report.tsx" })
        .getAttribute("href"),
    ).toBe("/p/demo/repository?path=web%2Freport.tsx");
  }, 15_000);

  it("offers the claim holder, the area lead and the main agent, the holder first", async () => {
    const { dialog } = await openTask();
    const messages = await openMessages(dialog);
    const choice = within(messages).getByRole("combobox", { name: "Send to" });
    await waitFor(() => {
      expect(
        within(choice)
          .getAllByRole("option")
          .map((option) => option.textContent),
      ).toEqual([
        "frontend/ui (claim holder)",
        "web-lead (area lead)",
        "main (main agent)",
      ]);
    }, WAIT);
    expect((choice as HTMLSelectElement).value).toBe("holder");
    await userEvent.selectOptions(choice, "agent:web-lead");
    const box = within(messages).getByRole("combobox", { name: "Feedback" });
    expect(box.getAttribute("placeholder")).toBe("Write feedback to web-lead…");
    await userEvent.type(box, "Who builds this?");
    await userEvent.click(
      within(messages).getByRole("button", { name: "Send" }),
    );
    await waitFor(() => {
      expect(daemon.made(`POST ${P}/demo/feedback`)[0]?.body).toEqual({
        task: "T-2",
        body: "Who builds this?",
        to: ["web-lead"],
      });
    }, WAIT);
    // The write ends once the message list has been read again.
    expect(
      await within(messages).findByText(
        "Who builds this?",
        { selector: "p" },
        WAIT,
      ),
    ).toBeTruthy();
  }, 15_000);
});

describe("the task's messages", () => {
  const message = (
    n: number,
    thread: number,
    over: Partial<MessageView>,
  ): MessageView => ({
    id: messageId(n),
    thread: messageId(thread),
    sender: "frontend/ui",
    choices: [],
    header: null,
    asks_human: false,
    lines: null,
    recipients: [],
    body: `message ${String(n)}`,
    proposal: null,
    request: null,
    reply_to: n === thread ? null : messageId(thread),
    task: "T-2",
    sent_at: 1_000 + n,
    ...over,
  });

  it("stays collapsed with the human's thread count and opens on a click to every message of each thread", async () => {
    linked = [
      message(40, 40, { sender: "human:tanim", body: "Use the short copy" }),
      message(41, 40, {
        sender: "frontend/ui",
        body: "Asked the lead first",
        task: null,
      }),
      message(42, 40, {
        sender: "web-lead",
        body: "Short copy it is",
        task: null,
      }),
      message(50, 50, { body: "Which font?", asks_human: true }),
    ];
    const { dialog } = await openTask();
    const messages = region(dialog, "Messages");
    expect(
      await within(messages).findByText("2 threads", {}, WAIT),
    ).toBeTruthy();
    expect(within(messages).getByText("1 waits for you")).toBeTruthy();
    const head = within(messages).getByRole("button", { name: "Messages" });
    expect(head.getAttribute("aria-expanded")).toBe("false");
    const body = document.getElementById(
      head.getAttribute("aria-controls") ?? "",
    );
    expect(body?.hidden).toBe(true);
    expect(
      body?.contains(within(messages).getByText("Use the short copy")),
    ).toBe(true);
    expect(
      within(messages).queryByRole("combobox", {
        name: "Feedback",
        hidden: false,
      }),
    ).toBeNull();
    await userEvent.click(head);
    expect(head.getAttribute("aria-expanded")).toBe("true");
    expect(body?.hidden).toBe(false);
    const threads = within(messages)
      .getAllByRole("listitem")
      .filter((item) => item.parentElement?.tagName === "UL");
    expect(threads).toHaveLength(2);
    const [asked, feedback] = threads as [HTMLElement, HTMLElement];
    expect(within(asked).getByText("Waits for you")).toBeTruthy();
    expect(within(asked).getByText("Which font?")).toBeTruthy();
    expect(
      ["Use the short copy", "Asked the lead first", "Short copy it is"].map(
        (body) => within(feedback).getByText(body).textContent,
      ),
    ).toHaveLength(3);
    expect(within(feedback).queryByText("Waits for you")).toBeNull();
    expect(
      within(messages).getByRole("combobox", { name: "Feedback" }),
    ).toBeTruthy();
    expect(
      daemon.calls.some(
        (call) =>
          call.path.startsWith(`${P}/demo/messages?`) &&
          call.path.includes("task=T-2") &&
          call.path.includes("mine=true"),
      ),
    ).toBe(true);
    await userEvent.type(
      within(messages).getByRole("combobox", { name: "Feedback" }),
      "Half drafted",
    );
    await userEvent.click(head);
    expect(body?.hidden).toBe(true);
    expect(
      within(messages).queryByRole("combobox", {
        name: "Feedback",
        hidden: false,
      }),
    ).toBeNull();
    await userEvent.click(head);
    expect(
      within(messages).getByRole<HTMLTextAreaElement>("combobox", {
        name: "Feedback",
      }).value,
    ).toBe("Half drafted");
  }, 15_000);
});

describe("a task without a proposal", () => {
  it("shows the report and completes the task with a pending state", async () => {
    const { dialog } = await openTask("T-7", DESIGN);
    const lead = region(dialog, "Lead check");
    expect(within(lead).getByText("Waits for a lead check")).toBeTruthy();
    expect(within(lead).getByText(REPORT)).toBeTruthy();
    expect(
      within(dialog).queryByRole("region", { name: "Proposal" }),
    ).toBeNull();
    expect(
      within(dialog).queryByRole("region", { name: "Workspace" }),
    ).toBeNull();
    await userEvent.click(
      within(lead).getByRole("button", { name: "Complete task" }),
    );
    expect(
      within(lead)
        .getByRole("button", { name: "Completing task…" })
        .getAttribute("aria-busy"),
    ).toBe("true");
    expect(
      await within(region(dialog, "Lead check")).findByText(
        "Completed by a lead check",
        {},
        WAIT,
      ),
    ).toBeTruthy();
    expect(
      within(region(dialog, "Lead check")).queryByRole("button", {
        name: "Complete task",
      }),
    ).toBeNull();
    expect(daemon.made(`POST ${P}/demo/tasks/T-7/complete`)).toHaveLength(1);
  }, 15_000);

  it("sends the task back with a note, refusing an empty one", async () => {
    const { dialog } = await openTask("T-7", DESIGN);
    const lead = region(dialog, "Lead check");
    await userEvent.click(
      within(lead).getByRole("button", { name: "Send back" }),
    );
    const form = within(lead).getByRole("form", {
      name: "Send the task back",
    });
    await userEvent.click(
      within(form).getByRole("button", { name: "Send back" }),
    );
    expect(
      await within(form).findByText("Say what the task worker must change."),
    ).toBeTruthy();
    expect(daemon.made(`POST ${P}/demo/tasks/T-7/send-back`)).toHaveLength(0);
    await userEvent.type(
      within(form).getByRole("textbox", { name: "Note to the task worker" }),
      "Use the colour tokens.",
    );
    await userEvent.click(
      within(form).getByRole("button", { name: "Send back" }),
    );
    expect(
      within(form)
        .getByRole("button", { name: "Sending back…" })
        .getAttribute("aria-busy"),
    ).toBe("true");
    const after = region(dialog, "Lead check");
    expect(
      await within(after).findByText("Sent back to the task worker", {}, WAIT),
    ).toBeTruthy();
    expect(within(after).getByText("Use the colour tokens.")).toBeTruthy();
    expect(daemon.made(`POST ${P}/demo/tasks/T-7/send-back`)[0]?.body).toEqual({
      note: "Use the colour tokens.",
    });
  }, 15_000);

  it("marks the card in Review with the report note", async () => {
    setup("/p/demo/board");
    const title = await screen.findByRole("link", { name: DESIGN }, WAIT);
    const card = title.closest<HTMLElement>("[data-task]");
    if (!card) throw new Error("no card");
    expect(card.dataset.task).toBe("T-7");
    expect(within(card).getByText("Waits for lead check")).toBeTruthy();
    expect(within(card).getByText(REPORT)).toBeTruthy();
    expect(within(card).getByText("no proposal")).toBeTruthy();
  }, 15_000);

  it("creates a task that needs no proposal", async () => {
    setup("/p/demo/t/new");
    const dialog = await screen.findByRole("dialog", {}, WAIT);
    await userEvent.type(
      within(dialog).getByRole("textbox", { name: "Title" }),
      "Draw the icons",
    );
    await userEvent.click(
      within(dialog).getByRole("radio", { name: /An agent, with no proposal/ }),
    );
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Create task" }),
    );
    await screen.findByRole("heading", { name: "Draw the icons" }, WAIT);
    expect(daemon.made(`POST ${P}/demo/tasks`)[0]?.body).toEqual({
      title: "Draw the icons",
      description: "",
      kind: "no_proposal",
    });
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
        .getByRole("button", { name: "Archiving task…" })
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
