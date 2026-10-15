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
  act,
  cleanup,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { keys } from "../../api/keys";
import type { WorkspaceEntry } from "../../api/workspaces";
import { applyEvent } from "../../live/apply";
import type { LiveWorkspace } from "../../live/envelope";
import recordedWorkspaces from "../../live/recorded/workspaces.json";
import { liveStatusStore } from "../../live/status";
import { workspaceEntry } from "../../test/payloads";
import {
  errorReply,
  stubFetch,
  type StubReply,
  type StubRequest,
} from "../../test/stubFetch";
import { LiveActivityScreen } from "./LiveActivityScreen";

const NOW = Date.now();
const BASE = "/api/v1/projects/demo";

function live(workspace: string, paths: string[], at = NOW): LiveWorkspace {
  return {
    workspace,
    task: "T-1",
    worker: "frontend/ui",
    base: "r7",
    files: paths.map((path) => ({
      path,
      previous_path: null,
      change: "modified" as const,
      added_lines: 3,
      removed_lines: 1,
      binary: false,
    })),
    summary: "",
    last_activity_at: at,
    checkpoints: [
      { id: "ck-1", label: null, automatic: false, created_at: NOW },
    ],
    proposal: null,
    stalled: false,
  };
}

function entry(
  workspace: string,
  patch: Partial<WorkspaceEntry> = {},
): WorkspaceEntry {
  return workspaceEntry(workspace, {
    last_activity_at: NOW,
    live: live(workspace, ["src/a.ts"]),
    ...patch,
  });
}

function revision(id: string, parent: string | null) {
  return {
    id,
    parent,
    created_at: NOW,
    tree: "t",
    origin: "proposal",
    task: null,
    proposal: null,
    title: null,
    author: null,
    pull: null,
  };
}

interface Daemon {
  /** The body of the workspace list, or a reply such as an error. */
  list: () => Promise<StubReply> | StubReply;
}

let daemon: Daemon;
let calls: StubRequest[];

function answer(workspaces: unknown[]): StubReply {
  return { body: { workspaces } };
}

function serve() {
  calls = stubFetch({
    [`GET ${BASE}/workspaces`]: () => daemon.list(),
    [`GET ${BASE}/events/history`]: { body: { events: [], before: null } },
    [`GET ${BASE}/reviews/checkouts`]: { body: { checkouts: [] } },
    [`GET ${BASE}/repository`]: {
      body: {
        head: "r9",
        main_path: "main",
        recent: [
          revision("r9", "r8"),
          revision("r8", "r7"),
          revision("r7", null),
        ],
      },
    },
  });
}

let client: QueryClient;

