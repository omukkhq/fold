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
import { afterEach, describe, expect, it, vi } from "vitest";
import type { AgentView } from "@/api/agents";
import { keys } from "@/api/keys";
import type { Session } from "@/api/session";
import type { WorkerList } from "@/api/workers";
import type { AgentsSearch } from "@/features/workers/search";
import { applyEvent } from "@/live/apply";
import { agent } from "@/test/agents";
import { byContent } from "@/test/content";
import { errorReply, stubFetch, type StubReply } from "@/test/stubFetch";
import { permissionQuestion, workerList, workerProcess } from "@/test/workers";
import { AgentsScreen } from "./AgentsScreen";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const BASE = "/api/v1/projects/demo";

const team = workerList({
  areas: [{ name: "web", label: "frontend", lead: "web-lead" }],
  workers: [
    workerProcess("web-lead"),
    workerProcess("web-coder", {
      role: "task_coder",
      started_by: "web-lead",
      tasks: ["T-12"],
      model: "sonnet",
      state: "stopping",
    }),
    workerProcess("review", { role: "review_lead", area: null, state: "idle" }),
  ],
  roles: [
    { role: "area_lead", cache_ttl: "1h", model: null },
    { role: "review_lead", cache_ttl: "1h", model: null },
  ],
});

type Routes = Record<string, StubReply | (() => StubReply)>;

function routes(extra: Routes = {}): Routes {
  return {
    "GET /api/v1/session": { body: { human: "tanim", authenticated: true } },
    [`GET ${BASE}/worker-questions`]: { body: { items: [] } },
    [`GET ${BASE}/needs-you`]: { body: { items: [] } },
    [`GET ${BASE}/reviews/batches`]: { body: { batches: [] } },
    [`GET ${BASE}/board`]: { body: { columns: [] } },
    [`GET ${BASE}/labels`]: {
      body: { labels: [{ name: "frontend", color: "slate" }] },
    },
    ...extra,
  };
}

async function setup(
  agents: AgentView[],
  workers: WorkerList,
  search: AgentsSearch = {},
  onSearch: (next: AgentsSearch) => void = () => undefined,
  session: Session = { human: "tanim", authenticated: true },
) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  queryClient.setQueryData(keys.agents("demo"), agents);
  queryClient.setQueryData(keys.session(), session);
  queryClient.setQueryData(keys.workers("demo"), workers);
  const root = createRootRoute();
  const index = createRoute({
    getParentRoute: () => root,
    path: "/",
    component: () => (
      <AgentsScreen slug="demo" search={search} onSearch={onSearch} />
    ),
  });
  const router = createRouter({
    routeTree: root.addChildren([index]),
    history: createMemoryHistory({ initialEntries: ["/"] }),
  });
  render(
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  await screen.findByRole("heading", { name: "Agents" });
  return queryClient;
}

function row(name: string): HTMLElement {
  const cell = screen
    .getAllByText(`@${name}`)
    .find((element) => element.closest("tbody"));
  const tr = cell?.closest("tr");
  if (!tr) throw new Error(`no row for ${name}`);
  return tr;
}

function startButton(): HTMLElement {
  const [first] = screen.getAllByRole("button", { name: "Start a lead" });
  if (!first) throw new Error("no start button");
  return first;
}

