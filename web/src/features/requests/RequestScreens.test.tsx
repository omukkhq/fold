// @vitest-environment jsdom
import { byContent } from "@/test/content";
import { QueryClientProvider, type QueryClient } from "@tanstack/react-query";
import { RouterProvider, createMemoryHistory } from "@tanstack/react-router";
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { File as NodeFile } from "node:buffer";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { RequestDetail, RequestMessage } from "@/api/requests";
import { createQueryClient } from "@/app/queryClient";
import { createAppRouter } from "@/app/router";
import { applyEvent } from "@/live/apply";
import { isEventEnvelope } from "@/live/envelope";
import { daemon, ok, refuse } from "@/test/daemon";
import {
  card,
  requestComment,
  requestDetail,
  requestDraft,
  requestMessage,
  requestSummary,
} from "@/test/requests";
import type { MultipartBody } from "@/test/stubFetch";
import { ThemeProvider } from "@/theme/ThemeProvider";

vi.mock("@/api/client", async (importOriginal) => {
  const real = await importOriginal<typeof import("@/api/client")>();
  const { daemon: fake } = await import("@/test/daemon");
  return { ...real, api: fake.api };
});

/** The thread id is built, so the utility guard does not read it as a margin class. */
const THREAD = `m-${String(1)}`;
let requests: Map<string, RequestDetail>;
let threads: Map<string, RequestMessage[]>;
let next: number;
let held: Promise<void> | null;
/** The code the next requester write fails with, when set. */
let refusal: {
  status: number;
  code: string;
  details?: Record<string, unknown>;
} | null;

function hold(): () => void {
  let release: () => void = () => undefined;
  held = new Promise<void>((resolve) => {
    release = resolve;
  });
  return () => {
    held = null;
    release();
  };
}

function put(detail: RequestDetail) {
  requests.set(detail.request.id, detail);
}

function seed() {
  next = 8;
  held = null;
  refusal = null;
  requests = new Map();
  threads = new Map();
  put(
    requestDetail(
      requestSummary("RQ-7", {
        excerpt: "Product wants the board as a spreadsheet",
      }),
    ),
  );
  put(
    requestDetail(
      requestSummary("RQ-6", {
        title: "Login form accepts an empty password",
        kind: "bug",
        state: "drafting",
        requester: "frontend",
        drafter: "backend",
      }),
    ),
  );
  put(
    requestDetail(
      requestSummary("RQ-4", {
        title: "Report export as CSV and PDF",
        kind: "feature",
        state: "draft_ready",
        drafter: "backend",
        latest_version: 2,
      }),
      {
        body: "Export the data that #T-9 shows, in @web/src/report.tsx. Not `#T-1`.",
        attachments: [
          {
            name: "sample.csv",
            media_type: "text/csv",
            size: 18 * 1024,
            position: 1,
          },
        ],
        drafts: [
          requestDraft(1, {
            cards: [card("c1", "Query"), card("c2", "Export")],
          }),
          requestDraft(2, {
            cards: [
              card("c1", "Query"),
              card("c2", "CSV download"),
              card("c3", "PDF download"),
            ],
            edges: [
              { dependent: "c2", prerequisite: "c1" },
              { dependent: "c3", prerequisite: "c1" },
            ],
            note: "the PDF has its own card",
          }),
        ],
        comments: [
          requestComment(1, "Split the export, as in #RQ-3, not #P-3."),
        ],
        thread: { id: THREAD, last_at: 1, messages: 1 },
      },
    ),
  );
  threads.set(THREAD, [
    requestMessage(1, "backend", "human:tanim", "Archived tasks too? #T-9"),
  ]);
  put(
    requestDetail(
      requestSummary("RQ-3", {
        title: "Dark theme hides the gutter",
        kind: "bug",
        state: "draft_ready",
        requester: "review",
        drafter: "frontend",
        latest_version: 1,
      }),
      { drafts: [requestDraft(1)] },
    ),
  );
}

function update(
  id: string,
  change: (d: RequestDetail) => RequestDetail,
): RequestDetail {
  const found = requests.get(id);
  if (!found) throw new Error(`no ${id}`);
  const changed = change(found);
  put(changed);
  return changed;
}

