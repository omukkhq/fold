// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  RouterProvider,
  createMemoryHistory,
  createRootRoute,
  createRoute,
  createRouter,
} from "@tanstack/react-router";
import { cleanup, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { errorReply, stubFetch } from "@/test/stubFetch";
import { taskCost } from "@/test/cost";
import { TaskCostSection } from "./TaskCostSection";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

function show() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const root = createRootRoute();
  const index = createRoute({
    getParentRoute: () => root,
    path: "/",
    component: () => <TaskCostSection slug="demo" task="T-12" />,
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
}

describe("TaskCostSection", () => {
  it("shows the total, the build and review parts and each worker", async () => {
    stubFetch({
      "GET /api/v1/projects/demo/cost/tasks/T-12": { body: taskCost() },
    });
    show();
    const section = await screen.findByRole("region", { name: "Cost" });
    expect(await within(section).findByText("$5.00")).toBeTruthy();
    expect(within(section).getByText("Build $4.00")).toBeTruthy();
    expect(within(section).getByText("Review $1.00")).toBeTruthy();
    expect(
      within(section).getByText(
        "reviewer · unknown model · 6 turns · reviewed P-3, P-4",
      ),
    ).toBeTruthy();
    expect(
      within(section)
        .getByRole("link", { name: "Project cost" })
        .getAttribute("href"),
    ).toBe("/p/demo/agents?tab=cost");
  });

  it("says when no worker worked on the task or the read failed", async () => {
    stubFetch({
      "GET /api/v1/projects/demo/cost/tasks/T-12": {
        body: taskCost({ workers: [], total_micro_usd: 0 }),
      },
    });
    show();
    expect(await screen.findByText(/No worker that fold runs/)).toBeTruthy();
    cleanup();
    stubFetch({
      "GET /api/v1/projects/demo/cost/tasks/T-12": errorReply(
        404,
        "task_not_found",
      ),
    });
    show();
    expect(
      await screen.findByText(/could not read the task's cost/),
    ).toBeTruthy();
  });
});
