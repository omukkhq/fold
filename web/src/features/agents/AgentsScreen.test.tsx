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
import { afterEach, describe, expect, it } from "vitest";
import type { AgentView } from "@/api/agents";
import { keys } from "@/api/keys";
import { AgentsScreen } from "./AgentsScreen";

afterEach(cleanup);

async function setup(agents?: AgentView[]) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  if (agents) {
    queryClient.setQueryData(keys.agents("demo"), agents);
  }
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

describe("AgentsScreen", () => {
  it("shows the empty state before any agent connects", async () => {
    await setup([]);
    expect(
      screen.getByRole("region", { name: "No agent has connected" }),
    ).toBeTruthy();
  });

  it("separates stalled and stale, and links a review claim to its proposal", async () => {
    await setup();
    const backend = screen.getByRole("region", { name: "Agent backend" });
    expect(within(backend).getByText("Stalled")).toBeTruthy();
    expect(within(backend).getByText("Stale")).toBeTruthy();
    const review = screen.getByRole("region", { name: "Agent review" });
    expect(
      within(review)
        .getAllByRole("link", { name: "P-4" })[0]
        ?.getAttribute("href"),
    ).toBe("/p/demo/proposals/P-4");
  });

  it("grants integrator after the confirmation and shows the tag once the daemon answers", async () => {
    await setup();
    const frontend = screen.getByRole("region", { name: "Agent frontend" });
    expect(within(frontend).queryByText("integrator")).toBeNull();
    await userEvent.click(
      within(frontend).getByRole("button", { name: "Grant integrator" }),
    );
    const dialog = screen.getByRole("dialog", {
      name: "Grant integrator to frontend?",
    });
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Grant integrator" }),
    );
    expect(
      within(dialog)
        .getByRole("button", { name: "Grant integrator" })
        .getAttribute("aria-busy"),
    ).toBe("true");
    await waitFor(() => {
      expect(screen.queryByRole("dialog")).toBeNull();
    });
    expect(within(frontend).getByText("integrator")).toBeTruthy();
    expect(
      within(frontend).getByRole("button", { name: "Remove integrator" }),
    ).toBeTruthy();
  });
});