async function requesterWrite(
  id: string,
  change: (d: RequestDetail) => RequestDetail,
) {
  if (held) await held;
  if (refusal) return refuse(refusal.status, refusal.code, refusal.details);
  return ok(update(id, change));
}

const board = (tasks: string[]) => ({
  columns: [
    { status: "backlog", total: 0, cards: [] },
    {
      status: "todo",
      total: tasks.length,
      cards: tasks.map((task, i) => ({
        task,
        title: `Board ${task}`,
        status: i === 0 ? "todo" : "backlog",
        blocked: false,
        blocked_by: [],
        blocks: 0,
        needs: [],
        labels: [],
        attention: [],
        checklist: {
          steps_done: 0,
          steps_total: 0,
          linked_done: 0,
          linked_total: 0,
        },
        messages: 0,
        proposal: null,
        activity: null,
        human_review: null,
        completed_at: null,
      })),
    },
  ],
});

function routes() {
  daemon.shell(["demo"]);
  daemon.on("GET /api/v1/projects/:slug/requests", () =>
    ok({ items: [...requests.values()].map((d) => d.request) }),
  );
  daemon.on("GET /api/v1/projects/:slug/requests/:id", (call) => {
    const found = requests.get(call.params.id ?? "");
    return found ? ok(found) : refuse(404, "request_not_found");
  });
  daemon.on("POST /api/v1/projects/:slug/requests", async (call) => {
    if (held) await held;
    if (refusal) return refuse(refusal.status, refusal.code, refusal.details);
    const form = call.body as MultipartBody;
    const id = `RQ-${String(next)}`;
    next += 1;
    const body = form.fields.body ?? "";
    const created = requestDetail(requestSummary(id, { excerpt: body }), {
      body,
      attachments: form.files.map((f, i) => ({
        name: f.name,
        media_type: f.type,
        size: 1,
        position: i + 1,
      })),
    });
    put(created);
    return ok(created, 201);
  });
  daemon.on("POST /api/v1/projects/:slug/requests/:id/changes", (call) =>
    requesterWrite(call.params.id ?? "", (d) => ({
      ...d,
      request: { ...d.request, state: "changes_requested" },
      comments: [
        ...d.comments,
        requestComment(
          (call.body as { version: number }).version,
          (call.body as { comment: string }).comment,
        ),
      ],
    })),
  );
  daemon.on("POST /api/v1/projects/:slug/requests/:id/drafts", (call) =>
    requesterWrite(call.params.id ?? "", (d) => {
      const version = (d.request.latest_version ?? 0) + 1;
      const body = call.body as Pick<
        RequestDetail["drafts"][number],
        "cards" | "edges" | "note"
      >;
      return {
        ...d,
        request: {
          ...d.request,
          state: "draft_ready",
          latest_version: version,
        },
        drafts: [
          ...d.drafts,
          requestDraft(version, {
            author: "human:tanim",
            cards: body.cards,
            edges: body.edges,
            note: body.note ?? null,
          }),
        ],
      };
    }),
  );
  daemon.on("POST /api/v1/projects/:slug/requests/:id/approve", (call) =>
    requesterWrite(call.params.id ?? "", (d) => ({
      ...d,
      request: { ...d.request, state: "approved" },
      tasks: ["T-12", "T-13", "T-14"],
    })),
  );
  daemon.on("POST /api/v1/projects/:slug/requests/:id/close", (call) =>
    requesterWrite(call.params.id ?? "", (d) => ({
      ...d,
      request: { ...d.request, state: "closed" },
      close_reason: (call.body as { reason: string }).reason,
    })),
  );
  daemon.on("GET /api/v1/projects/:slug/threads/:id", (call) =>
    ok({
      id: call.params.id,
      awaiting_human: false,
      last_at: 1,
      messages: threads.get(call.params.id ?? "") ?? [],
      participants: [],
      proposal: null,
      request: "RQ-4",
      task: null,
    }),
  );
  daemon.on("POST /api/v1/projects/:slug/feedback", (call) => {
    const body = call.body as { body: string; request: string };
    const thread = threads.get(THREAD) ?? [];
    const message = requestMessage(
      thread.length + 1,
      "human:tanim",
      "backend",
      body.body,
    );
    threads.set(THREAD, [...thread, message]);
    update(body.request, (d) => ({
      ...d,
      thread: { id: THREAD, last_at: 2, messages: thread.length + 1 },
    }));
    return ok(message, 201);
  });
  daemon.on("GET /api/v1/projects/:slug/proposals", ok({ proposals: [] }));
  daemon.on("GET /api/v1/projects/:slug/board", () => {
    const approved = [...requests.values()].flatMap((d) => d.tasks);
    return ok(board(approved.length > 0 ? approved : ["T-9"]));
  });
  daemon.on(
    "GET /api/v1/projects/:slug/revisions",
    ok({ revisions: [], before: null }),
  );
  daemon.on(
    "GET /api/v1/projects/:slug/repository",
    ok({ head: "r3", main_path: "/tmp/main", recent: [] }),
  );
  daemon.on("GET /api/v1/projects/:slug/revisions/:r/tree", (call) =>
    ok({
      path: "",
      entries: call.path.includes("path=web")
        ? [
            {
              name: "report.tsx",
              path: "web/report.tsx",
              kind: "file",
              hash: "h",
              size: 1,
            },
          ]
        : [
            {
              name: "web",
              path: "web",
              kind: "directory",
              hash: "h",
              size: null,
            },
          ],
    }),
  );
}

