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
  fireEvent,
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
import type { ThreadView } from "@/api/messages";
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
  closable: true,
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
  shown: ThreadView = thread,
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
  render(
    <QueryClientProvider client={new QueryClient()}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  await screen.findByLabelText("Reply");
}

describe("ThreadPanel", () => {
  it("marks the thread's asks answered and says when another human answered first", async () => {
    daemon.on("POST /api/v1/projects/demo/asks/answered", {
      status: 200,
      body: { answered: [] },
    });
    await setup(() => Promise.resolve());
    await userEvent.click(
      screen.getByRole("button", { name: "Mark answered" }),
    );
    expect(await screen.findByText("It was already answered.")).toBeTruthy();
    expect(
      daemon.made("POST /api/v1/projects/demo/asks/answered")[0]?.body,
    ).toEqual({ thread: MESSAGE });
  });

  it("offers only the reply box while only a worker's question waits", async () => {
    await setup(() => Promise.resolve(), { ...thread, closable: false });
    expect(screen.queryByRole("button", { name: "Mark answered" })).toBeNull();
  });

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

  it("says a message to humans was sent to them and marks the signed-in human", async () => {
    daemon.on("GET /api/v1/session", {
      status: 200,
      body: { human: "tanim", authenticated: true },
    });
    await setup(() => Promise.resolve(), {
      ...thread,
      messages: [
        message(MESSAGE, {
          sender: "web-worker",
          recipients: [recipient("human:tanim"), recipient("human:rafeed")],
          body: "Which one?",
          sent_at: NOW,
        }),
      ],
    });
    const lines = await screen.findAllByText("sent to", { exact: false });
    await waitFor(() => {
      expect(lines.map((line) => line.textContent)).toEqual([
        "sent to@tanim (you)",
        "sent to@rafeed",
      ]);
    });
    expect(screen.queryByText("not delivered to", { exact: false })).toBeNull();
  });

  it("refuses an empty reply", async () => {
    const onReply = vi.fn(() => Promise.resolve());
    await setup(onReply);
    await userEvent.click(screen.getByRole("button", { name: "Send" }));
    expect(await screen.findByText("Write a message first.")).toBeTruthy();
    expect(onReply).not.toHaveBeenCalled();
  });

  it("sends on Enter and starts a new line on Shift+Enter", async () => {
    const onReply = vi.fn(() => Promise.resolve());
    await setup(onReply);
    await userEvent.type(
      screen.getByLabelText("Reply"),
      "Use it{Shift>}{Enter}{/Shift}Recharts{Enter}",
    );
    expect(onReply).toHaveBeenCalledTimes(1);
    expect(onReply).toHaveBeenCalledWith(MESSAGE, "Use it\nRecharts");
  });

  it("does nothing on Enter in an empty box", async () => {
    const onReply = vi.fn(() => Promise.resolve());
    await setup(onReply);
    const box = screen.getByLabelText<HTMLTextAreaElement>("Reply");
    await userEvent.type(box, "  {Enter}");
    expect(onReply).not.toHaveBeenCalled();
    expect(box.value).toBe("  ");
    expect(screen.queryByText("Write a message first.")).toBeNull();
  });

  it("never sends the Enter of an input method", async () => {
    const onReply = vi.fn(() => Promise.resolve());
    await setup(onReply);
    const box = screen.getByLabelText("Reply");
    await userEvent.type(box, "kanji");
    fireEvent.keyDown(box, { key: "Enter", isComposing: true });
    // Safari ends the composition before the keydown of its Enter.
    fireEvent.compositionStart(box);
    fireEvent.compositionEnd(box);
    fireEvent.keyDown(box, { key: "Enter" });
    fireEvent.keyUp(box, { key: "Enter" });
    await userEvent.type(box, " ok{Enter}");
    await waitFor(() => {
      expect(onReply.mock.calls).toEqual([[MESSAGE, "kanji ok"]]);
    });
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
        screen
          .getByRole("button", { name: "Sending…" })
          .getAttribute("aria-busy"),
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
    const choices = screen.getByRole("list", { name: "Choices" });
    const marked = within(choices).getAllByRole("img", { name: "chosen" });
    expect(marked).toHaveLength(1);
    expect(marked[0]?.closest("button")?.textContent).toContain("Recharts");
    expect(marked[0]?.getAttribute("class")).toContain("text-success");
    for (const button of within(choices).getAllByRole("button")) {
      expect(button.className).not.toContain("opacity");
      expect(button.textContent).not.toContain("· chosen");
    }
    expect(
      screen.getByText("Chosen option of the question above."),
    ).toBeTruthy();
    expect(screen.queryByLabelText("Your own answer")).toBeNull();
  });

  it("answers on Enter in the own-answer box", async () => {
    const onReply = vi.fn(() => Promise.resolve());
    renderThread(asked, onReply);
    await userEvent.type(
      await screen.findByLabelText("Your own answer"),
      "Neither{Shift>}{Enter}{/Shift}Use SVG{Enter}",
    );
    expect(onReply).toHaveBeenCalledWith(QUESTION, "Neither\nUse SVG");
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

  it("quotes the lines a question to the author is about and links them to the proposal's diff", async () => {
    const asked = baseThread(MESSAGE, {
      proposal: "P-4",
      messages: [
        message(MESSAGE, {
          sender: "human:tanim",
          proposal: "P-4",
          recipients: [recipient("backend/db")],
          body: "Why two constants?",
          lines: {
            version: 2,
            path: "web/src/board.ts",
            side: "new",
            start_line: 7,
            end_line: 120,
            excerpt: "const a = 1;\nconst b = 2;",
            excerpt_cut: true,
          },
        }),
      ],
    });
    await setup(() => Promise.resolve(), asked);
    const quote = screen.getByRole("figure");
    expect(
      within(quote)
        .getByRole("link", {
          name: "web/src/board.ts · v2 · lines 7–120 · new side",
        })
        .getAttribute("href"),
    ).toBe("/p/demo/proposals/P-4?version=2&path=web%2Fsrc%2Fboard.ts");
    expect(within(quote).getByText("const b = 2;")).toBeTruthy();
    expect(within(quote).getByText("8")).toBeTruthy();
    expect(within(quote).getByText("The first 2 of 114 lines.")).toBeTruthy();
  });
});
