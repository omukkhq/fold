// @vitest-environment jsdom
import {
  Outlet,
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
import { afterEach, beforeAll, describe, expect, it } from "vitest";
import type { TaskStatus } from "@/api/board";
import type { EventEnvelope } from "@/live/envelope";
import { MoveToasts } from "./MoveToasts";
import { notificationStore } from "./store";

beforeAll(() => {
  Element.prototype.hasPointerCapture = () => false;
  Element.prototype.releasePointerCapture = () => {};
});
afterEach(cleanup);

function moved(seq: number, task: string, to: TaskStatus): EventEnvelope {
  return {
    kind: "event",
    seq,
    type: "task.status_changed",
    actor: "fold",
    task,
    proposal: null,
    workspace: null,
    revision: null,
    data: { from: "in_progress", to },
    created_at: Date.now(),
  };
}

function receive(slug: string, event: EventEnvelope, replay = false) {
  act(() => {
    notificationStore.receive(slug, event, {
      replay,
      title: (task) => (task === "T-12" ? "Add the bell" : null),
    });
  });
}

async function renderToasts(slug: string) {
  const root = createRootRoute({
    component: () => (
      <>
        <MoveToasts slug={slug} />
        <Outlet />
      </>
    ),
  });
  const index = createRoute({
    getParentRoute: () => root,
    path: "/",
    component: () => <p>Home</p>,
  });
  const task = createRoute({
    getParentRoute: () => root,
    path: "/p/$slug/t/$task",
    component: function TaskScreen() {
      const { task: id } = task.useParams();
      return <p>Task screen {id}</p>;
    },
  });
  const router = createRouter({
    routeTree: root.addChildren([index, task]),
    history: createMemoryHistory({ initialEntries: ["/"] }),
  });
  render(<RouterProvider router={router} />);
  await screen.findByText("Home");
}

describe("MoveToasts", () => {
  it("keeps a phone's bottom action bar free and sits at the bottom right from md", async () => {
    await renderToasts("place");
    const region = screen.getByRole("region", { name: "Card moves (F8)" });
    const list = region.querySelector("ol") ?? region;
    const classes = list.className.split(" ");
    expect(classes).toContain("top-(--height-bar-compact)");
    expect(classes).not.toContain("bottom-16");
    expect(classes).toContain("md:bottom-16");
    expect(classes).toContain("md:top-auto");
  });

  it("shows one polite toast for the moves of one window", async () => {
    await renderToasts("toast");
    receive("toast", moved(1, "T-12", "review"));
    receive("toast", moved(2, "T-14", "done"));
    const text = "1 card moved to Review, 1 moved to Done";
    const region = screen.getByRole("region", { name: "Card moves (F8)" });
    await waitFor(
      () => {
        expect(within(region).getAllByRole("listitem")).toHaveLength(1);
      },
      { timeout: 3_000 },
    );
    expect(within(region).getByRole("listitem").textContent).toContain(text);
    await waitFor(() => {
      const polite = [
        ...document.querySelectorAll<HTMLElement>('[aria-live="polite"]'),
      ];
      expect(polite.some((region) => region.textContent.includes(text))).toBe(
        true,
      );
    });
    await userEvent.click(screen.getByRole("button", { name: "Dismiss" }));
    await waitFor(() => {
      expect(within(region).queryAllByRole("listitem")).toHaveLength(0);
    });
  });
});
