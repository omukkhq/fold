// @vitest-environment jsdom
import * as RadixDialog from "@radix-ui/react-dialog";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  RouterProvider,
  createMemoryHistory,
  createRootRoute,
  createRouter,
} from "@tanstack/react-router";
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import type { TaskDetail } from "@/api/tasks";
import { taskView } from "@/test/payloads";
import { TitleBlock } from "./TitleBlock";

afterEach(cleanup);

async function mount(task: TaskDetail) {
  const root = createRootRoute({
    component: () => (
      <RadixDialog.Root open>
        <TitleBlock slug="demo" task={task} strip={null} />
      </RadixDialog.Root>
    ),
  });
  render(
    <QueryClientProvider client={new QueryClient()}>
      <RouterProvider
        router={createRouter({
          routeTree: root,
          history: createMemoryHistory({ initialEntries: ["/"] }),
        })}
      />
    </QueryClientProvider>,
  );
  await screen.findByText(task.title);
}

const proposal = {
  id: "P-7",
  title: "Add the form",
  review_claim: null,
  files: 3,
  state: "open" as const,
  version: 2,
  checks_failed: false,
  reviews: [],
};

describe("the proposal strip of the task dialog", () => {
  it("links to the proposal screen and says it waits for this human", async () => {
    await mount(
      taskView("T-1", {
        status: "review",
        proposal,
        attention: [{ kind: "human_review", closable: false }],
      }),
    );
    const link = screen.getByRole("link", { name: "Go to proposal" });
    expect(link.getAttribute("href")).toBe("/p/demo/proposals/P-7");
    expect(screen.getByText("P-7 v2 waits for your review")).toBeTruthy();
    expect(screen.getByLabelText("Proposal").className).toContain(
      "bg-secondary-soft",
    );
  });

  it("is neutral while a reviewer holds the version", async () => {
    await mount(
      taskView("T-1", {
        status: "review",
        proposal: { ...proposal, review_claim: "review-lead" },
      }),
    );
    expect(screen.getByText("P-7 v2 is with @review-lead")).toBeTruthy();
    expect(screen.getByLabelText("Proposal").className).toContain("bg-sunken");
  });

  it("is absent without an open proposal and outside Review", async () => {
    await mount(taskView("T-1", { status: "review" }));
    expect(screen.queryByRole("link", { name: "Go to proposal" })).toBeNull();
    cleanup();
    await mount(taskView("T-1", { status: "in_progress", proposal }));
    expect(screen.queryByRole("link", { name: "Go to proposal" })).toBeNull();
  });
});
