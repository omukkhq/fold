// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  RouterProvider,
  createMemoryHistory,
  createRootRoute,
  createRouter,
} from "@tanstack/react-router";
import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { type StubReply, stubFetch } from "@/test/stubFetch";
import { needsItem } from "@/test/workers";
import { Inbox } from "./Inbox";

beforeAll(() => {
  Element.prototype.hasPointerCapture = () => false;
  Element.prototype.releasePointerCapture = () => {};
});
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const BASE = "/api/v1/projects/demo/agent-changes";
const paths = ["plan.md", "junk1.txt", "a/very/long/path/that/wraps.txt"];

const change = needsItem({
  kind: "agent_change",
  screen: "repository",
  agent: "reviewer-35",
  agents: ["reviewer-35"],
  paths,
});

function heldBack(tree: string, emptied: string[]): StubReply {
  return {
    status: 409,
    body: {
      error: {
        code: "record_needs_confirmation",
        message: "the change empties a tracked file; nothing was recorded",
        details: {
          tree,
          emptied,
          emptied_count: emptied.length,
          agent_changed: paths,
          agent_changed_count: paths.length,
          agents: ["reviewer-35"],
        },
      },
    },
  };
}

const recorded: StubReply = {
  body: {
    outcome: "recorded",
    commit: null,
    revision: "r4",
    files: paths,
    conflicted: [],
    touched: [],
    skipped: [],
    created: true,
    unstash: null,
  },
};

