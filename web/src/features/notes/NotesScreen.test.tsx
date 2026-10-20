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
import type { NoteView } from "@/api/notes";
import { stubFetch, type StubReply } from "@/test/stubFetch";
import { NotesScreen } from "./NotesScreen";
import { parseNotesSearch } from "./notes";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

function note(
  id: string,
  agent: string,
  area: string | null,
  title: string,
): NoteView {
  return {
    id,
    author: agent,
    agent,
    area,
    title,
    decision: `The decision of ${title}.`,
    reason: `The reason of ${title}.`,
    links: id === "n-1" ? ["#T-12"] : [],
    written_at: 1_760_000_000_000,
  };
}

const NOTES = [
  note("n-3", "planner", null, "Scripts run with bun"),
  note("n-2", "web-lead", "web", "Cards sort by due date"),
  note("n-1", "api-lead", "api", "Errors carry a code"),
];

type Routes = Record<string, StubReply | (() => StubReply)>;

async function setup(
  routes: Routes,
  unread: number | null,
  initial = "/p/demo/notes",
) {
  const calls = stubFetch({
    "GET /api/v1/session": { body: { human: "tanim", authenticated: true } },
    "GET /api/v1/projects/demo/agents": { body: { agents: [] } },
    ...routes,
  });
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  queryClient.setQueryData(keys.notes("demo"), { items: NOTES, more: false });
  queryClient.setQueryData(keys.notesUnread("demo"), unread);
  const root = createRootRoute();
  const notes = createRoute({
    getParentRoute: () => root,
    path: "/p/$slug/notes",
    validateSearch: parseNotesSearch,
    component: function Notes() {
      const search = notes.useSearch();
      return <NotesScreen slug="demo" search={search} />;
    },
  });
  const router = createRouter({
    routeTree: root.addChildren([notes]),
    history: createMemoryHistory({ initialEntries: [initial] }),
  });
  render(
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  await screen.findByRole("heading", { name: "Notes" });
  return { calls, router };
}

const titles = () =>
  within(screen.getByRole("list", { name: "Notes" }))
    .getAllByRole("heading")
    .map((heading) => heading.textContent);

describe("NotesScreen", () => {
  it("lists the notes, marks the new ones and the read mark, and replies in the author's chat", async () => {
    const { calls } = await setup(
      {
        "POST /api/v1/projects/demo/notes/read": { body: { unread: 0 } },
        "GET /api/v1/projects/demo/notes": {
          body: { items: NOTES, next: null, unread: 0 },
        },
      },
      2,
    );
    expect(titles()).toEqual([
      "Scripts run with bun",
      "Cards sort by due date",
      "Errors carry a code",
    ]);
    const first = screen.getByRole("listitem", {
      name: "n-3 Scripts run with bun",
    });
    expect(within(first).getByText("New")).toBeTruthy();
    expect(within(first).getByText("No area")).toBeTruthy();
    const last = screen.getByRole("listitem", {
      name: "n-1 Errors carry a code",
    });
    expect(within(last).queryByText("New")).toBeNull();
    expect(
      within(last).getByText("The reason of Errors carry a code."),
    ).toBeTruthy();
    expect(
      within(last).getByRole("link", { name: /T-12/ }).getAttribute("href"),
    ).toBe("/p/demo/t/T-12");
    const reply = within(last).getByRole("link", { name: "Reply" });
    expect(reply.getAttribute("href")).toBe(
      "/p/demo/agents/api-lead/chat?note=n-1",
    );
    await waitFor(() => {
      const post = calls.find((call) => call.method === "POST");
      expect(post?.body).toEqual({ through: "n-3" });
    });
    expect(within(first).getByText("New")).toBeTruthy();
  });

  it("filters by area and agent through the address", async () => {
    const { calls, router } = await setup({}, 0);
    await userEvent.click(screen.getByRole("button", { name: "Area" }));
    await userEvent.click(
      await screen.findByRole("menuitem", { name: "No area" }),
    );
    await waitFor(() => {
      expect(titles()).toEqual(["Scripts run with bun"]);
    });
    expect(router.state.location.search).toEqual({ area: "-" });
    await userEvent.click(screen.getByRole("button", { name: "Area: none" }));
    await userEvent.click(await screen.findByRole("menuitem", { name: "web" }));
    await userEvent.click(screen.getByRole("button", { name: "Agent" }));
    await userEvent.click(
      await screen.findByRole("menuitem", { name: "planner" }),
    );
    expect(await screen.findByText("No note matches")).toBeTruthy();
    expect(calls.some((call) => call.method === "POST")).toBe(false);
  });

  it("shows the note a link names and the empty state", async () => {
    await setup({}, null, "/p/demo/notes?note=n-2");
    const current = screen.getByRole("listitem", {
      name: "n-2 Cards sort by due date",
    });
    expect(current.getAttribute("aria-current")).toBe("true");
    cleanup();
    vi.unstubAllGlobals();
    const queryClient = new QueryClient();
    queryClient.setQueryData(keys.notes("demo"), { items: [], more: true });
    stubFetch({});
    const root = createRootRoute({
      component: () => <NotesScreen slug="demo" search={{}} />,
    });
    const router = createRouter({
      routeTree: root,
      history: createMemoryHistory({ initialEntries: ["/"] }),
    });
    render(
      <QueryClientProvider client={queryClient}>
        <RouterProvider router={router} />
      </QueryClientProvider>,
    );
    expect(await screen.findByText("No notes yet")).toBeTruthy();
    expect(screen.getByText(/Older notes are not listed/)).toBeTruthy();
  });
});
