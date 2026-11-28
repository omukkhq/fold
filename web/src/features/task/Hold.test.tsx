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
    putOnHold: vi.fn(() => Promise.resolve()),
    takeOffHold: vi.fn(() => Promise.resolve()),
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

describe("the held-through group", () => {
  it("names each held task as a link, next to the manual hold", async () => {
    const task = taskView("T-2", { held_by: ["T-9", "T-3"], hold: HOLD });
    mount(<TaskSide slug="demo" task={task} live={null} now={0} />);
    const group = await screen.findByRole("region", { name: "Held through" });
    const links = within(group).getAllByRole("link");
    expect(links.map((link) => link.textContent)).toEqual(["T-3", "T-9"]);
    expect(links[0]?.getAttribute("href")).toBe("/p/demo/t/T-3");
    expect(screen.getByRole("region", { name: "On hold" })).toBeTruthy();
  });

  it("is absent without a held task", async () => {
    mount(<TaskSide slug="demo" task={taskView("T-2")} live={null} now={0} />);
    await screen.findByRole("region", { name: "Links" });
    expect(screen.queryByRole("region", { name: "Held through" })).toBeNull();
  });
});

describe("the hold group", () => {
  it("puts a task on hold with a reason", async () => {
    const task = taskView("T-2", { can_set_hold: true });
    mount(<TaskSide slug="demo" task={task} live={null} now={0} />);
    // Without a hold the Links group offers Put on hold, which opens the On hold row's form.
    const links = await screen.findByRole("region", { name: "Links" });
    expect(within(links).getByText(/not on hold/)).toBeTruthy();
    await userEvent.click(
      within(links).getByRole("button", { name: "Put on hold" }),
    );
    const group = screen.getByRole("region", { name: "On hold" });
    await userEvent.type(
      within(group).getByRole("textbox", { name: "Reason" }),
      "Wait for the logo",
    );
    const release = holdNext(seam.putOnHold);
    await userEvent.click(
      within(group).getByRole("button", { name: "Put on hold" }),
    );
    await waitFor(() => {
      expect(seam.putOnHold).toHaveBeenCalledWith(
        "demo",
        "T-2",
        "Wait for the logo",
      );
    });
    expect(
      within(group)
        .getByRole("button", { name: "Putting on hold…" })
        .getAttribute("aria-busy"),
    ).toBe("true");
    release();
  });

  it("shows the reason and takes the task off hold", async () => {
    const task = taskView("T-2", {
      can_set_hold: true,
      can_clear_hold: true,
      hold: HOLD,
    });
    mount(<TaskSide slug="demo" task={task} live={null} now={0} />);
    const group = await screen.findByRole("region", { name: "On hold" });
    expect(within(group).getByText("Wait for the logo")).toBeTruthy();
    const release = holdNext(seam.takeOffHold);
    await userEvent.click(
      within(group).getByRole("button", { name: "Take off hold" }),
    );
    await waitFor(() => {
      expect(seam.takeOffHold).toHaveBeenCalledWith("demo", "T-2");
    });
    expect(
      within(group)
        .getByRole("button", { name: "Taking off hold…" })
        .getAttribute("aria-busy"),
    ).toBe("true");
    release();
  });

  it("offers no control without a named human", async () => {
    const task = taskView("T-2", { hold: HOLD });
    mount(<TaskSide slug="demo" task={task} live={null} now={0} />);
    const group = await screen.findByRole("region", { name: "On hold" });
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
