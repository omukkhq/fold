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
import type { ReactNode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import * as seam from "@/api/tasks";
import { taskView } from "@/test/payloads";
import { HumanWork } from "./HumanWork";
import { TaskSide } from "./TaskSide";

vi.mock("@/api/tasks", async (importOriginal) => {
  const real = await importOriginal<typeof import("@/api/tasks")>();
  return {
    ...real,
    setBlocker: vi.fn(() => Promise.resolve()),
    clearBlocker: vi.fn(() => Promise.resolve()),
    completeTask: vi.fn(() => Promise.resolve()),
  };
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

function mount(node: ReactNode) {
  const root = createRootRoute({ component: () => node });
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
}

/** Holds the next call of a mocked write until `release` runs, so a test sees the control while it is pending. */
function holdNext(write: (...args: never[]) => Promise<unknown>) {
  let release = () => {};
  vi.mocked(write).mockImplementationOnce(
    () => new Promise<void>((resolve) => (release = resolve)),
  );
  return () => {
    release();
  };
}

const HOLD = { reason: "Wait for the logo", set_by: "web-lead", set_at: 0 };

describe("the blocker group", () => {
  it("sets a blocker with a reason", async () => {
    const task = taskView("T-2", { can_set_blocker: true });
    mount(<TaskSide slug="demo" task={task} live={null} now={0} />);
    const group = await screen.findByRole("region", { name: "Blocker" });
    await userEvent.click(
      within(group).getByRole("button", { name: "Set blocker" }),
    );
    await userEvent.type(
      within(group).getByRole("textbox", { name: "Reason" }),
      "Wait for the logo",
    );
    const release = holdNext(seam.setBlocker);
    await userEvent.click(
      within(group).getByRole("button", { name: "Set blocker" }),
    );
    await waitFor(() => {
      expect(seam.setBlocker).toHaveBeenCalledWith(
        "demo",
        "T-2",
        "Wait for the logo",
      );
    });
    expect(
      within(group)
        .getByRole("button", { name: "Setting blocker…" })
        .getAttribute("aria-busy"),
    ).toBe("true");
    release();
  });

  it("shows the reason and clears the blocker", async () => {
    const task = taskView("T-2", {
      can_set_blocker: true,
      can_clear_blocker: true,
      blocker: HOLD,
    });
    mount(<TaskSide slug="demo" task={task} live={null} now={0} />);
    const group = await screen.findByRole("region", { name: "Blocker" });
    expect(within(group).getByText("Wait for the logo")).toBeTruthy();
    const release = holdNext(seam.clearBlocker);
    await userEvent.click(
      within(group).getByRole("button", { name: "Clear blocker" }),
    );
    await waitFor(() => {
      expect(seam.clearBlocker).toHaveBeenCalledWith("demo", "T-2");
    });
    expect(
      within(group)
        .getByRole("button", { name: "Clearing blocker…" })
        .getAttribute("aria-busy"),
    ).toBe("true");
    release();
  });

  it("offers no control without a named human", async () => {
    const task = taskView("T-2", { blocker: HOLD });
    mount(<TaskSide slug="demo" task={task} live={null} now={0} />);
    const group = await screen.findByRole("region", { name: "Blocker" });
    expect(within(group).queryByRole("button")).toBeNull();
  });
});

describe("the human task control", () => {
  it("marks the work Done", async () => {
    const task = taskView("T-3", { kind: "human", can_mark_done: true });
    mount(<HumanWork slug="demo" task={task} />);
    const section = await screen.findByRole("region", { name: "Human task" });
    const release = holdNext(seam.completeTask);
    await userEvent.click(
      within(section).getByRole("button", { name: "Mark Done" }),
    );
    await waitFor(() => {
      expect(seam.completeTask).toHaveBeenCalledWith("demo", "T-3");
    });
    expect(
      within(section)
        .getByRole("button", { name: "Marking Done…" })
        .getAttribute("aria-busy"),
    ).toBe("true");
    release();
  });

  it("draws nothing for a task an agent works on", async () => {
    mount(
      <>
        <p>ready</p>
        <HumanWork slug="demo" task={taskView("T-1")} />
      </>,
    );
    await screen.findByText("ready");
    expect(screen.queryByRole("region", { name: "Human task" })).toBeNull();
  });
});