function mount() {
  const root = createRootRoute();
  const index = createRoute({
    getParentRoute: () => root,
    path: "/",
    component: () => <LiveActivityScreen slug="demo" />,
  });
  const router = createRouter({
    routeTree: root.addChildren([index]),
    history: createMemoryHistory({ initialEntries: ["/"] }),
  });
  render(
    <QueryClientProvider client={client}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  liveStatusStore.reset();
  daemon = { list: () => answer([]) };
  serve();
});
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("LiveActivityScreen", () => {
  it("shows the loading skeleton, then a panel with three zones", async () => {
    let resolve: (value: StubReply) => void = () => {};
    daemon.list = () =>
      new Promise((r) => {
        resolve = r;
      });
    mount();
    expect(
      await screen.findAllByRole("status", { name: "Loading" }),
    ).toHaveLength(2);
    resolve(answer([entry("ws-1")]));
    const panel = await screen.findByRole("region", { name: "Workspace ws-1" });
    expect(
      within(panel).getByRole("heading", { name: /Working changes/ })
        .textContent,
    ).toContain("not recorded");
    expect(
      within(panel).getByRole("heading", { name: "Recorded changes" }),
    ).toBeTruthy();
    expect(within(panel).getByRole("heading", { name: "Main" })).toBeTruthy();
    expect(
      await within(panel).findByText(
        "Main is at r9; base r7 is 2 revisions behind.",
      ),
    ).toBeTruthy();
    expect(
      await within(panel).findByRole("heading", { name: "Title T-1" }),
    ).toBeTruthy();
    expect(within(panel).getByText("@frontend/ui")).toBeTruthy();
  });

  it("links a working file, a checkpoint, a changeset and the task", async () => {
    daemon.list = () =>
      answer([
        entry("ws-1", {
          changesets: [
            { id: "C-18", message: "Fields", recorded_at: NOW, base: "r7" },
          ],
        }),
      ]);
    mount();
    const panel = await screen.findByRole("region", { name: "Workspace ws-1" });
    const hrefs = within(panel)
      .getAllByRole("link")
      .map((a) => a.getAttribute("href"));
    expect(hrefs).toContain("/p/demo/t/T-1");
    expect(hrefs).toContain("/p/demo/diff?from=r7&to=ws-1&path=src%2Fa.ts");
    expect(hrefs).toContain("/p/demo/diff?from=r7&to=ck-1");
    expect(hrefs).toContain("/p/demo/diff?from=r7&to=C-18");
  });

  it("updates the file list from a workspace_changed event and drops a panel on a snapshot", async () => {
    daemon.list = () =>
      answer([
        entry("ws-1"),
        entry("ws-2", { last_activity_at: 1, live: live("ws-2", [], 1) }),
      ]);
    mount();
    const panel = await screen.findByRole("region", { name: "Workspace ws-1" });
    expect(within(panel).getAllByRole("link", { name: /\.ts/ })).toHaveLength(
      1,
    );
    act(() => {
      applyEvent(client, "demo", {
        kind: "workspace_changed",
        ...live("ws-1", ["src/a.ts", "src/b.ts"], NOW + 5),
      });
    });
    await waitFor(() => {
      expect(within(panel).getAllByRole("link", { name: /\.ts/ })).toHaveLength(
        2,
      );
    });
    expect(within(panel).getByText(/2 files, \+6 -2/)).toBeTruthy();
    act(() => {
      applyEvent(client, "demo", {
        kind: "workspace_changed",
        ...live("ws-1", ["src/a.ts"], NOW + 6),
        base: "r9",
      });
    });
    await waitFor(() => {
      expect(
        within(panel)
          .getByRole("link", { name: /src\/a\.ts/ })
          .getAttribute("href"),
      ).toBe("/p/demo/diff?from=r9&to=ws-1&path=src%2Fa.ts");
    });
    act(() => {
      applyEvent(client, "demo", {
        kind: "snapshot",
        seq: 3,
        workspaces: [live("ws-2", [], 1)],
      });
    });
    await waitFor(() => {
      expect(
        screen.queryByRole("region", { name: "Workspace ws-1" }),
      ).toBeNull();
    });
    expect(screen.getByRole("region", { name: "Workspace ws-2" })).toBeTruthy();
    expect(
      client.getQueryData(keys.workspaceLive("demo", "ws-1")),
    ).toBeUndefined();
  });

  it("reads one workspace list and the stream, and a change frame asks the daemon for nothing", async () => {
    daemon.list = () => answer([entry("ws-1")]);
    mount();
    const panel = await screen.findByRole("region", { name: "Workspace ws-1" });
    await within(panel).findByText(
      "Main is at r9; base r7 is 2 revisions behind.",
    );
    const before = calls.length;
    act(() => {
      applyEvent(client, "demo", {
        kind: "workspace_changed",
        ...live("ws-1", ["src/a.ts", "src/b.ts"], NOW + 5),
      });
    });
    await waitFor(() => {
      expect(within(panel).getAllByRole("link", { name: /\.ts/ })).toHaveLength(
        2,
      );
    });
    expect(calls.slice(before)).toEqual([]);
    const paths = calls.map((call) => call.path);
    expect(paths.filter((path) => path.endsWith("/changes"))).toEqual([]);
    expect(paths.filter((path) => path.includes("/tasks/"))).toEqual([]);
    expect(paths.filter((path) => path === `${BASE}/workspaces`)).toHaveLength(
      1,
    );
  });

  it("keeps a panel where it is while its files change", async () => {
    daemon.list = () =>
      answer([
        entry("ws-1", { last_activity_at: 10, live: live("ws-1", [], 10) }),
        entry("ws-2", { last_activity_at: 20, live: live("ws-2", [], 20) }),
      ]);
    mount();
    await screen.findByRole("region", { name: "Workspace ws-1" });
    const order = () =>
      screen
        .getAllByRole("region", { name: /^Workspace/ })
        .map((r) => r.getAttribute("aria-label"));
    expect(order()).toEqual(["Workspace ws-2", "Workspace ws-1"]);
    act(() => {
      applyEvent(client, "demo", {
        kind: "workspace_changed",
        ...live("ws-1", ["x.ts"], 99),
      });
    });
    await screen.findByRole("link", { name: /x\.ts/ });
    expect(order()).toEqual(["Workspace ws-2", "Workspace ws-1"]);
  });

  it("shows a missing workspace without a file list and a degraded watcher notice", async () => {
    daemon.list = () =>
      answer([
        entry("ws-1", { watch: "degraded" }),
        entry("ws-2", { state: "missing", live: null, last_activity_at: 1 }),
      ]);
    mount();
    const missing = await screen.findByRole("region", {
      name: "Workspace ws-2",
    });
    expect(within(missing).getByText("missing")).toBeTruthy();
    expect(
      within(missing).queryByRole("heading", { name: /Working changes/ }),
    ).toBeNull();
    const degraded = screen.getByRole("region", { name: "Workspace ws-1" });
    expect((await within(degraded).findByRole("note")).textContent).toContain(
      "10 second scan",
    );
  });

  it("shows the stalled and stale_claim items of a recorded workspace payload as different badges", async () => {
    const recorded: unknown = recordedWorkspaces;
    daemon.list = () => ({ body: recorded });
    mount();
    const stalled = await screen.findByRole("region", {
      name: "Workspace ws-1",
    });
    expect(within(stalled).getByText("Stalled")).toBeTruthy();
    expect(within(stalled).getByText(/^no edits for /)).toBeTruthy();
    expect(within(stalled).queryByText("Stale")).toBeNull();
    const stale = screen.getByRole("region", { name: "Workspace ws-2" });
    expect(within(stale).getByText("Stale")).toBeTruthy();
    expect(within(stale).queryByText("Stalled")).toBeNull();
    expect(within(stale).getByText("@backend")).toBeTruthy();
  });

  it("collapses long file lists behind Show more", async () => {
    const paths = ["a", "b", "c", "d", "e"].map((p) => `${p}.ts`);
    daemon.list = () => answer([entry("ws-1", { live: live("ws-1", paths) })]);
    mount();
    const panel = await screen.findByRole("region", { name: "Workspace ws-1" });
    expect(within(panel).getAllByRole("link", { name: /\.ts/ })).toHaveLength(
      3,
    );
    await userEvent.click(
      within(panel).getByRole("button", { name: "Show 2 more" }),
    );
    expect(within(panel).getAllByRole("link", { name: /\.ts/ })).toHaveLength(
      5,
    );
  });

  it("shows the empty panel when no workspace is active", async () => {
    mount();
    expect(
      await screen.findByRole("heading", { name: "No active workspaces" }),
    ).toBeTruthy();
  });

  it("shows the error banner and Retry loads again", async () => {
    let failures = 1;
    daemon.list = () => {
      if (failures > 0) {
        failures -= 1;
        return errorReply(500, "internal_error");
      }
      return answer([]);
    };
    mount();
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("Activity could not load.");
    await userEvent.click(within(alert).getByRole("button", { name: "Retry" }));
    expect(
      await screen.findByRole("heading", { name: "No active workspaces" }),
    ).toBeTruthy();
  });

  it("shows the reconnecting banner while the stream is down", async () => {
    liveStatusStore.set({ state: "reconnecting", stateAsOf: NOW - 5000 });
    mount();
    const banner = await screen.findByRole("status");
    expect(banner.textContent).toContain("Live updates paused");
  });
});
