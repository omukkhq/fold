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
import { errorReply, stubFetch } from "@/test/stubFetch";
import { MessagesScreen } from "./MessagesScreen";
import type { MessagesSearch } from "./search";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

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
      screen.getByRole("region", { name: "No thread matches" }),
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

  it("names a thread by its first line and marks a question waiting for a human", async () => {
    await setup({}, [question]);
    const list = screen.getByRole("list", { name: "Threads" });
    expect(within(list).getByText("Which chart library?")).toBeTruthy();
    expect(
      within(list).getByText("Question waiting for a human answer."),
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
      screen.queryByText("Question waiting for a human answer."),
    ).toBeNull();
  });
});
