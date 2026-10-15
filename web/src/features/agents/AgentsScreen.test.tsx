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
import { agent, claim, worker, workspace } from "@/test/agents";
import { stubFetch } from "@/test/stubFetch";
import { AgentsScreen } from "./AgentsScreen";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

async function setup(agents: AgentView[]) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  queryClient.setQueryData(keys.agents("demo"), agents);
  const root = createRootRoute();
  const index = createRoute({
    getParentRoute: () => root,
    path: "/",
    component: () => <AgentsScreen slug="demo" />,
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
}

const AGENTS_ROUTE = "/api/v1/projects/demo/agents";
const TAG_ROUTE = `${AGENTS_ROUTE}/frontend/tags/integrator`;

const frontend = agent("frontend", {
  last_call_at: Date.now() - 5000,
  workers: [
    worker("frontend", { activity: "list_tasks" }),
    worker("frontend/ui", {
      activity: "claim_task T-12",
      last_call_at: Date.now() - 5000,
      claims: [claim()],
      workspaces: [workspace({ changed_files: 3 })],
    }),
  ],
});
const review = agent("review", {
  workers: [
    worker("review", {
      claims: [claim({ task: "T-10", kind: "review", stale: true })],
    }),
  ],
});

describe("AgentsScreen", () => {
  it("shows the empty state before any agent connects", async () => {
    await setup([]);
    expect(
      screen.getByRole("region", { name: "No agent has connected" }),
    ).toBeTruthy();
  });

  it("shows presence, claims and workspaces from the daemon", async () => {
    await setup([frontend, review]);
    const card = screen.getByRole("region", { name: "Agent frontend" });
    expect(within(card).getByText(/active · 1 session/)).toBeTruthy();
    expect(within(card).getByText("T-12 · ws-7")).toBeTruthy();
    expect(
      within(card).getByText(/frontend\/ui · 3 changed files/),
    ).toBeTruthy();
    expect(within(card).queryByText("Stale")).toBeNull();
  });

  it("draws the session lines, the stalled badge and the granter only when the payload carries them", async () => {
    await setup([
      {
        ...frontend,
        tags: ["integrator"],
        open_sessions: [
          { started_at: 0, pid: 81231, cwd: "/home/tanim/ledger" },
        ],
        attention: [{ kind: "stalled", task: "T-12" }],
        integrator_granted_by: "human:tanim",
      },
      review,
    ]);
    const card = screen.getByRole("region", { name: "Agent frontend" });
    expect(
      within(card).getByText(/pid 81231 · \/home\/tanim\/ledger/),
    ).toBeTruthy();
    expect(within(card).getByText("Stalled")).toBeTruthy();
    expect(within(card).getByText(/integrator granted by tanim/)).toBeTruthy();
    const plain = screen.getByRole("region", { name: "Agent review" });
    expect(within(plain).queryByText(/pid/)).toBeNull();
    expect(within(plain).queryByText("Stalled")).toBeNull();
    expect(within(plain).queryByText(/granted by/)).toBeNull();
  });

  it("marks a stale claim and links a review claim to its task", async () => {
    await setup([frontend, review]);
    const card = screen.getByRole("region", { name: "Agent review" });
    expect(within(card).getByText("Stale")).toBeTruthy();
    expect(
      within(card)
        .getAllByRole("link", { name: "T-10" })[0]
        ?.getAttribute("href"),
    ).toBe("/p/demo/t/T-10");
  });

  it("grants integrator after the confirmation and shows the tag once the daemon answers", async () => {
    let granted = false;
    stubFetch({
      [`PUT ${TAG_ROUTE}`]: () => {
        granted = true;
        return { body: { agent: "frontend", tags: ["integrator"] } };
      },
      [`GET ${AGENTS_ROUTE}`]: () => ({
        body: {
          agents: [{ ...frontend, tags: granted ? ["integrator"] : [] }],
        },
      }),
    });
    await setup([frontend]);
    const card = screen.getByRole("region", { name: "Agent frontend" });
    expect(within(card).queryByText("integrator")).toBeNull();
    await userEvent.click(
      within(card).getByRole("button", { name: "Grant integrator" }),
    );
    const dialog = screen.getByRole("dialog", {
      name: "Grant integrator to frontend?",
    });
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Grant integrator" }),
    );
    await waitFor(() => {
      expect(screen.queryByRole("dialog")).toBeNull();
    });
    expect(await within(card).findByText("integrator")).toBeTruthy();
    expect(
      within(card).getByRole("button", { name: "Remove integrator" }),
    ).toBeTruthy();
  });

  it("keeps the dialog open and says so when the daemon refuses", async () => {
    stubFetch({
      [`PUT ${TAG_ROUTE}`]: {
        status: 404,
        body: { error: { code: "agent_not_found", message: "", details: {} } },
      },
    });
    await setup([frontend]);
    await userEvent.click(
      screen.getByRole("button", { name: "Grant integrator" }),
    );
    const dialog = screen.getByRole("dialog");
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Grant integrator" }),
    );
    expect(await within(dialog).findByRole("alert")).toBeTruthy();
    expect(within(dialog).getByRole("alert").textContent).toContain(
      "fold has no agent with this name.",
    );
  });
});