beforeEach(() => {
  seed();
  routes();
  vi.spyOn(window, "scrollTo").mockImplementation(() => undefined);
});
afterEach(() => {
  daemon.reset();
  cleanup();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

function renderAt(path: string) {
  const queryClient = createQueryClient();
  const router = createAppRouter(queryClient);
  router.update({
    history: createMemoryHistory({ initialEntries: [path] }),
    context: { queryClient },
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

function deliver(
  queryClient: QueryClient,
  type: string,
  data: Record<string, unknown>,
) {
  const frame: unknown = {
    kind: "event",
    seq: 100,
    type,
    actor: "backend",
    task: null,
    proposal: null,
    workspace: null,
    revision: null,
    data,
    created_at: Date.now(),
  };
  if (!isEventEnvelope(frame)) throw new Error("not a frame");
  act(() => {
    applyEvent(queryClient, "demo", frame);
  });
}

async function rows() {
  const table = await screen.findByRole("table");
  await waitFor(() => {
    expect(within(table).queryByLabelText("Loading requests")).toBeNull();
  });
  return within(table)
    .getAllByRole("row")
    .slice(1)
    .map((row) => row.querySelector("td")?.textContent ?? "");
}

describe("Requests list", () => {
  it("lists every request with its kind, state and drafter; an unsorted one shows its excerpt muted", async () => {
    renderAt("/p/demo/requests");
    expect(await rows()).toEqual(["RQ-7", "RQ-6", "RQ-4", "RQ-3"]);
    const excerpt = screen.getByRole("link", {
      name: "Product wants the board as a spreadsheet",
    });
    expect(excerpt.className).toContain("text-text-muted");
    const unsorted = excerpt.closest("tr");
    expect(unsorted && within(unsorted).getByText("Unsorted")).toBeTruthy();
    expect(unsorted && within(unsorted).getByText("unclaimed")).toBeTruthy();
    expect(screen.getByText("4 requests · 1 needs a human")).toBeTruthy();
    expect(
      screen
        .getByRole("link", { name: "All · 4" })
        .getAttribute("aria-current"),
    ).toBe("page");
    expect(
      screen.getByRole("navigation", { name: "Project" }).textContent,
    ).toContain("Requests");
  });

  it("filters by state, kind and the human's own requests through the address", async () => {
    const { router } = renderAt("/p/demo/requests?state=draft_ready");
    expect(await rows()).toEqual(["RQ-4", "RQ-3"]);
    await userEvent.click(screen.getByRole("button", { name: "Bug" }));
    await waitFor(() => {
      expect(router.state.location.search).toEqual({
        state: "draft_ready",
        kind: "bug",
      });
    });
    expect(await rows()).toEqual(["RQ-3"]);
    await userEvent.click(screen.getByRole("button", { name: "Mine" }));
    expect(
      await screen.findByRole("region", { name: "No request matches" }),
    ).toBeTruthy();
  });

  it("shows a request an agent created from the stream, with no reload", async () => {
    const { queryClient } = renderAt("/p/demo/requests");
    expect(await rows()).toHaveLength(4);
    put(
      requestDetail(
        requestSummary("RQ-9", {
          requester: "frontend",
          excerpt: "Agent asks",
        }),
      ),
    );
    deliver(queryClient, "request.created", { request: "RQ-9" });
    await waitFor(async () => {
      expect(await rows()).toEqual(["RQ-7", "RQ-6", "RQ-4", "RQ-3", "RQ-9"]);
    });
  });

  it("renders the empty state for a project without requests", async () => {
    requests.clear();
    renderAt("/p/demo/requests");
    expect(
      await screen.findByRole("region", { name: "No requests yet" }),
    ).toBeTruthy();
    expect(
      screen.getByText("An agent creates one with the create_request tool."),
    ).toBeTruthy();
  });

  it("shows a load failure with its code and reads again", async () => {
    daemon.on(
      "GET /api/v1/projects/:slug/requests",
      refuse(500, "internal_error"),
    );
    renderAt("/p/demo/requests");
    const alert = await screen.findByRole("alert");
    expect(within(alert).getByText("code: internal_error")).toBeTruthy();
    expect(screen.getByText("Not loaded")).toBeTruthy();
    daemon.on("GET /api/v1/projects/:slug/requests", () =>
      ok({ items: [...requests.values()].map((d) => d.request) }),
    );
    await userEvent.click(
      within(alert).getByRole("button", { name: "Try again" }),
    );
    expect(await rows()).toHaveLength(4);
  });
});

describe("New request dialog", () => {
  async function open() {
    renderAt("/p/demo/requests");
    await rows();
    await userEvent.click(
      screen.getAllByRole("button", { name: "New request" })[0] as HTMLElement,
    );
    return screen.findByRole("dialog", { name: "New request" });
  }

  it("creates a request from the message and the files, pending until fold confirms", async () => {
    // jsdom's File loses its name and bytes inside the runtime's multipart encoder, so the upload uses the runtime's own classes.
    const probe = await new Response(new URLSearchParams("a=b")).formData();
    vi.stubGlobal("FormData", probe.constructor);
    const dialog = await open();
    expect(within(dialog).queryByLabelText("Kind")).toBeNull();
    expect(within(dialog).queryByLabelText("Title")).toBeNull();
    await userEvent.type(
      within(dialog).getByLabelText("What do you need?"),
      "Export the weekly report",
    );
    const input = within(dialog).getByLabelText("Attach files");
    fireEvent.change(input, {
      target: {
        files: [new NodeFile(["a,b"], "sample.csv", { type: "text/csv" })],
      },
    });
    expect(within(dialog).getByText("sample.csv")).toBeTruthy();
    const release = hold();
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Create request" }),
    );
    const pending = await within(dialog).findByRole("button", {
      name: "Creating…",
    });
    expect(pending.getAttribute("aria-busy")).toBe("true");
    expect(
      within(dialog).getByText(
        /The request appears in the list when fold confirms/,
      ),
    ).toBeTruthy();
    release();
    await waitFor(() => {
      expect(screen.queryByRole("dialog")).toBeNull();
    });
    expect(await rows()).toContain("RQ-8");
    expect(requests.get("RQ-8")?.attachments.map((a) => a.name)).toEqual([
      "sample.csv",
    ]);
  });

  it("refuses a file over 10 MiB before any upload", async () => {
    const dialog = await open();
    await userEvent.type(
      within(dialog).getByLabelText("What do you need?"),
      "Look",
    );
    const big = new File(["x"], "mockup.png", { type: "image/png" });
    Object.defineProperty(big, "size", { value: 14.2 * 1024 * 1024 });
    fireEvent.change(within(dialog).getByLabelText("Attach files"), {
      target: { files: [big] },
    });
    expect(within(dialog).getByRole("alert").textContent).toContain(
      "mockup.png is 14.2 MiB",
    );
    expect(
      within(dialog).getByRole("button", { name: "Create request" }),
    ).toHaveProperty("disabled", true);
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Remove mockup.png" }),
    );
    expect(
      within(dialog).getByRole("button", { name: "Create request" }),
    ).toHaveProperty("disabled", false);
    expect(daemon.made("POST /api/v1/projects/demo/requests")).toHaveLength(0);
  });

  it("shows the daemon's refusal with its code and keeps the message", async () => {
    refusal = { status: 401, code: "human_name_required" };
    const dialog = await open();
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Create request" }),
    );
    expect(
      await within(dialog).findByText("Say what you need first."),
    ).toBeTruthy();
    await userEvent.type(
      within(dialog).getByLabelText("What do you need?"),
      "Need it",
    );
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Create request" }),
    );
    const alert = await within(dialog).findByRole("alert");
    expect(alert.textContent).toContain("fold could not create the request");
    expect(alert.textContent).toContain("code: human_name_required");
    expect(within(dialog).getByLabelText("What do you need?")).toHaveProperty(
      "value",
      "Need it",
    );
  });

  it("suggests fold items after # and files after @ and writes the pick", async () => {
    const dialog = await open();
    const box = within(dialog).getByLabelText("What do you need?");
    await userEvent.type(box, "Data of #T-");
    const option = await screen.findByRole("option", { name: /#T-9/ });
    expect(screen.getByText("Tasks")).toBeTruthy();
    expect(option.getAttribute("aria-selected")).toBe("true");
    await userEvent.keyboard("{Enter}");
    expect(box).toHaveProperty("value", "Data of #T-9 ");
    await userEvent.type(box, "in @w");
    await userEvent.click(
      await screen.findByRole("option", { name: /@web\// }),
    );
    expect(box).toHaveProperty("value", "Data of #T-9 in @web/");
    expect(
      await screen.findByRole("option", { name: /@report\.tsx/ }),
    ).toBeTruthy();
    await userEvent.keyboard("{Escape}");
    await waitFor(() => {
      expect(screen.queryByRole("listbox")).toBeNull();
    });
    expect(screen.getByRole("dialog", { name: "New request" })).toBeTruthy();
  });
});

describe("Request detail", () => {
  it("shows the message with linked mentions, the attachments, the draft versions, edges, comments and thread", async () => {
    renderAt("/p/demo/requests/RQ-4");
    expect(
      await screen.findByRole("heading", {
        name: "Report export as CSV and PDF",
        level: 1,
      }),
    ).toBeTruthy();
    const body = screen.getByRole("region", { name: "Request" });
    expect(
      within(body).getByRole("link", { name: "T-9" }).getAttribute("href"),
    ).toBe("/p/demo/t/T-9");
    expect(
      within(body)
        .getByRole("link", { name: "web/src/report.tsx" })
        .getAttribute("href"),
    ).toBe("/p/demo/repository?path=web%2Fsrc%2Freport.tsx");
    expect(within(body).queryByRole("link", { name: "T-1" })).toBeNull();
    expect(
      within(body)
        .getByRole("link", { name: "Download sample.csv" })
        .getAttribute("href"),
    ).toBe("/api/v1/projects/demo/requests/RQ-4/attachments/1");
    expect(screen.getByText("Version 2 of 2")).toBeTruthy();
    expect(screen.getByText("PDF download")).toBeTruthy();
    expect(screen.getByText("CSV download needs Query")).toBeTruthy();
    await userEvent.click(screen.getByRole("button", { name: "v1" }));
    expect(screen.getByText("Version 1 of 2")).toBeTruthy();
    expect(screen.queryByText("PDF download")).toBeNull();
    expect(screen.getByText("1 on v1, none on v2")).toBeTruthy();
    const comments = screen.getByRole("list", { name: "Comments" });
    expect(within(comments).getByRole("link", { name: "RQ-3" })).toBeTruthy();
    // The project has no P-3, so that mention names nothing and stays text.
    expect(within(comments).queryByRole("link", { name: "P-3" })).toBeNull();
    const thread = await screen.findByRole("list", {
      name: "Clarification thread",
    });
    expect(within(thread).getByText(/Archived tasks too\?/)).toBeTruthy();
  });

  it("approves the version on screen, pending until fold answers, then lists the created tasks in their columns", async () => {
    renderAt("/p/demo/requests/RQ-4");
    const panel = await screen.findByRole("complementary", {
      name: "Request actions",
    });
    expect(within(panel).getByText(/You asked for RQ-4/)).toBeTruthy();
    const release = hold();
    await userEvent.click(
      within(panel).getByRole("button", { name: "Approve v2" }),
    );
    const pending = await within(panel).findByRole("button", {
      name: "Approving v2…",
    });
    expect(pending.getAttribute("aria-busy")).toBe("true");
    expect(
      within(panel).getByText(/Waiting for fold to create 3 tasks/),
    ).toBeTruthy();
    expect(within(panel).getByRole("button", { name: "Edit" })).toHaveProperty(
      "disabled",
      true,
    );
    expect(screen.queryByRole("region", { name: "Created tasks" })).toBeNull();
    release();
    const created = await screen.findByRole("list", { name: "Created tasks" });
    expect(
      daemon.made("POST /api/v1/projects/demo/requests/RQ-4/approve")[0]?.body,
    ).toEqual({
      expected_version: 2,
    });
    await waitFor(() => {
      expect(within(created).getByText("Board T-12")).toBeTruthy();
    });
    expect(within(created).getByText("Todo")).toBeTruthy();
    expect(within(created).getAllByText("Backlog")).toHaveLength(2);
    expect(within(created).getByText("from c3")).toBeTruthy();
    expect(screen.getByRole("combobox", { name: "Message" })).toHaveProperty(
      "disabled",
      true,
    );
  });

  it("asks for changes with a comment on the latest version", async () => {
    renderAt("/p/demo/requests/RQ-4");
    const panel = await screen.findByRole("complementary", {
      name: "Request actions",
    });
    await userEvent.click(
      within(panel).getByRole("button", { name: "Request changes" }),
    );
    expect(
      await within(panel).findByText("Say what needs to change."),
    ).toBeTruthy();
    await userEvent.type(
      within(panel).getByLabelText("Comment for v2"),
      "Name the CSV header",
    );
    const release = hold();
    await userEvent.click(
      within(panel).getByRole("button", { name: "Request changes" }),
    );
    expect(
      await within(panel).findByRole("button", { name: "Requesting changes…" }),
    ).toBeTruthy();
    release();
    expect(await screen.findByText("Changes requested")).toBeTruthy();
    expect(
      daemon.made("POST /api/v1/projects/demo/requests/RQ-4/changes")[0]?.body,
    ).toEqual({
      version: 2,
      comment: "Name the CSV header",
    });
    expect(within(panel).queryByRole("button", { name: /Approve/ })).toBeNull();
  });

  it("edits the draft into a new version, checking it first with the daemon's reasons", async () => {
    renderAt("/p/demo/requests/RQ-4");
    const panel = await screen.findByRole("complementary", {
      name: "Request actions",
    });
    await userEvent.click(within(panel).getByRole("button", { name: "Edit" }));
    expect(screen.getByText("Editing · saves as version 3")).toBeTruthy();
    await userEvent.clear(screen.getByLabelText("Title of c3"));
    await userEvent.click(
      within(panel).getByRole("button", { name: "Save as v3" }),
    );
    expect(within(panel).getByRole("alert").textContent).toContain(
      "Card c3 needs a title.",
    );
    expect(
      daemon.made("POST /api/v1/projects/demo/requests/RQ-4/drafts"),
    ).toHaveLength(0);
    await userEvent.click(screen.getByRole("button", { name: "Remove c3" }));
    await userEvent.click(
      screen.getByRole("button", { name: "Add card · 2 of 20" }),
    );
    await userEvent.type(screen.getByLabelText("Title of c3"), "Totals row");
    await userEvent.type(
      screen.getByLabelText("Description of c3"),
      "Scope: totals",
    );
    await userEvent.click(screen.getByRole("button", { name: "Move c3 up" }));
    await userEvent.click(screen.getByRole("button", { name: "Add edge" }));
    await userEvent.type(
      within(panel).getByLabelText("Note for v3"),
      "added totals",
    );
    await userEvent.click(
      within(panel).getByRole("button", { name: "Save as v3" }),
    );
    expect(await screen.findByText("Version 3 of 3")).toBeTruthy();
    const sent = daemon.made(
      "POST /api/v1/projects/demo/requests/RQ-4/drafts",
    )[0]?.body as {
      cards: { key: string }[];
      edges: unknown[];
      note: string;
    };
    expect(sent.cards.map((c) => c.key)).toEqual(["c1", "c3", "c2"]);
    expect(sent.edges).toEqual([
      { dependent: "c2", prerequisite: "c1" },
      { dependent: "c1", prerequisite: "c3" },
    ]);
    expect(sent.note).toBe("added totals");
  });

  it("closes the request with a reason", async () => {
    renderAt("/p/demo/requests/RQ-4");
    const panel = await screen.findByRole("complementary", {
      name: "Request actions",
    });
    await userEvent.click(within(panel).getByRole("button", { name: "Close" }));
    const dialog = await screen.findByRole("dialog", { name: "Close RQ-4" });
    await userEvent.type(
      within(dialog).getByLabelText("Reason"),
      "Done in #T-9",
    );
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Close request" }),
    );
    const closed = await screen.findByRole("region", { name: "Closed" });
    expect(within(closed).getByRole("link", { name: "T-9" })).toBeTruthy();
  });

  it("shows an error code on the action that failed and reads the request again", async () => {
    refusal = {
      status: 409,
      code: "stale_version",
      details: { expected: 2, latest: 3 },
    };
    renderAt("/p/demo/requests/RQ-4");
    const panel = await screen.findByRole("complementary", {
      name: "Request actions",
    });
    const reads = daemon.made("GET /api/v1/projects/demo/requests/RQ-4").length;
    await userEvent.click(
      within(panel).getByRole("button", { name: "Approve v2" }),
    );
    const alert = await within(panel).findByRole("alert");
    expect(alert.textContent).toContain(
      "v3 is now the latest version. Read it, then try once more.",
    );
    expect(alert.textContent).toContain("code: stale_version");
    await waitFor(() => {
      expect(
        daemon.made("GET /api/v1/projects/demo/requests/RQ-4").length,
      ).toBeGreaterThan(reads);
    });
  });

  it("gives a non-requester no actions but lets them write in the thread", async () => {
    renderAt("/p/demo/requests/RQ-3");
    const panel = await screen.findByRole("complementary", {
      name: "Request actions",
    });
    expect(
      within(panel).getByText(
        byContent("Only @review can approve, ask for changes, edit or close."),
      ),
    ).toBeTruthy();
    expect(within(panel).queryAllByRole("button")).toEqual([]);
    expect(
      within(panel).getByText(
        byContent(
          "@review approves one version at a time. Each new version starts unapproved.",
        ),
      ),
    ).toBeTruthy();
    expect(screen.getByRole("combobox", { name: "Message" })).toHaveProperty(
      "disabled",
      false,
    );
  });

  it("shows a new draft version from the stream, with no reload", async () => {
    const { queryClient } = renderAt("/p/demo/requests/RQ-3");
    expect(await screen.findByText("Version 1 of 1")).toBeTruthy();
    update("RQ-3", (d) => ({
      ...d,
      request: { ...d.request, latest_version: 2 },
      drafts: [
        ...d.drafts,
        requestDraft(2, {
          author: "frontend",
          cards: [card("c1", "Gutter colour")],
        }),
      ],
    }));
    deliver(queryClient, "request.draft_submitted", {
      request: "RQ-3",
      version: 2,
    });
    expect(await screen.findByText("Version 2 of 2")).toBeTruthy();
    expect(screen.getByText("Gutter colour")).toBeTruthy();
  });

  it("sends a thread message that changes nothing on the request", async () => {
    renderAt("/p/demo/requests/RQ-4");
    const box = await screen.findByRole("combobox", { name: "Message" });
    expect(box.getAttribute("placeholder")).toBe(
      "Reply to @backend about RQ-4…",
    );
    await userEvent.type(box, "No, only what the page shows");
    await userEvent.click(screen.getByRole("button", { name: "Send" }));
    const thread = screen.getByRole("list", { name: "Clarification thread" });
    expect(
      await within(thread).findByText("No, only what the page shows"),
    ).toBeTruthy();
    expect(daemon.made("POST /api/v1/projects/demo/feedback")[0]?.body).toEqual(
      {
        body: "No, only what the page shows",
        request: "RQ-4",
      },
    );
    expect(screen.getByText("2 messages in the thread of RQ-4")).toBeTruthy();
  });

  it("explains an id the project does not have", async () => {
    renderAt("/p/demo/requests/RQ-99");
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("RQ-99 does not exist in this project");
    expect(
      within(alert)
        .getByRole("link", { name: "Back to requests" })
        .getAttribute("href"),
    ).toBe("/p/demo/requests");
  });
});
