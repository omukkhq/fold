// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  RouterProvider,
  createMemoryHistory,
  createRootRoute,
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
import { keys } from "@/api/keys";
import * as seam from "@/api/tasks";
import { taskView } from "@/test/payloads";
import { TaskSide } from "./TaskSide";

// The task read never allows the change, so the dialog leaves these controls out; a task that allows it mounts them here.
vi.mock("@/api/tasks", async (importOriginal) => {
  const real = await importOriginal<typeof import("@/api/tasks")>();
  return {
    ...real,
    setHumanReview: vi.fn(() => Promise.resolve()),
    clearHumanReview: vi.fn(() => Promise.resolve()),
  };
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

function mount(task: seam.TaskDetail) {
  const client = new QueryClient();
  client.setQueryData(keys.session(), { human: "tanim", authenticated: true });
  const root = createRootRoute({
    component: () => <TaskSide slug="demo" task={task} live={null} now={0} />,
  });
  render(
    <QueryClientProvider client={client}>
      <RouterProvider
        router={createRouter({
          routeTree: root,
          history: createMemoryHistory({ initialEntries: ["/"] }),
        })}
      />
    </QueryClientProvider>,
  );
}

describe("the human-review controls", () => {
  it("turn the mark on with the signed-in reviewer and a reason", async () => {
    mount(taskView("T-2", { can_change_human_review: true }));
    const review = await screen.findByRole("region", { name: "Human review" });
    await userEvent.click(
      within(review).getByRole("button", { name: "Require my review" }),
    );
    await waitFor(() => {
      expect(
        within(review).getByRole<HTMLInputElement>("textbox", {
          name: "Reviewer",
        }).value,
      ).toBe("tanim");
    });
    await userEvent.type(
      within(review).getByRole("textbox", { name: "Reason" }),
      "Auth work",
    );
    await userEvent.click(
      within(review).getByRole("button", { name: "Require review" }),
    );
    await waitFor(() => {
      expect(seam.setHumanReview).toHaveBeenCalledWith("demo", "T-2", {
        reviewer: "tanim",
        reason: "Auth work",
      });
    });
  });

  it("turn the mark off", async () => {
    mount(
      taskView("T-2", {
        can_change_human_review: true,
        human_review: { reviewer: null },
      }),
    );
    const review = await screen.findByRole("region", { name: "Human review" });
    expect(within(review).getByText("Required · any human")).toBeTruthy();
    await userEvent.click(
      within(review).getByRole("button", { name: "Turn off human review" }),
    );
    await waitFor(() => {
      expect(seam.clearHumanReview).toHaveBeenCalledWith("demo", "T-2");
    });
  });
});
