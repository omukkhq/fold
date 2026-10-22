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
import { NotificationBell } from "./NotificationBell";
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
      self: null,
    });
  });
}

async function renderBell(slug: string) {
  const root = createRootRoute({
    component: () => (
      <>
        <NotificationBell slug={slug} />
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

describe("NotificationBell", () => {
  it("names the unread count and resets it when the list opens", async () => {
    await renderBell("count");
    expect(screen.getByRole("button", { name: "Notifications" })).toBeTruthy();
    receive("count", moved(1, "T-12", "review"), true);
    receive("count", moved(2, "T-14", "done"), true);
    await userEvent.click(
      screen.getByRole("button", { name: "Notifications, 2 unread" }),
    );
    const list = await screen.findByRole("dialog", { name: "Notifications" });
    const items = within(list).getAllByRole("listitem");
    expect(items.map((item) => item.textContent)).toEqual([
      expect.stringContaining("T-14 moved to Done"),
      expect.stringContaining("T-12 moved to Review"),
    ]);
    expect(within(list).getByText("Add the bell")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Notifications" })).toBeTruthy();
  });

  it("opens the task of an entry", async () => {
    await renderBell("open");
    receive("open", moved(1, "T-12", "review"), true);
    await userEvent.click(
      screen.getByRole("button", { name: /^Notifications/ }),
    );
    await userEvent.click(
      await screen.findByRole("link", { name: /T-12 moved to Review/ }),
    );
    expect(await screen.findByText("Task screen T-12")).toBeTruthy();
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("clears one entry and then every entry", async () => {
    await renderBell("clear");
    receive("clear", moved(1, "T-12", "review"), true);
    receive("clear", moved(2, "T-14", "done"), true);
    await userEvent.click(
      screen.getByRole("button", { name: /^Notifications/ }),
    );
    await userEvent.click(
      await screen.findByRole("button", { name: "Clear T-14 moved to Done" }),
    );
    const list = screen.getByRole("dialog", { name: "Notifications" });
    expect(within(list).getAllByRole("listitem")).toHaveLength(1);
    await userEvent.click(
      within(list).getByRole("button", { name: "Clear all" }),
    );
    expect(within(list).queryAllByRole("listitem")).toHaveLength(0);
    expect(within(list).getByText("No card moves")).toBeTruthy();
    expect(
      within(list).queryByRole("button", { name: "Clear all" }),
    ).toBeNull();
  });

  it("shows one polite toast for the moves of one window", async () => {
    await renderBell("toast");
    receive("toast", moved(1, "T-12", "review"));
    receive("toast", moved(2, "T-14", "done"));
    const text = "T-12 “Add the bell” moved to Review · T-14 moved to Done";
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
