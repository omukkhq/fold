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
import type { NeedsYouItem } from "@/api/workers";
import { comparedFile, comparison, del, fileDiff, hunk } from "@/test/diff";
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

/** The project folder against its revision: the emptied plan and the new junk. */
const FOLDER = [
  comparedFile("plan.md", "modified", { added_lines: 0, removed_lines: 1 }),
  comparedFile("junk1.txt", "added", { added_lines: 1, removed_lines: 0 }),
  comparedFile("a/very/long/path/that/wraps.txt", "added", {
    added_lines: 1,
    removed_lines: 0,
  }),
];

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
  items: NeedsYouItem[] = [change],
) {
  let settled = false;
  const calls = stubFetch({
    "GET /api/v1/session": { body: { human: "ana", authenticated: true } },
    "GET /api/v1/projects/demo/needs-you": () => ({
      body: { items: settled ? [] : items },
    }),
    "GET /api/v1/projects/demo/folder/diff": (request) => {
      const path = request.search.get("path");
      const file = FOLDER.find((one) => one.path === path);
      if (path === null) {
        return { body: { view: "files", ...comparison(FOLDER) } };
      }
      return file === undefined
        ? {
            status: 404,
            body: {
              error: {
                code: "path_not_in_comparison",
                message: "not changed",
                details: {},
              },
            },
          }
        : {
            body: {
              view: "file",
              ...fileDiff(file, [hunk(1, 0, [del("the plan")])]),
            },
          };
    },
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
    await screen.findByRole("button", { name: /^Inbox, \d+ waits? for you/ }),
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
    expect((await within(dialog).findByRole("alert")).textContent).toBe(
      "The folder changed; check the files again.",
    );
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
      expect((await within(dialog).findByText(path)).className).toContain(
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
    expect(dialog.textContent).toContain("a change of @reviewer-35");
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

  it("names the agent by @name and the files in the path face", async () => {
    const { inbox } = await renderInbox({});
    const project = within(inbox).getByRole("region", { name: "Project" });
    expect(within(project).getByText("@reviewer-35")).toBeTruthy();
    expect(project.textContent).toContain(
      "changed files in the project folder while it worked",
    );
    const first = within(project).getByText("plan.md");
    expect(first.className).toContain("font-mono");
    const words = first.closest("button");
    expect(words?.textContent).toBe(
      "3 files: plan.md and 2 more, show the changes",
    );
    expect(first.closest(".wrap-anywhere")).not.toBeNull();
  });

  it("dismisses every changed shared file from its line, pending until Needs you is read again", async () => {
    let finish: (reply: StubReply) => void = () => {};
    const waiting = new Promise<StubReply>((resolve) => {
      finish = resolve;
    });
    const shared = (agent: string, paths: string[]) =>
      needsItem({ kind: "shared_changed", screen: "repository", agent, paths });
    const { calls } = await renderInbox(
      { "POST /api/v1/projects/demo/shared/dismiss": () => waiting },
      [shared("ops-lead", ["design/home.pen"]), shared("backend", ["a.pen"])],
    );
    const inbox = screen.getByRole("dialog", { name: "Inbox" });
    const [dismiss] = within(inbox).getAllByRole("button", {
      name: "Dismiss all 2",
    });
    if (dismiss === undefined) throw new Error("no Dismiss");
    expect(dismiss.className).toContain("-ml-10");
    await userEvent.click(dismiss);
    expect(
      await within(inbox).findByRole("button", { name: /Dismissing/ }),
    ).toBeTruthy();
    finish({ body: { dismissed: 2 } });
    expect(await screen.findByText("Nothing waits for you.")).toBeTruthy();
    expect(posts(calls)).toEqual(["/api/v1/projects/demo/shared/dismiss"]);
  });

  it("opens the worker's change from its files, with each file's diff and the three choices in the dialog", async () => {
    const { calls, inbox } = await renderInbox({
      [`POST ${BASE}/restore`]: () => ({
        body: { revision: "r3", files: paths, entries: 1 },
      }),
    });
    await userEvent.click(
      within(inbox).getByRole("button", { name: /show the changes/ }),
    );
    const dialog = await screen.findByRole("dialog", {
      name: "3 files changed while @reviewer-35 worked",
    });
    const plan = await within(dialog).findByRole("button", {
      name: /plan\.md/,
    });
    expect(plan.getAttribute("aria-expanded")).toBe("false");
    expect(plan.textContent).toContain("−1");
    expect(
      within(dialog).getByRole("button", { name: /junk1\.txt/ }).textContent,
    ).toContain("new file");
    await userEvent.click(plan);
    expect(plan.getAttribute("aria-expanded")).toBe("true");
    expect(await within(dialog).findByText("the plan")).toBeTruthy();
    expect(calls.some((call) => call.search.get("path") === "plan.md")).toBe(
      true,
    );
    for (const name of ["Record", "Keep as mine"]) {
      expect(within(dialog).getByRole("button", { name })).toBeTruthy();
    }
    expect(
      within(dialog).getAllByRole("button", { name: "Close" }),
    ).toHaveLength(2);
    // On a phone the actions stack full width with the main action on top: it comes last in the markup of a reversed column.
    const put = within(dialog).getByRole("button", {
      name: "Put the agent's files aside",
    });
    const actions = put.parentElement;
    expect(actions?.className).toContain("flex-col-reverse");
    expect(actions?.className).toContain("max-md:*:w-full");
    expect(actions?.lastElementChild).toBe(put);
    await userEvent.click(
      within(dialog).getByRole("button", {
        name: "Put the agent's files aside",
      }),
    );
    const confirm = await screen.findByRole("dialog", {
      name: "Put 3 files aside?",
    });
    await userEvent.click(
      within(confirm).getByRole("button", { name: "Put files aside" }),
    );
    expect(await screen.findByText("Nothing waits for you.")).toBeTruthy();
    expect(posts(calls)).toEqual([`${BASE}/restore`]);
  });

  it("opens each file of the record dialog to its diff", async () => {
    await renderInbox({
      [`POST ${BASE}/record`]: () => heldBack("abc", ["plan.md"]),
    });
    await userEvent.click(screen.getByRole("button", { name: "Record" }));
    const dialog = await screen.findByRole("dialog", {
      name: "Record these files?",
    });
    const emptied = within(dialog).getByRole("region", { name: "Emptied" });
    const row = await within(emptied).findByRole("button", {
      name: /plan\.md/,
    });
    // A row shows a background on no state, so the row the pointer rests on never reads as selected; the keyboard gets the focus ring.
    expect(row.className).not.toMatch(/(^|\s)(hover:)?bg-/);
    expect(row.className).toContain("focus-visible:outline-2");
    await userEvent.click(row);
    expect(await within(emptied).findByText("the plan")).toBeTruthy();
  });

  it("shows the changed shared files with their size and change time, and dismisses them from the dialog", async () => {
    const at = new Date(2026, 10, 7, 14, 12).getTime();
    const { calls, inbox } = await renderInbox(
      {
        "GET /api/v1/projects/demo/shared/changes": () => ({
          body: {
            files: [
              {
                path: "design/home.pen",
                kind: "file",
                size: 2048,
                modified: at,
              },
              {
                path: "design/gone.pen",
                kind: "missing",
                size: null,
                modified: null,
              },
            ],
          },
        }),
        "POST /api/v1/projects/demo/shared/dismiss": () => ({
          body: { dismissed: 1 },
        }),
      },
      [
        needsItem({
          kind: "shared_changed",
          screen: "repository",
          agent: "ops-lead",
          paths: ["design/home.pen", "design/gone.pen"],
        }),
      ],
    );
    await userEvent.click(
      within(inbox).getByRole("button", { name: /show the changes/ }),
    );
    const dialog = await screen.findByRole("dialog", {
      name: "Shared files that changed while an agent worked",
    });
    const home = await within(dialog).findByText("design/home.pen");
    expect(home.parentElement?.textContent).toMatch(
      /2\.0 KB · changed .*14:12/,
    );
    expect(
      within(dialog).getByText("gone from the project folder"),
    ).toBeTruthy();
    const dismiss = within(dialog).getByRole("button", { name: "Dismiss" });
    expect(dismiss.className).toContain("bg-ink");
    expect(dismiss.parentElement?.lastElementChild).toBe(dismiss);
    await userEvent.click(dismiss);
    expect(await screen.findByText("Nothing waits for you.")).toBeTruthy();
    expect(posts(calls)).toEqual(["/api/v1/projects/demo/shared/dismiss"]);
  });
});
