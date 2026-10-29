// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  Outlet,
  RouterProvider,
  createMemoryHistory,
  createRootRoute,
  createRoute,
  createRouter,
} from "@tanstack/react-router";
import { act, cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import type { TaskStatus } from "@/api/board";
import { notificationStore } from "@/features/notifications/store";
import type { EventEnvelope } from "@/live/envelope";
import { keys } from "@/api/keys";
import type { NeedsYouItem } from "@/api/workers";
import { stubFetch } from "@/test/stubFetch";
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

function moved(
  seq: number,
  task: string,
  to: TaskStatus,
  from: TaskStatus = "in_progress",
): EventEnvelope {
  return {
    kind: "event",
    seq,
    type: "task.status_changed",
    actor: "fold",
    task,
    proposal: null,
    workspace: null,
    revision: null,
    data: { from, to },
    created_at: Date.now() + seq,
  };
}

function receive(slug: string, event: EventEnvelope) {
  act(() => {
    notificationStore.receive(slug, event, {
      replay: true,
      title: (task) => (task === "T-12" ? "Add the bell" : null),
    });
  });
}

async function renderInbox(
  items: NeedsYouItem[],
  human: string | null,
  slug = "demo",
) {
  stubFetch({
    "GET /api/v1/session": { body: { human, authenticated: true } },
    [`GET /api/v1/projects/${slug}/needs-you`]: { body: { items } },
  });
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const root = createRootRoute({
    component: () => (
      <>
        <Inbox slug={slug} />
        <Outlet />
      </>
    ),
  });
  const index = createRoute({
    getParentRoute: () => root,
    path: "/",
    component: () => <p>Home</p>,
  });
  const agents = createRoute({
    getParentRoute: () => root,
    path: "/p/$slug/agents",
    component: () => <p>Agents screen</p>,
  });
  const task = createRoute({
    getParentRoute: () => root,
    path: "/p/$slug/t/$task",
    component: function TaskScreen() {
      const { task: id } = task.useParams();
      return <p>Task screen {id}</p>;
    },
  });
  const router = createRouter({
    routeTree: root.addChildren([index, agents, task]),
    history: createMemoryHistory({ initialEntries: ["/"] }),
  });
  render(
    <QueryClientProvider client={client}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  await screen.findByText("Home");
  return { client, router };
}

const permission = needsItem({
  kind: "permission",
  screen: "worker",
  question: "pq-3",
  agent: "web-worker",
  tool: "Bash",
  task: "T-12",
});

const openInbox = async (name: RegExp = /^Inbox/) => {
  await userEvent.click(await screen.findByRole("button", { name }));
  return screen.getByRole("dialog", { name: "Inbox" });
};

describe("Inbox, Needs you", () => {
  it("shows no gold count while nothing waits, and opens on the card moves", async () => {
    const { client } = await renderInbox([], "tanim", "quiet");
    await vi.waitFor(() => {
      expect(client.getQueryData(keys.needsYou("quiet"))).toEqual([]);
    });
    const button = screen.getByRole("button", { name: "Inbox" });
    expect(button.textContent).toBe("Inbox");
    const inbox = await openInbox();
    expect(
      within(inbox)
        .getByRole("tab", { name: /Updates/ })
        .getAttribute("aria-selected"),
    ).toBe("true");
    await userEvent.click(
      within(inbox).getByRole("tab", { name: "Needs you" }),
    );
    expect(within(inbox).getByText("Nothing waits for you.")).toBeTruthy();
  });

  it("counts the items for this human and opens each item's screen", async () => {
    const { router } = await renderInbox(
      [
        permission,
        needsItem({
          kind: "human_review",
          screen: "proposal",
          proposal: "P-4",
          human: "ana",
        }),
      ],
      "tanim",
    );
    const button = await screen.findByRole("button", {
      name: "Inbox, 1 waits for you",
    });
    expect(button.textContent).toBe("Inbox1");
    const inbox = await openInbox();
    const ask = within(inbox).getByRole("region", { name: "Agents ask you" });
    expect(ask.textContent).toContain("@web-worker asks to run Bash on T-12");
    expect(
      within(inbox).queryByRole("region", { name: "Waiting for you" }),
    ).toBeNull();
    await userEvent.click(within(inbox).getByRole("link"));
    expect(await screen.findByText("Agents screen")).toBeTruthy();
    expect(router.state.location.search).toEqual({ question: "pq-3" });
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("ends the sentence of a review or a draft with its title", async () => {
    await renderInbox(
      [
        needsItem({
          kind: "human_review",
          proposal: "P-4",
          title: "Audit log",
        }),
        needsItem({ kind: "request_draft", request: "RQ-5" }),
      ],
      null,
    );
    const inbox = await openInbox();
    const wait = within(inbox).getByRole("region", { name: "Waiting for you" });
    const [review, draft] = within(wait).getAllByRole("listitem");
    expect(review?.textContent).toContain("P-4 needs your review: Audit log");
    expect(draft?.textContent).toContain("RQ-5 has a draft ready to approve");
  });

  it("keeps each id on one line, cuts a long title and gives its whole text on keyboard focus, without a native title", async () => {
    const title = "Record changes in an audit log for every write";
    await renderInbox(
      [
        needsItem({
          kind: "human_review",
          screen: "proposal",
          proposal: "P-4",
          task: "T-7",
          title,
        }),
      ],
      null,
    );
    const inbox = await openInbox();
    expect(within(inbox).getByText("P-4").className).toContain(
      "whitespace-nowrap",
    );
    const sentence = within(inbox).getByText(title).closest(".line-clamp-2");
    expect(sentence).not.toBeNull();
    expect(inbox.querySelector("[title]")).toBeNull();
    expect(screen.queryByRole("tooltip")).toBeNull();
    const link = within(inbox).getByRole("link");
    act(() => {
      link.focus();
    });
    expect((await screen.findByRole("tooltip")).textContent).toBe(title);
  });

  it("shows an unstash conflict as a line that opens nothing and counts it", async () => {
    await renderInbox(
      [
        permission,
        needsItem({
          kind: "unstash_conflicted",
          screen: "repository",
          paths: ["src/forms/index.ts", "a.ts", "b.ts"],
        }),
      ],
      null,
    );
    const inbox = await openInbox(/^Inbox, 2 wait for you/);
    const project = within(inbox).getByRole("region", { name: "Project" });
    expect(within(project).getByText("fld record").tagName).toBe("CODE");
    expect(project.textContent).toContain(
      "Unstash conflicts. Fix the files and run fld record",
    );
    expect(
      within(project).getByText("src/forms/index.ts and 2 more"),
    ).toBeTruthy();
    expect(within(project).queryByRole("link")).toBeNull();
    expect(within(inbox).getAllByRole("link")).toHaveLength(1);
  });

  it("shows the command that brings the Git branch back as caption code, and the paths of an item as plain words", async () => {
    await renderInbox(
      [
        needsItem({
          kind: "head_moved",
          screen: "repository",
          reason: "run `git switch main` in the project folder",
        }),
        needsItem({
          kind: "folder_held",
          screen: "repository",
          paths: ["a`b`.md"],
        }),
      ],
      null,
    );
    const inbox = await openInbox();
    const project = within(inbox).getByRole("region", { name: "Project" });
    const command = within(project).getByText("git switch main");
    expect(command.tagName).toBe("CODE");
    expect(command.className).toContain("text-13");
    expect(within(project).getByText("a`b`.md").tagName).toBe("SPAN");
  });

  it("shows main's invalid checks file as a line that opens nothing and counts it", async () => {
    await renderInbox(
      [
        permission,
        needsItem({
          kind: "checks_file_invalid_on_main",
          screen: "repository",
          title: "fold-checks.toml line 12: unknown field `tiemout`",
          paths: ["fold-checks.toml"],
        }),
      ],
      null,
    );
    const inbox = await openInbox(/^Inbox, 2 wait for you/);
    const project = within(inbox).getByRole("region", { name: "Project" });
    expect(
      within(project).getByText(
        "Integrations wait until the checks file on main is fixed",
      ),
    ).toBeTruthy();
    expect(
      within(project).getByText(
        "fold-checks.toml line 12: unknown field `tiemout`",
      ),
    ).toBeTruthy();
    expect(within(project).queryByRole("link")).toBeNull();
  });
});

describe("Inbox, Updates", () => {
  it("groups the moves of one task in one row and marks them read once shown", async () => {
    await renderInbox([permission], null, "count");
    receive("count", moved(1, "T-12", "review"));
    receive("count", moved(2, "T-14", "done"));
    receive("count", moved(3, "T-12", "done", "review"));
    await screen.findByRole("button", {
      name: "Inbox, 1 waits for you, 3 new updates",
    });
    const inbox = await openInbox();
    const updates = within(inbox).getByRole("tab", { name: /Updates/ });
    expect(updates.textContent).toBe("Updates3 new");
    await userEvent.click(updates);
    const rows = within(inbox).getAllByRole("listitem");
    expect(rows).toHaveLength(2);
    expect(
      within(inbox).getByRole("link", {
        name: /^T-12 moved In progress to Review to Done/,
      }),
    ).toBeTruthy();
    expect(
      within(inbox).getByRole("link", {
        name: /^T-14 moved In progress to Done/,
      }),
    ).toBeTruthy();
    expect(within(inbox).getByText("Add the bell")).toBeTruthy();
    expect(
      screen.getByRole("button", { name: "Inbox, 1 waits for you" }),
    ).toBeTruthy();
  });

  it("opens the task of a row", async () => {
    await renderInbox([], null, "open");
    receive("open", moved(1, "T-12", "review"));
    const inbox = await openInbox();
    await userEvent.click(
      within(inbox).getByRole("link", { name: /T-12 moved/ }),
    );
    expect(await screen.findByText("Task screen T-12")).toBeTruthy();
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("clears the moves of one task and then every move", async () => {
    await renderInbox([], null, "clear");
    receive("clear", moved(1, "T-12", "review"));
    receive("clear", moved(2, "T-14", "done"));
    receive("clear", moved(3, "T-14", "review", "done"));
    const inbox = await openInbox();
    await userEvent.click(
      within(inbox).getByRole("button", { name: "Clear the moves of T-14" }),
    );
    expect(within(inbox).getAllByRole("listitem")).toHaveLength(1);
    expect(notificationStore.feed("clear").map((e) => e.task)).toEqual([
      "T-12",
    ]);
    await userEvent.click(
      within(inbox).getByRole("button", { name: "Clear all" }),
    );
    expect(within(inbox).queryAllByRole("listitem")).toHaveLength(0);
    expect(within(inbox).getByText("No card moves")).toBeTruthy();
    expect(
      within(inbox).queryByRole("button", { name: "Clear all" }),
    ).toBeNull();
  });
});
