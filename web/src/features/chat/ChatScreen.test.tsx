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
import type { ChatView, MessageView } from "@/api/messages";
import { errorReply, stubFetch, type StubReply } from "@/test/stubFetch";
import { areaEntry, workerList, workerProcess } from "@/test/workers";
import { ChatScreen } from "./ChatScreen";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const BASE = "/api/v1/projects/demo";

function message(id: string, sender: string, body: string): MessageView {
  return {
    id,
    thread: "m-12",
    reply_to: null,
    sender,
    task: null,
    proposal: null,
    request: null,
    body,
    sent_at: 1_760_000_000_000,
    recipients: [],
    choices: [],
    header: null,
  };
}

function page(messages: MessageView[]): ChatView {
  return { agent: "web-lead", messages, next: null };
}

const team = workerList({
  areas: [
    areaEntry("web", "frontend", {
      lead: "web-lead",
      instructions: "Draw first.",
    }),
  ],
  workers: [workerProcess("web-lead")],
});

type Routes = Record<string, StubReply | (() => StubReply)>;

function routes(extra: Routes = {}): Routes {
  return {
    "GET /api/v1/session": { body: { human: "tanim", authenticated: true } },
    [`GET ${BASE}/workers`]: { body: team },
    [`GET ${BASE}/agents`]: { body: { agents: [] } },
    [`GET ${BASE}/agents/web-lead/chat`]: { body: page([]) },
    ...extra,
  };
}

function setup(agent = "web-lead", note?: string) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const root = createRootRoute();
  const index = createRoute({
    getParentRoute: () => root,
    path: "/",
    component: () => <ChatScreen slug="demo" agent={agent} note={note} />,
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
}

describe("ChatScreen", () => {
  it("shows the empty chat and how a message reaches the lead", async () => {
    stubFetch(routes());
    setup();
    expect(await screen.findByText("No messages yet")).toBeTruthy();
    expect(await screen.findByText(/area lead of web/)).toBeTruthy();
    expect(screen.getByText(/as a turn/)).toBeTruthy();
  });

  it("sends a message and shows it once the daemon has it", async () => {
    let sent = false;
    const calls = stubFetch(
      routes({
        [`POST ${BASE}/agents/web-lead/chat`]: () => {
          sent = true;
          return {
            status: 201,
            body: message("m-14", "human:tanim", "Use the tokens."),
          };
        },
        [`GET ${BASE}/agents/web-lead/chat`]: () => ({
          body: page(
            sent
              ? [
                  message("m-12", "web-lead", "Hello."),
                  message("m-14", "human:tanim", "Use the tokens."),
                ]
              : [message("m-12", "web-lead", "Hello.")],
          ),
        }),
      }),
    );
    setup();
    const chat = await screen.findByRole("region", { name: "Chat" });
    expect(await within(chat).findByText("Hello.")).toBeTruthy();
    await userEvent.type(
      await within(chat).findByLabelText("Message"),
      "Use the tokens.",
    );
    await userEvent.click(within(chat).getByRole("button", { name: "Send" }));
    expect(await within(chat).findByText("Use the tokens.")).toBeTruthy();
    const post = calls.find((call) => call.method === "POST");
    expect(post?.body).toEqual({ body: "Use the tokens." });
  });

  it("starts a reply to a note with the note's mention", async () => {
    const calls = stubFetch(
      routes({
        [`POST ${BASE}/agents/web-lead/chat`]: {
          status: 201,
          body: message("m-14", "human:tanim", "#n-4 Why?"),
        },
      }),
    );
    setup("web-lead", "n-4");
    const chat = await screen.findByRole("region", { name: "Chat" });
    const box = await within(chat).findByLabelText("Message");
    expect((box as HTMLTextAreaElement).value).toBe("#n-4 ");
    await userEvent.type(box, "Why?");
    await userEvent.click(within(chat).getByRole("button", { name: "Send" }));
    await waitFor(() => {
      expect((box as HTMLTextAreaElement).value).toBe("");
    });
    const post = calls.find((call) => call.method === "POST");
    expect(post?.body).toEqual({ body: "#n-4 Why?" });
  });

  it("shows the options of a question the agent asks in the chat", async () => {
    stubFetch(
      routes({
        [`GET ${BASE}/agents/web-lead/chat`]: {
          body: page([
            {
              ...message("m-12", "web-lead", "Which palette?"),
              header: "Palette",
              choices: [
                { label: "Warm", description: "Sand and clay." },
                { label: "Cold", description: null },
              ],
            },
          ]),
        },
      }),
    );
    setup();
    const choices = await screen.findByRole("list", { name: "Choices" });
    expect(within(choices).getByText("Warm")).toBeTruthy();
    expect(within(choices).getByText("Sand and clay.")).toBeTruthy();
    expect(within(choices).getByText("Cold")).toBeTruthy();
    expect(screen.getByText("Palette")).toBeTruthy();
  });

  it("shows a refused send with its sentence", async () => {
    stubFetch(
      routes({
        [`POST ${BASE}/agents/web-lead/chat`]: errorReply(
          404,
          "agent_not_found",
        ),
      }),
    );
    setup();
    const chat = await screen.findByRole("region", { name: "Chat" });
    await userEvent.type(await within(chat).findByLabelText("Message"), "hi");
    await userEvent.click(within(chat).getByRole("button", { name: "Send" }));
    const alert = await within(chat).findByRole("alert");
    expect(alert.textContent).toContain("fold has no agent with this name.");
  });

  it("edits the lasting instructions of the lead's area", async () => {
    let saved = false;
    const calls = stubFetch(
      routes({
        [`PATCH ${BASE}/areas/web`]: () => {
          saved = true;
          return { body: team.areas[0] };
        },
        [`GET ${BASE}/workers`]: () => ({
          body: saved
            ? workerList({
                areas: [
                  areaEntry("web", "frontend", {
                    lead: "web-lead",
                    instructions: "Draw first.\n\nUse the tokens.",
                  }),
                ],
                workers: team.workers,
              })
            : team,
        }),
      }),
    );
    setup();
    const panel = await screen.findByRole("region", {
      name: "Instructions of area web",
    });
    expect(within(panel).getByText("Draw first.")).toBeTruthy();
    await userEvent.click(
      await within(panel).findByRole("button", { name: "Edit" }),
    );
    const field = within(panel).getByRole("textbox", { name: "Instructions" });
    await userEvent.type(field, "\n\nUse the tokens.");
    await userEvent.click(within(panel).getByRole("button", { name: "Save" }));
    await waitFor(() => {
      expect(
        screen.queryByRole("textbox", { name: "Instructions" }),
      ).toBeNull();
    });
    expect(
      screen.getByRole("region", { name: "Instructions of area web" })
        .textContent,
    ).toContain("Use the tokens.");
    const patch = calls.find((call) => call.method === "PATCH");
    expect(patch?.body).toEqual({
      instructions: "Draw first.\n\nUse the tokens.",
    });
  });

  it("shows no area panel for the main agent", async () => {
    stubFetch(
      routes({
        [`GET ${BASE}/agents/main/chat`]: { body: page([]) },
      }),
    );
    setup("main");
    expect(await screen.findByText(/Claude Code session/)).toBeTruthy();
    expect(
      screen.queryByRole("region", { name: /Instructions of area/ }),
    ).toBeNull();
  });
});