describe("AgentsScreen", () => {
  it("shows how to start when no agent connected and fold runs no worker", async () => {
    stubFetch(routes());
    await setup([], workerList());
    const empty = screen.getByRole("region", { name: "No agents yet" });
    expect(within(empty).getByText("fld claude")).toBeTruthy();
    expect(
      within(empty).getByRole("button", { name: "Start a lead" }),
    ).toBeTruthy();
  });

  it("offers Start and Stop only to a named human", async () => {
    stubFetch(
      routes({
        "GET /api/v1/session": { body: { human: null, authenticated: true } },
      }),
    );
    await setup([agent("main")], team, {}, undefined, {
      human: null,
      authenticated: true,
    });
    expect(screen.queryByRole("button", { name: "Start a lead" })).toBeNull();
    expect(screen.queryByRole("button", { name: /^Stop/ })).toBeNull();
  });

  it("shows the main agent alone with the no-leads state", async () => {
    stubFetch(routes());
    await setup([agent("main")], workerList());
    expect(screen.getByText("No leads yet")).toBeTruthy();
    expect(
      screen.getByText(byContent(/Ask @main to plan the work/)),
    ).toBeTruthy();
    expect(within(row("main")).getByText("active")).toBeTruthy();
    expect(await within(row("main")).findByText("@tanim")).toBeTruthy();
  });

  it("groups the team under the main agent, the areas and review", async () => {
    stubFetch(routes());
    await setup([agent("main")], team);
    const table = screen.getByRole("table", { name: "Team" });
    expect(within(table).getByText("Main agent")).toBeTruthy();
    expect(await within(table).findByText("frontend")).toBeTruthy();
    expect(
      within(table).getByText("area web · lead owns 0 tasks"),
    ).toBeTruthy();
    expect(within(table).getByText("Review")).toBeTruthy();
    const lead = row("web-lead");
    expect(within(lead).getByText("Area lead")).toBeTruthy();
    expect(within(lead).getByText("$5.87")).toBeTruthy();
    expect(within(lead).queryByText("91%")).toBeNull();
    expect(within(table).queryByText(/cache/i)).toBeNull();
    expect(within(lead).getByText("164k")).toBeTruthy();
    const coder = row("web-coder");
    expect(within(coder).getByText("T-12")).toBeTruthy();
    expect(
      within(coder).getByRole("button", { name: "Stop web-coder" }).textContent,
    ).toContain("Stopping");
    expect(
      within(row("review")).getByText("waiting for its next turn"),
    ).toBeTruthy();
  });

  it("keeps Stop pending until the list shows the daemon's new state", async () => {
    let stopped = false;
    stubFetch(
      routes({
        [`POST ${BASE}/workers/web-lead/stop`]: () => {
          stopped = true;
          return { body: workerProcess("web-lead", { state: "stopping" }) };
        },
        [`GET ${BASE}/workers`]: () => ({
          body: stopped
            ? workerList({
                areas: team.areas,
                workers: [
                  workerProcess("web-lead", { state: "stopped", exit_code: 0 }),
                ],
              })
            : team,
        }),
      }),
    );
    await setup([], team);
    await userEvent.click(
      within(row("web-lead")).getByRole("button", { name: "Stop web-lead" }),
    );
    await waitFor(() => {
      expect(within(row("web-lead")).getByText("stopped")).toBeTruthy();
    });
    expect(within(row("web-lead")).queryByRole("button")).toBeNull();
    expect(within(row("web-lead")).getByText("exit code 0")).toBeTruthy();
  });

  it("shows a refused stop with its code", async () => {
    stubFetch(
      routes({
        [`POST ${BASE}/workers/web-lead/stop`]: errorReply(
          404,
          "worker_process_not_found",
        ),
      }),
    );
    await setup([], team);
    await userEvent.click(
      within(row("web-lead")).getByRole("button", { name: "Stop web-lead" }),
    );
    const alert = await within(row("web-lead")).findByRole("alert");
    expect(alert.textContent).toContain(
      "fold runs no process for this worker.",
    );
    expect(alert.textContent).toContain("code: worker_process_not_found");
  });

  it("starts a lead from the dialog and follows its steps from the stream", async () => {
    let started = false;
    const calls = stubFetch(
      routes({
        [`POST ${BASE}/workers`]: () => {
          started = true;
          return {
            status: 201,
            body: workerProcess("web-lead", { state: "starting" }),
          };
        },
        [`GET ${BASE}/workers`]: () => ({
          body: started
            ? workerList({
                areas: team.areas,
                workers: [workerProcess("web-lead", { state: "working" })],
              })
            : workerList({ areas: team.areas }),
        }),
      }),
    );
    const client = await setup(
      [agent("main")],
      workerList({ areas: team.areas }),
    );
    await userEvent.click(startButton());
    const dialog = screen.getByRole("dialog", { name: "Start a lead" });
    expect(
      within(dialog).getByRole("radio", { name: "Area lead" }),
    ).toHaveProperty("checked", true);
    await userEvent.selectOptions(
      within(dialog).getByLabelText("Model"),
      "sonnet",
    );
    await userEvent.type(
      within(dialog).getByLabelText("Instructions"),
      "Own the web area",
    );
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Start lead" }),
    );
    await waitFor(() => {
      expect(screen.queryByRole("dialog")).toBeNull();
    });
    expect(calls.find((c) => c.method === "POST")?.body).toEqual({
      role: "area_lead",
      area: "web",
      model: "sonnet",
      instructions: "Own the web area",
    });
    expect(within(row("web-lead")).getByText("working")).toBeTruthy();
    act(() => {
      applyEvent(client, "demo", {
        kind: "worker_output",
        agent: "web-lead",
        step: "tool_use",
        tool: "Agent",
        text_length: null,
      });
    });
    expect(await within(row("web-lead")).findByText("Agent")).toBeTruthy();
  });

  it("keeps the dialog open with the refusal and its code", async () => {
    stubFetch(
      routes({ [`POST ${BASE}/workers`]: errorReply(409, "area_has_lead") }),
    );
    await setup([agent("main")], team);
    await userEvent.click(startButton());
    const dialog = screen.getByRole("dialog", { name: "Start a lead" });
    expect(within(dialog).queryByText("Cache time")).toBeNull();
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Start lead" }),
    );
    const alert = await within(dialog).findByRole("alert");
    expect(alert.textContent).toContain(
      "A lead for the web area is already running.",
    );
    expect(alert.textContent).toContain("code: area_has_lead");
  });

  it("starts the review lead without an area", async () => {
    const calls = stubFetch(
      routes({
        [`POST ${BASE}/workers`]: {
          status: 201,
          body: workerProcess("review", { role: "review_lead", area: null }),
        },
        [`GET ${BASE}/workers`]: { body: team },
      }),
    );
    await setup([agent("main")], team);
    await userEvent.click(startButton());
    const dialog = screen.getByRole("dialog", { name: "Start a lead" });
    await userEvent.click(
      within(dialog).getByRole("radio", { name: "Review lead" }),
    );
    expect(within(dialog).queryByText("Area")).toBeNull();
    expect(
      within(dialog).getByText(/groups open proposals into review batches/),
    ).toBeTruthy();
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Start lead" }),
    );
    await waitFor(() => {
      expect(screen.queryByRole("dialog")).toBeNull();
    });
    expect(calls.find((c) => c.method === "POST")?.body).toEqual({
      role: "review_lead",
    });
  });

  it("asks for a permission answer and shows the daemon's answer", async () => {
    const question = permissionQuestion({ worker: "web-coder" });
    let answered = false;
    const calls = stubFetch(
      routes({
        [`GET ${BASE}/worker-questions`]: () => ({
          body: { items: answered ? [] : [question] },
        }),
        [`POST ${BASE}/worker-questions/pq-3/answer`]: () => {
          answered = true;
          return {
            body: {
              ...question,
              answer: {
                behavior: "allow",
                answered_by: "human:tanim",
                message: null,
                answered_at: Date.now(),
              },
            },
          };
        },
      }),
    );
    const onSearch = vi.fn();
    await setup([], team, { question: "pq-3" }, onSearch);
    const panel = screen.getByRole("complementary", {
      name: "Permission question",
    });
    expect(await within(panel).findByText("Bash")).toBeTruthy();
    expect(within(panel).getByText("Task coder")).toBeTruthy();
    expect(
      await within(row("web-coder")).findByRole("img", {
        name: "waits for a permission answer",
      }),
    ).toBeTruthy();
    await userEvent.click(within(panel).getByRole("button", { name: "Allow" }));
    const status = await within(panel).findByRole("status");
    expect(status.textContent).toContain("Allowed by @tanim");
    expect(calls.find((c) => c.method === "POST")?.body).toEqual({
      behavior: "allow",
    });
    await userEvent.click(within(panel).getByRole("button", { name: "Close" }));
    expect(onSearch).toHaveBeenCalledWith({});
  });

  it("says when the asked question is no longer open", async () => {
    stubFetch(routes());
    await setup([], team, { question: "pq-9" });
    const panel = screen.getByRole("complementary", {
      name: "Permission question",
    });
    expect(
      await within(panel).findByText(/no open request any more/),
    ).toBeTruthy();
  });

  it("shows a refused answer with its code", async () => {
    stubFetch(
      routes({
        [`GET ${BASE}/worker-questions`]: {
          body: {
            items: [permissionQuestion({ input: {} })],
          },
        },
        [`POST ${BASE}/worker-questions/pq-3/answer`]: errorReply(
          409,
          "permission_question_closed",
        ),
      }),
    );
    await setup([], team, { question: "pq-3" });
    const panel = screen.getByRole("complementary", {
      name: "Permission question",
    });
    await within(panel).findByText("Bash");
    await userEvent.click(within(panel).getByRole("button", { name: "Deny" }));
    const alert = await within(panel).findByRole("alert");
    expect(alert.textContent).toContain("code: permission_question_closed");
  });
});