/** The Inbox with a worker's change waiting until a write settles it, and the requests it sent. */
async function renderInbox(
  routes: Record<
    string,
    (search: URLSearchParams) => StubReply | Promise<StubReply>
  >,
) {
  let settled = false;
  const calls = stubFetch({
    "GET /api/v1/session": { body: { human: "ana", authenticated: true } },
    "GET /api/v1/projects/demo/needs-you": () => ({
      body: { items: settled ? [] : [change] },
    }),
    ...Object.fromEntries(
      Object.entries(routes).map(([route, reply]) => [
        route,
        async (request: { search: URLSearchParams }) => {
          const answer = await reply(request.search);
          if ((answer.status ?? 200) < 300) settled = true;
          return answer;
        },
      ]),
    ),
  });
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const root = createRootRoute({ component: () => <Inbox slug="demo" /> });
  const router = createRouter({
    routeTree: root,
    history: createMemoryHistory({ initialEntries: ["/"] }),
  });
  render(
    <QueryClientProvider client={client}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  await userEvent.click(
    await screen.findByRole("button", { name: /^Inbox, 1 waits/ }),
  );
  const inbox = screen.getByRole("dialog", { name: "Inbox" });
  return { calls, inbox };
}

const posts = (calls: { method: string; path: string }[]) =>
  calls.filter((call) => call.method === "POST").map((call) => call.path);

describe("the choices of a worker's change in the Inbox", () => {
  it("records only after the person checks the files the daemon names, with the confirmed tree", async () => {
    const { calls, inbox } = await renderInbox({
      [`POST ${BASE}/record`]: (search) =>
        search.get("confirm") === "abc"
          ? recorded
          : heldBack("abc", ["plan.md"]),
    });
    await userEvent.click(
      within(inbox).getByRole("button", { name: "Record" }),
    );
    const dialog = await screen.findByRole("dialog", {
      name: "Record these files?",
    });
    expect(screen.queryByRole("dialog", { name: "Inbox" })).toBeNull();
    expect(dialog.textContent).toContain(
      "This records the whole project folder on main, also your other unrecorded changes.",
    );
    expect(
      within(dialog).getByRole("region", { name: "Emptied" }).textContent,
    ).toContain("plan.md");
    const anyway = within(dialog).getByRole("button", {
      name: "Record anyway",
    });
    expect(anyway.className).toContain("bg-bad-soft");
    await userEvent.click(anyway);
    await screen.findByRole("dialog", { name: "Inbox" });
    expect(
      screen.queryByRole("dialog", { name: "Record these files?" }),
    ).toBeNull();
    expect(
      calls
        .filter((call) => call.method === "POST")
        .map((call) => call.search.get("confirm")),
    ).toEqual([null, "abc"]);
    expect(await screen.findByText("Nothing waits for you.")).toBeTruthy();
  });

  it("keeps the record dialog open with the refusal, and says when the folder changed since", async () => {
    let tries = 0;
    await renderInbox({
      [`POST ${BASE}/record`]: (search) => {
        tries += 1;
        if (search.get("confirm") === null) return heldBack("abc", ["plan.md"]);
        if (tries === 2) return heldBack("def", ["notes.md"]);
        return {
          status: 400,
          body: {
            error: {
              code: "invalid_request",
              message: "branch moved",
              details: { reason: "git_commits_wait" },
            },
          },
        };
      },
    });
    await userEvent.click(screen.getByRole("button", { name: "Record" }));
    const dialog = await screen.findByRole("dialog", {
      name: "Record these files?",
    });
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Record anyway" }),
    );
    expect(
      await within(dialog).findByText(
        "The folder changed; check the files again.",
      ),
    ).toBeTruthy();
    expect(dialog.textContent).toContain("notes.md");
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Record anyway" }),
    );
    expect((await within(dialog).findByRole("alert")).textContent).toContain(
      "commits that fold did not make",
    );
    expect(
      screen.getByRole("dialog", { name: "Record these files?" }),
    ).toBeTruthy();
  });

  it("asks before it puts the agent's files aside, names the stash entry and lists every file", async () => {
    const { calls, inbox } = await renderInbox({
      [`POST ${BASE}/restore`]: () => ({
        body: { revision: "r3", files: paths, entries: 1 },
      }),
    });
    await userEvent.click(
      within(inbox).getByRole("button", {
        name: "Put the agent's files aside",
      }),
    );
    const dialog = await screen.findByRole("dialog", {
      name: "Put 3 files aside?",
    });
    expect(dialog.textContent).toContain("stash@{0}");
    expect(dialog.textContent).toContain("fld unstash brings them back.");
    for (const path of paths) {
      expect(within(dialog).getByText(path).closest("li")?.className).toContain(
        "wrap-anywhere",
      );
    }
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Cancel" }),
    );
    expect(posts(calls)).toEqual([]);
    await userEvent.click(
      within(await screen.findByRole("dialog", { name: "Inbox" })).getByRole(
        "button",
        { name: "Put the agent's files aside" },
      ),
    );
    await userEvent.click(
      within(
        await screen.findByRole("dialog", { name: "Put 3 files aside?" }),
      ).getByRole("button", { name: "Put files aside" }),
    );
    expect(await screen.findByText("Nothing waits for you.")).toBeTruthy();
    expect(posts(calls)).toEqual([`${BASE}/restore`]);
  });

  it("asks before it keeps the files as the person's own, and shows a refusal in the dialog", async () => {
    let tries = 0;
    const { calls, inbox } = await renderInbox({
      [`POST ${BASE}/ignore`]: () => {
        tries += 1;
        return tries === 1
          ? {
              status: 403,
              body: {
                error: {
                  code: "worker_peer_refused",
                  message: "refused",
                  details: {},
                },
              },
            }
          : { body: { files: paths } };
      },
    });
    await userEvent.click(
      within(inbox).getByRole("button", { name: "Keep as mine" }),
    );
    const dialog = await screen.findByRole("dialog", {
      name: "Keep 3 files as yours?",
    });
    expect(dialog.textContent).toContain("a change of reviewer-35");
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Keep as mine" }),
    );
    expect((await within(dialog).findByRole("alert")).textContent).toContain(
      "a worker or a check",
    );
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Keep as mine" }),
    );
    expect(await screen.findByText("Nothing waits for you.")).toBeTruthy();
    expect(posts(calls)).toEqual([`${BASE}/ignore`, `${BASE}/ignore`]);
  });

  it("does not close a dialog while its write is in flight", async () => {
    let finish: (reply: StubReply) => void = () => {};
    const waiting = new Promise<StubReply>((resolve) => {
      finish = resolve;
    });
    const { inbox } = await renderInbox({
      [`POST ${BASE}/restore`]: () => waiting,
    });
    await userEvent.click(
      within(inbox).getByRole("button", {
        name: "Put the agent's files aside",
      }),
    );
    const dialog = await screen.findByRole("dialog", {
      name: "Put 3 files aside?",
    });
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Put files aside" }),
    );
    await within(dialog).findByRole("button", { name: /Putting files aside/ });
    await userEvent.keyboard("{Escape}");
    expect(
      screen.getByRole("dialog", { name: "Put 3 files aside?" }),
    ).toBeTruthy();
    expect(
      within(dialog).getByRole<HTMLButtonElement>("button", { name: "Cancel" })
        .disabled,
    ).toBe(true);
    finish({ body: { revision: "r3", files: paths, entries: 1 } });
    expect(await screen.findByText("Nothing waits for you.")).toBeTruthy();
  });
});
