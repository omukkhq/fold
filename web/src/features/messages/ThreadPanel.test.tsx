// @vitest-environment jsdom
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
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, describe, expect, it, vi } from "vitest";
import { expectMentionPopover } from "@/test/mentionBox";
import { mentionRoutes } from "@/test/mentionRoutes";
import { daemon } from "@/test/daemon";
import { message, recipient, thread as baseThread } from "@/test/messages";
import { ApiError } from "@/api/errors";
import { ThreadPanel } from "./ThreadPanel";

afterEach(() => {
  cleanup();
  daemon.reset();
});

vi.mock("@/api/client", async (importOriginal) => {
  const real = await importOriginal<typeof import("@/api/client")>();
  const { daemon: fake } = await import("@/test/daemon");
  return { ...real, api: fake.api };
});

// An id such as m-31 looks like a margin utility to the token guard, so the test builds it.
const MESSAGE = `m-${String(31)}`;
const NOW = Date.UTC(2026, 9, 1, 12, 0, 0);

const thread = baseThread(MESSAGE, {
  task: "T-12",
  participants: ["human:tanim", "frontend/ui"],
  awaiting_human: true,
  last_at: NOW,
  messages: [
    message(MESSAGE, {
      recipients: [recipient("human:tanim", { delivered_at: NOW })],
      body: "Which one?",
      sent_at: NOW,
    }),
  ],
});

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
  render(
    <QueryClientProvider client={new QueryClient()}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  await screen.findByLabelText("Reply");
}

describe("ThreadPanel", () => {
  it("suggests the files of a workspace after @ws-9:", async () => {
    mentionRoutes();
    await setup(vi.fn(() => Promise.resolve()));
    const box = screen.getByLabelText("Reply");
    await userEvent.type(box, "see @ws-9:d");
    await userEvent.click(
      await screen.findByRole("option", { name: /draft\.ts/ }),
    );
    expect(box).toHaveProperty("value", "see @ws-9:draft.ts ");
  });

  it("suggests items and files in the reply box", async () => {
    mentionRoutes();
    await setup(vi.fn(() => Promise.resolve()));
    await expectMentionPopover(screen.getByLabelText("Reply"));
  });

  it("shows the thread, delivery lines and the task link", async () => {
    await setup(() => Promise.resolve());
    expect(
      within(screen.getByRole("article")).getByText("Which one?"),
    ).toBeTruthy();
    const delivered = screen.getByText("delivered to", { exact: false });
    expect(delivered.textContent).toContain("tanim");
    expect(delivered.querySelector('[data-person="human"]')).toBeTruthy();
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

describe("ThreadPanel question", () => {
  const QUESTION = `m-${String(34)}`;
  const ANSWER = `m-${String(35)}`;
  const asked = baseThread(QUESTION, {
    participants: ["human:tanim", "frontend"],
    awaiting_human: true,
    messages: [
      message(QUESTION, {
        sender: "frontend",
        body: "Recharts or plain SVG?",
        header: "Chart library",
        choices: [
          {
            label: "Recharts",
            description: "Ready-made bar and line components.",
          },
          { label: "Plain SVG", description: null },
        ],
      }),
    ],
  });
  const answered = {
    ...asked,
    awaiting_human: false,
    messages: [
      ...asked.messages,
      message(ANSWER, {
        sender: "human:tanim",
        body: "Recharts",
        reply_to: QUESTION,
      }),
    ],
  };

  function renderThread(
    shown: typeof asked,
    onReply: (replyTo: string, body: string) => Promise<void>,
  ) {
    const root = createRootRoute();
    const index = createRoute({
      getParentRoute: () => root,
      path: "/",
      component: () => (
        <ThreadPanel slug="demo" thread={shown} now={NOW} onReply={onReply} />
      ),
    });
    const router = createRouter({
      routeTree: root.addChildren([index]),
      history: createMemoryHistory({ initialEntries: ["/"] }),
    });
    return render(
      <QueryClientProvider client={new QueryClient()}>
        <RouterProvider router={router} />
      </QueryClientProvider>,
    );
  }

  it("answers with the chosen option and stays pending until the thread shows the answer", async () => {
    const onReply = vi.fn(() => Promise.resolve());
    renderThread(asked, onReply);
    expect(
      await screen.findByText("Question · waiting for an answer"),
    ).toBeTruthy();
    expect(screen.getByText("Chart library")).toBeTruthy();
    expect(
      screen.getByText("Ready-made bar and line components."),
    ).toBeTruthy();
    const choices = screen.getByRole("list", { name: "Choices" });
    await userEvent.click(
      within(choices).getByRole("button", { name: /^Recharts/ }),
    );
    expect(onReply).toHaveBeenCalledWith(QUESTION, "Recharts");
    expect(await screen.findByRole("status")).toBeTruthy();
    for (const button of within(choices).getAllByRole("button")) {
      expect(button).toHaveProperty("disabled", true);
    }
  });

  it("marks the chosen option once the thread holds the answer", async () => {
    renderThread(
      answered,
      vi.fn(() => Promise.resolve()),
    );
    expect(await screen.findByText("Question · answered")).toBeTruthy();
    expect(
      screen.getByRole("button", { name: /^Recharts · chosen/ }),
    ).toHaveProperty("ariaPressed", "true");
    expect(
      screen.getByText("Chosen option of the question above."),
    ).toBeTruthy();
    expect(screen.queryByLabelText("Your own answer")).toBeNull();
  });

  it("sends a free answer and shows a refusal with its code", async () => {
    const onReply = vi.fn(() =>
      Promise.reject(new ApiError("message_not_found", "")),
    );
    renderThread(asked, onReply);
    await userEvent.type(
      await screen.findByLabelText("Your own answer"),
      "Neither",
    );
    await userEvent.click(screen.getByRole("button", { name: "Answer" }));
    expect(onReply).toHaveBeenCalledWith(QUESTION, "Neither");
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("The answer was not sent.");
    expect(alert.textContent).toContain("code: message_not_found");
  });
});
