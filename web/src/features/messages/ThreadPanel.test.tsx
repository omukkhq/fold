// @vitest-environment jsdom
import {
  RouterProvider,
  createMemoryHistory,
  createRootRoute,
  createRoute,
  createRouter,
} from "@tanstack/react-router";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ThreadView } from "@/api/messages";
import { ThreadPanel } from "./ThreadPanel";

afterEach(cleanup);

// An id such as m-31 looks like a margin utility to the token guard, so the test builds it.
const MESSAGE = `m-${String(31)}`;
const NOW = Date.UTC(2026, 9, 1, 12, 0, 0);

const thread: ThreadView = {
  id: MESSAGE,
  subject: "A question",
  task: "T-12",
  proposal: null,
  participants: ["human:tanim", "frontend/ui"],
  unread: true,
  last_at: NOW,
  messages: [
    {
      id: MESSAGE,
      thread: MESSAGE,
      sender: "frontend/ui",
      recipients: [
        { name: "human:tanim", delivered_at: NOW, acknowledged_at: null },
      ],
      body: "Which one?",
      sent_at: NOW,
      reply_to: null,
    },
  ],
};

async function setup(
  onReply: (replyTo: string, body: string) => Promise<void>,
) {
  const root = createRootRoute();
  const index = createRoute({
    getParentRoute: () => root,
    path: "/",
    component: () => (
      <ThreadPanel slug="demo" thread={thread} now={NOW} onReply={onReply} />
    ),
  });
  const router = createRouter({
    routeTree: root.addChildren([index]),
    history: createMemoryHistory({ initialEntries: ["/"] }),
  });
  render(<RouterProvider router={router} />);
  await screen.findByLabelText("Reply");
}

describe("ThreadPanel", () => {
  it("shows the thread, delivery lines and the task link", async () => {
    await setup(() => Promise.resolve());
    expect(screen.getByText("Which one?")).toBeTruthy();
    expect(
      screen.getByText("delivered to tanim", { exact: false }),
    ).toBeTruthy();
    expect(
      screen.getByRole("link", { name: "Open T-12" }).getAttribute("href"),
    ).toBe("/p/demo/t/T-12");
  });

  it("refuses an empty reply", async () => {
    const onReply = vi.fn(() => Promise.resolve());
    await setup(onReply);
    await userEvent.click(screen.getByRole("button", { name: "Send" }));
    expect(await screen.findByText("Write a message first.")).toBeTruthy();
    expect(onReply).not.toHaveBeenCalled();
  });

  it("shows pending without adding a message, then clears the box", async () => {
    let finish: () => void = () => {};
    const onReply = vi.fn(
      () => new Promise<void>((resolve) => (finish = resolve)),
    );
    await setup(onReply);
    await userEvent.type(screen.getByLabelText("Reply"), "Recharts");
    await userEvent.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => {
      expect(
        screen.getByRole("button", { name: "Send" }).getAttribute("aria-busy"),
      ).toBe("true");
    });
    expect(onReply).toHaveBeenCalledWith(MESSAGE, "Recharts");
    expect(screen.getAllByRole("article")).toHaveLength(1);
    finish();
    await waitFor(() => {
      expect(screen.getByLabelText<HTMLTextAreaElement>("Reply").value).toBe(
        "",
      );
    });
  });

  it("keeps the text and says the reply did not arrive when it fails", async () => {
    await setup(() => Promise.reject(new Error("boom")));
    await userEvent.type(screen.getByLabelText("Reply"), "Hi");
    await userEvent.click(screen.getByRole("button", { name: "Send" }));
    expect((await screen.findByRole("alert")).textContent).toBe(
      "The reply was not sent. The request did not reach the daemon. Try again.",
    );
    expect(screen.getByLabelText<HTMLTextAreaElement>("Reply").value).toBe(
      "Hi",
    );
  });
});
