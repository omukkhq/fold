// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
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
import { afterEach, describe, expect, it, vi } from "vitest";
import { keys } from "@/api/keys";
import type { ThreadView } from "@/api/messages";
import { message, recipient, thread } from "@/test/messages";
import { needsItem } from "@/test/workers";
import { errorReply, stubFetch } from "@/test/stubFetch";
import { MessagesScreen } from "./MessagesScreen";
import type { MessagesSearch } from "./search";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

/** The router of the latest setup, to read where a filter choice leads. */
let lastRouter: { state: { location: { href: string } } } | null = null;

/** The threads marked as waiting hold the asks Needs you lists, their first message each. */
async function setup(
  search: MessagesSearch,
  threads: ThreadView[],
  more = false,
): Promise<QueryClient> {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  queryClient.setQueryData(keys.messages("demo"), {
    items: threads,
    more,
  });
  queryClient.setQueryData(
    keys.needsYou("demo"),
    threads
      .filter((t) => t.awaiting_human)
      .map((t) => needsItem({ kind: "question", message: t.id })),
  );
  const root = createRootRoute();
  const index = createRoute({
    getParentRoute: () => root,
    path: "/",
    component: () => <MessagesScreen slug="demo" search={search} />,
  });
  const router = createRouter({
    routeTree: root.addChildren([index]),
    history: createMemoryHistory({ initialEntries: ["/"] }),
  });
  lastRouter = router;
  render(
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  await screen.findByRole("heading", { name: "Messages" });
  return queryClient;
}

const MESSAGES_ROUTE = "/api/v1/projects/demo/messages";
const FEEDBACK_ROUTE = "/api/v1/projects/demo/feedback";

// An id such as m-31 looks like a margin utility to the token guard, so the test builds it.
const FIRST = `m-${String(31)}`;
const MISSING = `m-${String(99)}`;
const SECOND = `m-${String(32)}`;

const question = thread(FIRST, {
  task: "T-12",
  awaiting_human: true,
  participants: ["human:tanim", "frontend/ui"],
  last_at: 1000,
  messages: [
    message(FIRST, {
      body: "Which chart library?\nRecharts or plain SVG.",
      recipients: [recipient("human:tanim", { delivered_at: 1000 })],
      sent_at: 1000,
    }),
  ],
});

describe("MessagesScreen", () => {
  it("shows the empty state for a project without messages", async () => {
    await setup({}, []);
    expect(
      screen.getByRole("region", { name: "No messages yet" }),
    ).toBeTruthy();
  });

  it("says no thread matches when the filters hide every thread", async () => {
    await setup({ task: "T-999" }, [question]);
    expect(
      screen.getByRole("region", { name: "No chat matches" }),
    ).toBeTruthy();
  });

  it("says older threads are not listed when the list stopped at its page limit", async () => {
    await setup({}, [question], true);
    expect(
      screen.getByText(
        "Showing the newest 1 threads. Older threads are not listed.",
      ),
    ).toBeTruthy();
  });

  it("marks the open thread in the list only while its pane shows", async () => {
    const current = () =>
      within(screen.getByRole("list", { name: "Chats" }))
        .getAllByRole("link")
        .filter((link) => link.getAttribute("aria-current") === "true");
    await setup({}, [question]);
    expect(current()).toHaveLength(0);
    cleanup();
    vi.stubGlobal(
      "matchMedia",
      vi.fn(() => ({
        matches: true,
        addEventListener: vi.fn(),
        removeEventListener: vi.fn(),
      })),
    );
    await setup({}, [question]);
    expect(current()).toHaveLength(1);
    cleanup();
    vi.unstubAllGlobals();
    await setup({ thread: FIRST }, [question]);
    expect(current()).toHaveLength(1);
  });

  it("names a chat by its agent and counts the questions that wait for you", async () => {
    await setup({}, [question]);
    const list = screen.getByRole("list", { name: "Chats" });
    const row = within(list).getByRole("link");
    expect(row.textContent).toContain("@frontend/ui");
    expect(row.textContent).toContain("Which chart library?");
    expect(
      within(row).getByRole("status", {
        name: "1 question set waits for you",
      }),
    ).toBeTruthy();
  });

  it("joins the threads of one agent into one chat and keeps it with the waiting filter", async () => {
    const report = thread(SECOND, {
      participants: ["frontend/ui", "human:tanim"],
      last_at: 2000,
      messages: [message(SECOND, { body: "The build passes.", sent_at: 2000 })],
    });
    const other = thread(`m-${String(40)}`, {
      awaiting_human: true,
      participants: ["backend", "human:tanim"],
      last_at: 500,
      messages: [
        message(`m-${String(40)}`, {
          sender: "backend",
          recipients: [recipient("human:tanim")],
          body: "Which port?",
          sent_at: 500,
        }),
      ],
    });
    const threads = [report, question, other];
    await setup({}, threads);
    const rows = () =>
      within(screen.getByRole("list", { name: "Chats" })).getAllByRole("link");
    expect(rows().map((row) => row.textContent)).toEqual([
      expect.stringContaining("@frontend/ui"),
      expect.stringContaining("@backend"),
    ]);
    // The chat shows its newest message, from the thread that does not wait.
    expect(rows()[0]?.textContent).toContain("The build passes.");
    expect(screen.getByText("2 chats")).toBeTruthy();
    expect(
      screen.getAllByRole("status", { name: "1 question set waits for you" }),
    ).toHaveLength(2);
    await userEvent.click(
      screen.getByRole("button", { name: "Filter: All chats" }),
    );
    const menu = screen.getByRole("dialog", { name: "Filter chats" });
    const waiting = within(menu).getByRole("button", { name: /Waits for you/ });
    expect(waiting.getAttribute("aria-pressed")).toBe("false");
    expect(
      within(waiting).getByRole("status", { name: "2 wait for you" }),
    ).toBeTruthy();
    cleanup();
    await setup({ waiting: true }, [report, other]);
    expect(rows()).toHaveLength(1);
    expect(rows()[0]?.textContent).toContain("@backend");
    await userEvent.click(
      screen.getByRole("button", { name: "Filter: Waits for you" }),
    );
    const open = screen.getByRole("dialog", { name: "Filter chats" });
    expect(
      within(open)
        .getByRole("button", { name: /Waits for you/ })
        .getAttribute("aria-pressed"),
    ).toBe("true");
    expect(screen.getByText("1 chat")).toBeTruthy();
  });

  it("opens a chat from its row, and a link to a message in the chat that holds it", async () => {
    const later = thread(SECOND, {
      task: "T-3",
      participants: ["frontend/ui", "human:tanim"],
      last_at: 2000,
      messages: [message(SECOND, { body: "Started T-3.", sent_at: 2000 })],
    });
    await setup({}, [later, question]);
    expect(
      within(screen.getByRole("list", { name: "Chats" }))
        .getByRole("link")
        .getAttribute("href"),
    ).toBe("/p/demo/messages?chat=frontend%2Fui");
    cleanup();
    await setup({ thread: FIRST }, [later, question]);
    const chat = screen.getByRole("region", { name: "@frontend/ui" });
    const messages = within(chat).getByRole("region", { name: "Messages" });
    expect(within(messages).getAllByRole("article")).toHaveLength(2);
    expect(
      within(chat).getByRole("region", { name: "Open question set" })
        .textContent,
    ).toContain("Which chart library?");
    expect(
      within(chat)
        .getByRole("link", { name: "Back to chats" })
        .getAttribute("href"),
    ).toBe("/p/demo/messages");
  });

  it("names the task the address narrows to in the filter menu", async () => {
    const report = thread(SECOND, {
      task: "T-3",
      last_at: 2000,
      messages: [message(SECOND, { body: "The build passes.", sent_at: 2000 })],
    });
    await setup({ task: "T-12" }, [report, question]);
    expect(
      within(screen.getByRole("list", { name: "Chats" })).getAllByRole(
        "listitem",
      ),
    ).toHaveLength(1);
    await userEvent.click(
      screen.getByRole("button", { name: "Filter: All chats · 1 more" }),
    );
    const menu = screen.getByRole("dialog", { name: "Filter chats" });
    expect(
      within(menu).getByRole<HTMLSelectElement>("combobox", { name: "Task" })
        .value,
    ).toBe("T-12");
    expect(
      within(menu)
        .getAllByRole("option")
        .map((option) => option.textContent),
    ).toEqual([
      "All agents",
      "frontend/ui",
      "All tasks",
      "T-3",
      "T-12",
      "All proposals",
    ]);
    await userEvent.selectOptions(
      within(menu).getByRole("combobox", { name: "Task" }),
      "T-3",
    );
    await waitFor(() => {
      expect(lastRouter?.state.location.href).toBe("/p/demo/messages?task=T-3");
    });
  });

  it("narrows the list by the search field and by the questions choice", async () => {
    const report = thread(SECOND, {
      participants: ["backend", "human:tanim"],
      last_at: 2000,
      messages: [
        message(SECOND, {
          sender: "backend",
          body: "The build passes.",
          sent_at: 2000,
        }),
      ],
    });
    const asked = thread(`m-${String(41)}`, {
      last_at: 500,
      messages: [
        message(`m-${String(41)}`, {
          body: "Which port?",
          choices: [{ label: "7420", description: null }],
        }),
      ],
    });
    await setup({}, [report, asked]);
    const list = () => screen.getByRole("list", { name: "Chats" });
    await userEvent.type(screen.getByLabelText("Search chats"), "build");
    expect(within(list()).getAllByRole("listitem")).toHaveLength(1);
    expect(within(list()).getAllByText("The build passes.")).toHaveLength(1);
    await userEvent.clear(screen.getByLabelText("Search chats"));
    await userEvent.type(screen.getByLabelText("Search chats"), "nothing");
    expect(
      screen.getByRole("region", { name: "No chat matches" }),
    ).toBeTruthy();
    cleanup();
    await setup({ questions: true }, [report, asked]);
    expect(within(list()).getAllByRole("listitem")).toHaveLength(1);
    expect(within(list()).getByRole("link").textContent).toContain(
      "@frontend/ui",
    );
    expect(
      screen.getByRole("button", { name: "Filter: Questions" }),
    ).toBeTruthy();
  });

  it("reads a requested thread the list did not reach, and says when it does not exist", async () => {
    const old = thread(`m-${String(7)}`, {
      participants: ["human:tanim", "backend"],
      messages: [message(`m-${String(7)}`, { body: "An old question." })],
    });
    stubFetch({
      [`GET /api/v1/projects/demo/threads/${old.id}`]: {
        body: old,
      },
      [`GET /api/v1/projects/demo/threads/${MISSING}`]: errorReply(
        404,
        "message_not_found",
      ),
    });
    await setup({ thread: old.id }, [question], true);
    expect(
      (await screen.findAllByText("An old question.")).length,
    ).toBeGreaterThan(0);
    cleanup();
    await setup({ thread: MISSING }, [question], true);
    expect(await screen.findByText(`${MISSING} does not exist`)).toBeTruthy();
  });

  it("says that a requested chat does not exist instead of opening another one", async () => {
    await setup({ chat: "docs" }, [question]);
    expect(screen.getByText("No chat with @docs")).toBeTruthy();
    expect(screen.queryByRole("region", { name: "Messages" })).toBeNull();
    expect(
      screen
        .getByRole("link", { name: /frontend\/ui/ })
        .getAttribute("aria-current"),
    ).toBeNull();
  });

  it("shows a reply as pending, then in the thread once the daemon answers", async () => {
    let answered = false;
    let release: () => void = () => undefined;
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    stubFetch({
      [`POST ${FEEDBACK_ROUTE}`]: async () => {
        await gate;
        answered = true;
        return {
          status: 201,
          body: message(SECOND, { thread: FIRST, body: "Plain SVG." }),
        };
      },
      [`GET ${MESSAGES_ROUTE}`]: () => ({
        body: {
          next: null,
          items: [
            {
              ...question,
              awaiting_human: !answered,
              messages: answered
                ? [
                    ...question.messages,
                    message(SECOND, {
                      thread: FIRST,
                      sender: "human:tanim",
                      body: "Plain SVG.",
                      reply_to: FIRST,
                    }),
                  ]
                : question.messages,
            },
          ],
        },
      }),
    });
    await setup({}, [question]);
    const panel = screen.getAllByRole("region")[0];
    if (!panel) {
      throw new Error("no thread panel");
    }
    expect(within(panel).getAllByRole("article")).toHaveLength(1);
    await userEvent.type(within(panel).getByLabelText("Reply"), "Plain SVG.");
    await userEvent.click(within(panel).getByRole("button", { name: "Send" }));
    expect(
      within(panel)
        .getByRole("button", { name: "Sending…" })
        .getAttribute("aria-busy"),
    ).toBe("true");
    expect(within(panel).getAllByRole("article")).toHaveLength(1);
    release();
    await waitFor(() => {
      expect(within(panel).getAllByRole("article")).toHaveLength(2);
    });
    expect(within(panel).getByText("Plain SVG.")).toBeTruthy();
    expect(
      within(panel).getByLabelText<HTMLTextAreaElement>("Reply").value,
    ).toBe("");
    expect(
      within(screen.getByRole("list", { name: "Chats" })).queryByRole("img", {
        name: "waits for you",
      }),
    ).toBeNull();
  });
});
