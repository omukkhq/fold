// @vitest-environment jsdom
import {
  RouterProvider,
  createMemoryHistory,
  createRootRoute,
  createRoute,
  createRouter,
} from "@tanstack/react-router";
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { expectMentionPopover } from "@/test/mentionBox";
import { mentionRoutes } from "@/test/mentionRoutes";
import { daemon, refuse } from "@/test/daemon";
import { message, recipient, thread as baseThread } from "@/test/messages";
import { ApiError } from "@/api/errors";
import type { ThreadView } from "@/api/messages";
import { formatAsOfMinute } from "@/format/time";
import { ChatPanel } from "./ChatPanel";
import { groupChats, type AgentChat } from "./chats";

/** The one chat of a thread's agent. */
function chatOf(...threads: ThreadView[]): AgentChat {
  const [chat] = groupChats(threads);
  if (!chat) throw new Error("no chat");
  return chat;
}

afterEach(() => {
  cleanup();
  daemon.reset();
  vi.unstubAllGlobals();
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
  onScreen = true,
  open: ReadonlySet<string> = new Set(),
) {
  const root = createRootRoute();
  const index = createRoute({
    getParentRoute: () => root,
    path: "/",
    component: () => (
      <ChatPanel
        slug="demo"
        chat={chatOf(shown)}
        now={NOW}
        open={open}
        onReply={onReply}
        onAnswer={() => Promise.resolve()}
        shown={onScreen}
      />
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

describe("ChatPanel", () => {
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

  it("shows a refused mark under the thread's bar, so a long error never squeezes the title", async () => {
    daemon.on(
      "POST /api/v1/projects/demo/asks/answered",
      refuse(409, "ask_closed"),
    );
    await setup(() => Promise.resolve());
    await userEvent.click(
      screen.getByRole("button", { name: "Mark answered" }),
    );
    const alert = await screen.findByRole("alert");
    expect(alert.closest("header")).toBeNull();
    expect(alert.parentElement?.previousElementSibling?.tagName).toBe("HEADER");
  });

  it("names the keys in the reply placeholder only on the wide layout", async () => {
    await setup(() => Promise.resolve());
    expect(screen.getByLabelText("Reply").getAttribute("placeholder")).toBe(
      "Reply to @frontend/ui…",
    );
    cleanup();
    vi.stubGlobal(
      "matchMedia",
      vi.fn(() => ({
        matches: true,
        addEventListener: vi.fn(),
        removeEventListener: vi.fn(),
      })),
    );
    await setup(() => Promise.resolve());
    expect(screen.getByLabelText("Reply").getAttribute("placeholder")).toBe(
      "Reply to @frontend/ui…  Enter sends, Shift+Enter adds a line",
    );
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

  it("shows the chat, delivery lines and the task links of the bar and of each message", async () => {
    await setup(() => Promise.resolve());
    const article = screen.getByRole("article");
    expect(within(article).getByText("Which one?")).toBeTruthy();
    const delivered = screen.getByText("delivered to", { exact: false });
    expect(delivered.textContent).toContain("tanim");
    expect(delivered.querySelector('[data-person="human"]')).toBeTruthy();
    const header = document.querySelector("header");
    if (!header) throw new Error("no header");
    expect(
      within(header).getByRole("link", { name: "T-12" }).getAttribute("href"),
    ).toBe("/p/demo/t/T-12");
    const chip = within(article).getByRole("link", { name: "T-12" });
    expect(chip.getAttribute("href")).toBe("/p/demo/t/T-12");
    expect(chip.className).toContain("whitespace-nowrap");
    expect(chip.className).not.toContain("font-mono");
  });

  it("says a message to humans was delivered, marks the signed-in human and marks the thread read for them", async () => {
    daemon.on("GET /api/v1/session", {
      status: 200,
      body: { human: "tanim", authenticated: true },
    });
    daemon.on(`POST /api/v1/projects/demo/threads/${MESSAGE}/read`, {
      status: 200,
      body: { read: [MESSAGE] },
    });
    await setup(() => Promise.resolve(), {
      ...thread,
      messages: [
        message(MESSAGE, {
          sender: "web-worker",
          recipients: [recipient("human:rafeed"), recipient("human:tanim")],
          body: "Which one?",
          sent_at: NOW,
        }),
      ],
    });
    const article = screen.getByRole("article");
    await waitFor(() => {
      expect(article.textContent).toContain("to you and @rafeed");
    });
    expect(article.textContent).toContain(
      `Delivered to @rafeed · ${formatAsOfMinute(NOW, NOW)}|Delivered to you · ${formatAsOfMinute(NOW, NOW)}`,
    );
    expect(screen.getByText("Today")).toBeTruthy();
    expect(screen.queryByText("not delivered to", { exact: false })).toBeNull();
    expect(screen.queryByRole("button", { name: "Details" })).toBeNull();
    await waitFor(() => {
      expect(
        daemon.made(`POST /api/v1/projects/demo/threads/${MESSAGE}/read`),
      ).toHaveLength(1);
    });
  });

  it("marks nothing read while the pane is hidden, and marks it once the tab comes back", async () => {
    daemon.on("GET /api/v1/session", {
      status: 200,
      body: { human: "tanim", authenticated: true },
    });
    daemon.on(`POST /api/v1/projects/demo/threads/${MESSAGE}/read`, {
      status: 200,
      body: { read: [MESSAGE] },
    });
    const read = `POST /api/v1/projects/demo/threads/${MESSAGE}/read`;
    await setup(() => Promise.resolve(), thread, false);
    await waitFor(() => {
      expect(screen.getByRole("article").textContent).toContain("to you");
    });
    expect(daemon.made(read)).toHaveLength(0);
    cleanup();
    const state = vi
      .spyOn(document, "visibilityState", "get")
      .mockReturnValue("hidden");
    await setup(() => Promise.resolve());
    await waitFor(() => {
      expect(screen.getByRole("article").textContent).toContain("to you");
    });
    expect(daemon.made(read)).toHaveLength(0);
    state.mockReturnValue("visible");
    act(() => {
      document.dispatchEvent(new Event("visibilitychange"));
    });
    await waitFor(() => {
      expect(daemon.made(read)).toHaveLength(1);
    });
    state.mockRestore();
  });

  it("folds the receipts of many recipients and opens the full list in place", async () => {
    await setup(() => Promise.resolve(), {
      ...thread,
      messages: [
        message(MESSAGE, {
          sender: "main",
          recipients: [
            recipient("backend", { pushed_at: NOW }),
            recipient("frontend", { acknowledged_at: NOW }),
            recipient("review", { acknowledged_at: NOW }),
          ],
          body: "Decisions.",
          sent_at: NOW,
        }),
      ],
    });
    const details = screen.getByRole("button", { name: "Details" });
    expect(details.getAttribute("aria-expanded")).toBe("false");
    expect(details.parentElement?.textContent).toContain(
      "Waiting for @backend · acknowledged by 2",
    );
    await userEvent.click(details);
    expect(details.getAttribute("aria-expanded")).toBe("true");
    const list = screen.getByRole("list", { name: "Delivery" });
    expect(within(list).getAllByRole("listitem")).toHaveLength(3);
    expect(
      within(list).getByText("it is busy until its current step ends", {
        exact: false,
      }),
    ).toBeTruthy();
  });

  it("draws your own message as a bubble and another human's under their initial", async () => {
    daemon.on("GET /api/v1/session", {
      status: 200,
      body: { human: "tanim", authenticated: true },
    });
    const OTHER = `m-${String(32)}`;
    await setup(() => Promise.resolve(), {
      ...thread,
      messages: [
        message(MESSAGE, {
          sender: "human:tanim",
          body: "Mine.",
          sent_at: NOW,
        }),
        message(OTHER, {
          sender: "human:rafeed",
          body: "Theirs.",
          sent_at: NOW,
        }),
      ],
    });
    await screen.findByText("You");
    const [own, other] = screen.getAllByRole("article");
    expect(own?.textContent).toContain("Mine.");
    expect(own?.closest("li")?.className).toContain("justify-end");
    expect(other?.textContent).toContain("rafeed");
    expect(other?.closest("li")?.textContent).toContain("R");
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
    fireEvent.compositionStart(box);
    fireEvent.keyDown(box, { key: "Enter" });
    // Safari ends the composition before the keydown of its Enter, which carries keyCode 229.
    fireEvent.compositionEnd(box);
    fireEvent.keyDown(box, { key: "Enter", keyCode: 229 });
    await userEvent.type(box, " ok{Enter}");
    await waitFor(() => {
      expect(onReply.mock.calls).toEqual([[MESSAGE, "kanji ok"]]);
    });
  });

  it("sends on the first Enter after a composition the mouse ended", async () => {
    const onReply = vi.fn(() => Promise.resolve());
    await setup(onReply);
    const box = screen.getByLabelText("Reply");
    await userEvent.type(box, "kanji");
    fireEvent.compositionStart(box);
    fireEvent.compositionEnd(box);
    fireEvent.keyDown(box, { key: "Enter", keyCode: 13 });
    await waitFor(() => {
      expect(onReply.mock.calls).toEqual([[MESSAGE, "kanji"]]);
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

  it("sends the same text again from the failure line", async () => {
    const onReply = vi
      .fn<(replyTo: string, body: string) => Promise<void>>()
      .mockRejectedValueOnce(new Error("boom"))
      .mockResolvedValueOnce(undefined);
    await setup(onReply);
    await userEvent.type(screen.getByLabelText("Reply"), "Hi");
    await userEvent.click(screen.getByRole("button", { name: "Send" }));
    await userEvent.click(
      await screen.findByRole("button", { name: "Send again" }),
    );
    expect(onReply.mock.calls).toEqual([
      [MESSAGE, "Hi"],
      [MESSAGE, "Hi"],
    ]);
    await waitFor(() => {
      expect(screen.queryByRole("alert")).toBeNull();
    });
  });

  it("keeps the text and says the reply did not arrive when it fails", async () => {
    await setup(() => Promise.reject(new Error("boom")));
    await userEvent.type(screen.getByLabelText("Reply"), "Hi");
    await userEvent.click(screen.getByRole("button", { name: "Send" }));
    expect((await screen.findByRole("alert")).textContent).toBe(
      "Not sent: The request did not reach the daemon. Try again.",
    );
    expect(screen.getByLabelText<HTMLTextAreaElement>("Reply").value).toBe(
      "Hi",
    );
  });
});

describe("ChatPanel question set", () => {
  const SET = `m-${String(34)}`;
  const LATER = `m-${String(36)}`;
  const toYou = [recipient("human:tanim")];
  const questions = [
    {
      text: "Is the MCP server connected again?",
      header: "MCP server",
      choices: [
        { label: "Yes, it is reconnected", description: "Since 16:40." },
        { label: "Not yet", description: null },
      ],
    },
    {
      text: "Which keys must the workers use?",
      header: null,
      choices: [
        { label: "Production keys", description: null },
        { label: "Mock the calls", description: null },
      ],
    },
    { text: "Do they run the e2e suite first?", header: null, choices: [] },
  ];
  const asked = baseThread(SET, {
    task: "T-220",
    participants: ["human:tanim", "backend-lead"],
    awaiting_human: true,
    closable: true,
    messages: [
      message(SET, {
        sender: "backend-lead",
        recipients: toYou,
        task: "T-220",
        body: "Both workers wait for the MCP server.",
        questions,
        sent_at: NOW - 1000,
      }),
    ],
  });
  const later = baseThread(LATER, {
    participants: ["human:tanim", "backend-lead"],
    awaiting_human: true,
    messages: [
      message(LATER, {
        sender: "backend-lead",
        recipients: toYou,
        body: "Another thing.",
        questions: [{ text: "Is Pencil open now?", header: null, choices: [] }],
        asks_human: true,
        sent_at: NOW,
      }),
    ],
  });

  function renderSets(
    threads: ThreadView[],
    open: string[],
    onAnswer: (ask: string, answers: string[]) => Promise<void>,
  ) {
    const root = createRootRoute();
    const index = createRoute({
      getParentRoute: () => root,
      path: "/",
      component: () => (
        <ChatPanel
          slug="demo"
          chat={chatOf(...threads)}
          now={NOW}
          open={new Set(open)}
          onReply={() => Promise.resolve()}
          onAnswer={onAnswer}
        />
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

  const card = () => screen.getByRole("region", { name: "Open question set" });
  const choice = (name: RegExp) =>
    within(within(card()).getByRole("list", { name: "Choices" })).getByRole(
      "button",
      { name },
    );

  it("says under the reply box that a reply does not answer the open set", async () => {
    renderSets([asked], [SET], () => Promise.resolve());
    expect(
      await screen.findByText(
        "Your reply does not answer the open set; answer it in the card.",
      ),
    ).toBeTruthy();
  });

  it("shows one question at a time with its step, and a choice or its number key shows the next", async () => {
    renderSets([asked], [SET], () => Promise.resolve());
    await screen.findByLabelText("Reply");
    expect(card().textContent).toContain("From");
    expect(within(card()).getByRole("link", { name: "T-220" })).toBeTruthy();
    expect(within(card()).getByText("MCP server")).toBeTruthy();
    expect(within(card()).getByText("Since 16:40.")).toBeTruthy();
    expect(within(card()).getByText("1 of 3")).toBeTruthy();
    expect(
      within(card()).getByRole("heading", {
        name: "Question 1 of 3: Is the MCP server connected again?",
      }),
    ).toBeTruthy();
    expect(within(card()).queryByRole("button", { name: "Back" })).toBeNull();
    expect(screen.queryByText("Which keys must the workers use?")).toBeNull();
    await userEvent.click(choice(/^Yes, it is reconnected/));
    const second = within(card()).getByRole("heading", {
      name: "Question 2 of 3: Which keys must the workers use?",
    });
    expect(document.activeElement).toBe(second);
    // The script moves the focus there for screen readers; the heading draws no ring.
    expect(second.className).toContain("outline-none");
    expect(second.className).not.toContain("focus-visible:outline-2");
    await userEvent.keyboard("2");
    expect(
      within(card()).getByRole("heading", {
        name: "Question 3 of 3: Do they run the e2e suite first?",
      }),
    ).toBeTruthy();
    expect(
      within(card()).queryByRole("list", { name: "Choices" }),
    ).toBeTruthy();
    await userEvent.click(within(card()).getByRole("button", { name: "Back" }));
    expect(
      within(card()).getByRole("heading", {
        name: "Question 2 of 3: Which keys must the workers use?",
      }),
    ).toBeTruthy();
    expect(choice(/^Mock the calls/).getAttribute("aria-pressed")).toBe("true");
  });

  it("takes an own answer with Enter and sends every answer as one reply, pending until the set closes", async () => {
    const onAnswer = vi.fn(() => Promise.resolve());
    renderSets([asked], [SET], onAnswer);
    await screen.findByLabelText("Reply");
    await userEvent.click(choice(/^Not yet/));
    await userEvent.click(
      within(card()).getByRole("button", { name: "Write your own answer…" }),
    );
    const own = within(card()).getByLabelText("Your own answer");
    expect(document.activeElement).toBe(own);
    // A number typed into the field stays in it.
    await userEvent.type(own, "Use 2 sandbox keys{Enter}");
    expect(
      within(card()).getByRole("heading", {
        name: "Question 3 of 3: Do they run the e2e suite first?",
      }),
    ).toBeTruthy();
    await userEvent.click(
      within(card()).getByRole("button", { name: "Write your own answer…" }),
    );
    await userEvent.type(
      within(card()).getByLabelText("Your own answer"),
      "Only the sign-in journey{Enter}",
    );
    expect(onAnswer).toHaveBeenCalledWith(SET, [
      "Not yet",
      "Use 2 sandbox keys",
      "Only the sign-in journey",
    ]);
    expect((await within(card()).findByRole("status")).textContent).toContain(
      "fold sends your answers",
    );
    for (const button of within(card()).getAllByRole("button")) {
      expect(button).toHaveProperty("disabled", true);
    }
  });

  it("keeps a written answer when the person goes back to it", async () => {
    renderSets([asked], [SET], () => Promise.resolve());
    await screen.findByLabelText("Reply");
    await userEvent.click(
      within(card()).getByRole("button", { name: "Write your own answer…" }),
    );
    await userEvent.type(
      within(card()).getByLabelText("Your own answer"),
      "It reconnects at 17:00{Enter}",
    );
    await userEvent.click(within(card()).getByRole("button", { name: "Back" }));
    expect(
      within(card()).getByLabelText<HTMLInputElement>("Your own answer").value,
    ).toBe("It reconnects at 17:00");
    await userEvent.keyboard("{Escape}");
    expect(within(card()).queryByLabelText("Your own answer")).toBeNull();
  });

  it("shows a refused answer with its code and sends the same answers again", async () => {
    const onAnswer = vi
      .fn<(ask: string, answers: string[]) => Promise<void>>()
      .mockRejectedValueOnce(new ApiError("invalid_request", ""))
      .mockResolvedValue(undefined);
    const one = baseThread(LATER, { ...later, messages: later.messages });
    renderSets([one], [LATER], onAnswer);
    await screen.findByLabelText("Reply");
    expect(
      within(card()).getByRole("heading", { name: "Is Pencil open now?" }),
    ).toBeTruthy();
    expect(within(card()).queryByText(/of 1/)).toBeNull();
    await userEvent.click(
      within(card()).getByRole("button", { name: "Write your own answer…" }),
    );
    await userEvent.type(
      within(card()).getByLabelText("Your own answer"),
      "Yes{Enter}",
    );
    const alert = await within(card()).findByRole("alert");
    expect(alert.textContent).toContain("The answers were not sent.");
    expect(alert.textContent).toContain("code: invalid_request");
    await userEvent.click(
      within(alert).getByRole("button", { name: "Send again" }),
    );
    expect(onAnswer).toHaveBeenCalledTimes(2);
    expect(onAnswer).toHaveBeenLastCalledWith(LATER, ["Yes"]);
  });

  it("shows only the oldest set, says how many more wait and keeps their questions hidden", async () => {
    renderSets([asked, later], [SET, LATER], () => Promise.resolve());
    await screen.findByLabelText("Reply");
    expect(within(card()).getByText("1 more set waits")).toBeTruthy();
    expect(screen.queryByText("Is Pencil open now?")).toBeNull();
    const timeline = screen.getByRole("region", { name: "Messages" });
    expect(
      within(timeline).getByRole("button", {
        name: "Waits for your answer below",
      }),
    ).toBeTruthy();
    expect(within(timeline).getByText("Waits for your answer")).toBeTruthy();
    expect(within(timeline).queryByText("Another thing.")).toBeNull();
    expect(
      within(timeline).getByText(
        "Its questions show once you answer the set below.",
      ),
    ).toBeTruthy();
    await userEvent.click(
      within(timeline).getByRole("button", {
        name: "Waits for your answer below",
      }),
    );
    expect(document.activeElement?.tagName).toBe("H3");
  });

  it("marks the set on the card answered alone, and offers no mark for a worker's AskUserQuestion", async () => {
    daemon.on("POST /api/v1/projects/demo/asks/answered", {
      status: 200,
      body: { answered: [SET] },
    });
    renderSets([asked], [SET], () => Promise.resolve());
    await screen.findByLabelText("Reply");
    await userEvent.click(
      screen.getByRole("button", { name: "Mark answered" }),
    );
    await waitFor(() => {
      expect(
        daemon.made("POST /api/v1/projects/demo/asks/answered")[0]?.body,
      ).toEqual({ ask: SET });
    });
    cleanup();
    const worker = {
      ...asked,
      messages: asked.messages.map((m) => ({ ...m, worker_question: true })),
    };
    renderSets([worker], [SET], () => Promise.resolve());
    await screen.findByLabelText("Reply");
    expect(screen.queryByRole("button", { name: "Mark answered" })).toBeNull();
  });

  it("shows each question of an answered set with its answer, and a single question's chosen option", async () => {
    const REPLY = `m-${String(35)}`;
    const ONE = `m-${String(40)}`;
    const answered = {
      ...asked,
      awaiting_human: false,
      messages: [
        ...asked.messages,
        message(REPLY, {
          sender: "human:tanim",
          reply_to: SET,
          body: "1. Is the MCP server connected again?\nNot yet",
          answers: ["Not yet", "Mock the calls", "No"],
          answers_ask: SET,
          sent_at: NOW,
        }),
      ],
    };
    const single = baseThread(ONE, {
      participants: ["human:tanim", "backend-lead"],
      messages: [
        message(ONE, {
          sender: "backend-lead",
          recipients: toYou,
          body: "Recharts or plain SVG?",
          choices: [
            { label: "Recharts", description: null },
            { label: "Plain SVG", description: null },
          ],
          questions: [
            {
              text: "Recharts or plain SVG?",
              header: null,
              choices: [
                { label: "Recharts", description: null },
                { label: "Plain SVG", description: null },
              ],
            },
          ],
          sent_at: NOW - 5000,
        }),
        message(`m-${String(41)}`, {
          sender: "human:tanim",
          reply_to: ONE,
          body: "Plain SVG",
          answers: ["Plain SVG"],
          answers_ask: ONE,
          sent_at: NOW - 4000,
        }),
      ],
    });
    renderSets([answered, single], [], () => Promise.resolve());
    await screen.findByLabelText("Reply");
    expect(
      screen.queryByRole("region", { name: "Open question set" }),
    ).toBeNull();
    const timeline = screen.getByRole("region", { name: "Messages" });
    expect(
      within(timeline)
        .getAllByRole("term")
        .map((term) => term.textContent),
    ).toEqual(["Recharts or plain SVG?", ...questions.map((q) => q.text)]);
    expect(within(timeline).getByText("Mock the calls")).toBeTruthy();
    // A closed set keeps what it asked under its body.
    expect(
      within(timeline).getAllByText("Which keys must the workers use?"),
    ).toHaveLength(2);
    expect(within(timeline).getByText("Recharts · Plain SVG")).toBeTruthy();
    expect(
      within(timeline)
        .getAllByRole("definition")
        .map((answer) => answer.textContent)[0],
    ).toBe("Plain SVG");
  });

  it("names the question above main's answer for you to a set of one", async () => {
    const ONE = `m-${String(45)}`;
    const keep = baseThread(ONE, {
      participants: ["human:tanim", "web-worker", "main"],
      messages: [
        message(ONE, {
          sender: "web-worker",
          recipients: toYou,
          body: "Keep the old keys?",
          worker_question: true,
          questions: [
            { text: "Keep the old keys?", header: null, choices: [] },
          ],
          sent_at: NOW - 2000,
        }),
        message(`m-${String(46)}`, {
          sender: "main",
          reply_to: ONE,
          body: "No, keep them",
          answers: ["No, keep them"],
          answers_ask: ONE,
          sent_at: NOW - 1000,
        }),
      ],
    });
    renderSets([keep], [], () => Promise.resolve());
    await screen.findByLabelText("Reply");
    const timeline = screen.getByRole("region", { name: "Messages" });
    expect(within(timeline).getByText("Answered for you")).toBeTruthy();
    const term = within(timeline).getByRole("term");
    expect(term.textContent).toBe("Keep the old keys?");
    expect(term.nextElementSibling?.textContent).toBe("No, keep them");
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

describe("ChatPanel with several threads", () => {
  const ASK_A = `m-${String(50)}`;
  const NOTE = `m-${String(51)}`;
  const ASK_B = `m-${String(52)}`;
  const REPORT = `m-${String(53)}`;
  const yesNo = [
    { label: "Yes", description: null },
    { label: "No", description: null },
  ];
  const choice = (id: string, body: string, at: number, fields = {}) =>
    message(id, {
      sender: "frontend",
      recipients: [recipient("human:tanim")],
      body,
      choices: yesNo,
      questions: [{ text: body, header: null, choices: yesNo }],
      sent_at: at,
      ...fields,
    });
  const first = baseThread(ASK_A, {
    task: "T-12",
    participants: ["frontend", "human:tanim"],
    awaiting_human: true,
    last_at: NOW - 3000,
    messages: [
      choice(ASK_A, "Use Recharts?", NOW - 4000, { task: "T-12" }),
      message(NOTE, {
        sender: "frontend",
        body: "I wait for the answer.",
        sent_at: NOW - 3000,
      }),
    ],
  });
  const second = baseThread(ASK_B, {
    proposal: "P-3",
    participants: ["frontend", "human:tanim"],
    awaiting_human: true,
    last_at: NOW - 2000,
    messages: [choice(ASK_B, "Keep the old port?", NOW - 2000)],
  });
  const third = baseThread(REPORT, {
    request: "RQ-4",
    participants: ["frontend", "human:tanim"],
    last_at: NOW - 1000,
    messages: [
      message(REPORT, {
        sender: "frontend",
        body: "The request is drafted.",
        sent_at: NOW - 1000,
      }),
    ],
  });

  let change: (threads: ThreadView[], open: string[]) => void = () => undefined;

  function Harness({
    initial,
    asks,
    link,
    onReply,
    onAnswer,
  }: {
    initial: ThreadView[];
    asks: string[];
    link?: string;
    onReply: (replyTo: string, body: string) => Promise<void>;
    onAnswer: (ask: string, answers: string[]) => Promise<void>;
  }) {
    const [threads, setThreads] = useState(initial);
    const [open, setOpen] = useState(asks);
    change = (next, still) => {
      setThreads(next);
      setOpen(still);
    };
    return (
      <ChatPanel
        slug="demo"
        chat={chatOf(...threads)}
        link={link}
        now={NOW}
        open={new Set(open)}
        onReply={onReply}
        onAnswer={onAnswer}
      />
    );
  }

  async function renderChat(
    initial: ThreadView[],
    onReply: (replyTo: string, body: string) => Promise<void>,
    link?: string,
    onAnswer: (ask: string, answers: string[]) => Promise<void> = () =>
      Promise.resolve(),
  ) {
    const asks = initial.flatMap((t) =>
      t.awaiting_human
        ? t.messages.filter((m) => m.questions.length > 0).map((m) => m.id)
        : [],
    );
    const root = createRootRoute();
    const index = createRoute({
      getParentRoute: () => root,
      path: "/",
      component: () => (
        <Harness
          initial={initial}
          asks={asks}
          link={link}
          onReply={onReply}
          onAnswer={onAnswer}
        />
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

  const card = () => screen.getByRole("region", { name: "Open question set" });
  const pick = (name: RegExp) =>
    within(within(card()).getByRole("list", { name: "Choices" })).getByRole(
      "button",
      { name },
    );
  const answer = (shown: ThreadView, id: number, to: string) => ({
    ...shown,
    messages: [
      ...shown.messages,
      message(`m-${String(id)}`, {
        sender: "human:tanim",
        body: "Yes",
        reply_to: to,
        answers: ["Yes"],
        answers_ask: to,
        sent_at: NOW,
      }),
    ],
  });

  it("shows every thread with the agent in time order, each message with its context", async () => {
    await renderChat([first, second, third], () => Promise.resolve());
    const articles = within(
      screen.getByRole("region", { name: "Messages" }),
    ).getAllByRole("article");
    expect(articles.map((a) => a.querySelector("a")?.textContent)).toEqual([
      "T-12",
      "T-12",
      "P-3",
      "RQ-4",
    ]);
    expect(articles[3]?.textContent).toContain("The request is drafted.");
  });

  it("shows the oldest set first, then the next set once the daemon closed the first, and moves the focus along", async () => {
    const onAnswer = vi.fn(() => Promise.resolve());
    await renderChat(
      [first, second, third],
      () => Promise.resolve(),
      undefined,
      onAnswer,
    );
    expect(
      within(card()).getByRole("heading", { name: "Use Recharts?" }),
    ).toBeTruthy();
    expect(within(card()).getByText("1 more set waits")).toBeTruthy();
    expect(screen.queryByText("Keep the old port?")).toBeNull();
    await userEvent.click(pick(/^Yes/));
    expect(onAnswer).toHaveBeenCalledWith(ASK_A, ["Yes"]);
    act(() => {
      change([answer(first, 70, ASK_A), second, third], [ASK_B]);
    });
    const next = within(card()).getByRole("heading", {
      name: "Keep the old port?",
    });
    expect(document.activeElement).toBe(next);
    expect(within(card()).queryByText(/more set/)).toBeNull();
    // A set of one keeps its question above its answer in the timeline.
    const answeredA = within(
      screen.getByRole("region", { name: "Messages" }),
    ).getByRole("term");
    expect(answeredA.textContent).toBe("Use Recharts?");
    expect(answeredA.nextElementSibling?.textContent).toBe("Yes");
    await userEvent.click(pick(/^No/));
    expect(onAnswer).toHaveBeenLastCalledWith(ASK_B, ["No"]);
    act(() => {
      change([answer(first, 70, ASK_A), answer(second, 71, ASK_B), third], []);
    });
    expect(
      screen.queryByRole("region", { name: "Open question set" }),
    ).toBeNull();
    expect(screen.getByText("Answers sent.")).toBeTruthy();
    expect(document.activeElement).toBe(screen.getByLabelText("Reply"));
  });

  it("says that a set was already answered when the daemon refuses it as closed, or when it leaves without an answer from here", async () => {
    const onAnswer = vi
      .fn<(ask: string, answers: string[]) => Promise<void>>()
      .mockRejectedValue(new ApiError("ask_not_open", "closed"));
    await renderChat(
      [first, second, third],
      () => Promise.resolve(),
      undefined,
      onAnswer,
    );
    await userEvent.click(pick(/^Yes/));
    const alert = await within(card()).findByRole("alert");
    expect(alert.textContent).toBe("This set was already answered.");
    expect(
      within(alert).queryByRole("button", { name: "Send again" }),
    ).toBeNull();
    // A one-question set that the daemon refused as closed offers no step and takes no other answer.
    expect(within(card()).queryByRole("button", { name: "Back" })).toBeNull();
    expect(pick(/^Yes/).hasAttribute("disabled")).toBe(true);
    // The disabled controls look disabled, as a disabled button does.
    const own = within(card()).getByRole("button", {
      name: "Write your own answer…",
    });
    expect(own.hasAttribute("disabled")).toBe(true);
    for (const control of [pick(/^Yes/), own]) {
      expect(control.className).toContain("disabled:opacity-60");
      expect(control.className).toContain("disabled:cursor-not-allowed");
    }
    act(() => {
      change([first, second, third], [ASK_B]);
    });
    expect(screen.getByText("This set was already answered.")).toBeTruthy();
    expect(
      within(card()).getByRole("heading", { name: "Keep the old port?" }),
    ).toBeTruthy();
    // Main answers the next set for the person while the card shows it.
    act(() => {
      change([first, second, third], []);
    });
    expect(
      screen.queryByRole("region", { name: "Open question set" }),
    ).toBeNull();
    expect(
      screen.getByText("This set was already answered.").getAttribute("role"),
    ).toBe("status");
  });

  it("picks a choice by its number key only while the focus is on the card", async () => {
    const onAnswer = vi.fn(() => Promise.resolve());
    await renderChat(
      [first, second, third],
      () => Promise.resolve(),
      undefined,
      onAnswer,
    );
    screen.getByRole("region", { name: "Messages" }).focus();
    await userEvent.keyboard("1");
    expect(onAnswer).not.toHaveBeenCalled();
    within(card()).getByRole("heading", { name: "Use Recharts?" }).focus();
    await userEvent.keyboard("1");
    expect(onAnswer).toHaveBeenCalledWith(ASK_A, ["Yes"]);
    expect(pick(/^Yes/).getAttribute("aria-keyshortcuts")).toBe("1");
  });

  it("adds no note under the reply box for a question with choices, which a reply ends", async () => {
    await renderChat([first, second, third], () => Promise.resolve());
    expect(
      screen.queryByText(
        "Your reply does not answer the open set; answer it in the card.",
      ),
    ).toBeNull();
  });

  it("shows the oldest set also when a link names a later one", async () => {
    await renderChat([first, second, third], () => Promise.resolve(), ASK_B);
    expect(
      within(card()).getByRole("heading", { name: "Use Recharts?" }),
    ).toBeTruthy();
  });

  it("forgets the note of Mark answered once the card shows another set", async () => {
    daemon.on("POST /api/v1/projects/demo/asks/answered", {
      status: 200,
      body: { answered: [] },
    });
    await renderChat([first, second], () => Promise.resolve());
    await userEvent.click(
      screen.getByRole("button", { name: "Mark answered" }),
    );
    expect(await screen.findByText("It was already answered.")).toBeTruthy();
    expect(
      daemon.made("POST /api/v1/projects/demo/asks/answered")[0]?.body,
    ).toEqual({ ask: ASK_A });
    act(() => {
      change([{ ...first, awaiting_human: false }, second], [ASK_B]);
    });
    expect(
      within(card()).getByRole("heading", { name: "Keep the old port?" }),
    ).toBeTruthy();
    await waitFor(() => {
      expect(screen.queryByText("It was already answered.")).toBeNull();
    });
  });

  it("names the newest three subjects in the bar and counts the rest", async () => {
    const fourth = baseThread(`m-${String(80)}`, {
      task: "T-40",
      participants: ["frontend", "human:tanim"],
      last_at: NOW - 500,
      messages: [
        message(`m-${String(80)}`, {
          sender: "frontend",
          body: "T-40 is done.",
          sent_at: NOW - 500,
        }),
      ],
    });
    await renderChat([first, second, third, fourth], () => Promise.resolve());
    const bar = screen.getByRole("banner");
    expect(
      within(bar)
        .getAllByRole("link")
        .map((link) => link.textContent),
    ).toEqual(["T-40", "RQ-4", "P-3"]);
    expect(within(bar).getByText("1 more")).toBeTruthy();
  });

  it("replies in the newest thread, or in the thread the person chooses", async () => {
    const onReply = vi.fn(() => Promise.resolve());
    const talk: ThreadView = {
      ...first,
      awaiting_human: false,
      messages: [
        message(ASK_A, {
          sender: "frontend",
          body: "Use Recharts?",
          sent_at: NOW - 4000,
        }),
        message(NOTE, {
          sender: "frontend",
          body: "I wait for the answer.",
          sent_at: NOW - 3000,
        }),
      ],
    };
    const plain = [talk, third];
    await renderChat(plain, onReply);
    expect(
      screen.queryByRole("region", { name: "Open question set" }),
    ).toBeNull();
    const about = screen.getByRole<HTMLSelectElement>("combobox", {
      name: "About",
    });
    expect(about.value).toBe(REPORT);
    expect([...about.options].map((option) => option.textContent)).toEqual([
      "RQ-4 · The request is drafted.",
      "T-12 · Use Recharts?",
    ]);
    await userEvent.type(screen.getByLabelText("Reply"), "Thanks{Enter}");
    expect(onReply).toHaveBeenLastCalledWith(REPORT, "Thanks");
    await userEvent.selectOptions(about, ASK_A);
    await userEvent.type(screen.getByLabelText("Reply"), "Go on{Enter}");
    expect(onReply).toHaveBeenLastCalledWith(NOTE, "Go on");
  });

  it("writes in the thread of the set on the card by default, and in another thread the person chooses", async () => {
    const onReply = vi.fn(() => Promise.resolve());
    await renderChat([first, third], onReply);
    const about = screen.getByRole<HTMLSelectElement>("combobox", {
      name: "About",
    });
    expect(about.value).toBe(ASK_A);
    expect([...about.options].map((option) => option.value)).toEqual([
      REPORT,
      ASK_A,
    ]);
    await userEvent.selectOptions(about, REPORT);
    await userEvent.type(screen.getByLabelText("Reply"), "Later{Enter}");
    expect(onReply).toHaveBeenLastCalledWith(REPORT, "Later");
  });

  it("shows a thread the agent started with another agent, under each agent's name", async () => {
    const shared = baseThread(`m-${String(60)}`, {
      participants: ["frontend", "backend", "human:tanim"],
      last_at: NOW,
      messages: [
        message(`m-${String(60)}`, {
          sender: "frontend",
          recipients: [recipient("backend")],
          body: "Which field?",
          sent_at: NOW - 10,
        }),
        message(`m-${String(61)}`, {
          sender: "backend",
          body: "The TIN.",
          sent_at: NOW,
        }),
      ],
    });
    await renderChat([shared, third], () => Promise.resolve());
    expect(screen.getByRole("heading", { level: 2 }).textContent).toBe(
      "@frontend",
    );
    const articles = within(
      screen.getByRole("region", { name: "Messages" }),
    ).getAllByRole("article");
    expect(articles.at(-1)?.textContent).toContain("backend");
    expect(articles.at(-1)?.textContent).toContain("The TIN.");
  });

  it("marks each unread thread of the chat read once", async () => {
    daemon.on("GET /api/v1/session", {
      status: 200,
      body: { human: "tanim", authenticated: true },
    });
    for (const id of [ASK_A, REPORT]) {
      daemon.on(`POST /api/v1/projects/demo/threads/${id}/read`, {
        status: 200,
        body: { read: [id] },
      });
    }
    const unread = (t: ThreadView): ThreadView => ({
      ...t,
      messages: t.messages.map((m) => ({
        ...m,
        recipients: [recipient("human:tanim")],
      })),
    });
    await renderChat([unread(first), unread(third)], () => Promise.resolve());
    await waitFor(() => {
      expect(
        daemon.made(`POST /api/v1/projects/demo/threads/${ASK_A}/read`),
      ).toHaveLength(1);
      expect(
        daemon.made(`POST /api/v1/projects/demo/threads/${REPORT}/read`),
      ).toHaveLength(1);
    });
  });
});
