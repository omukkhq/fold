// @vitest-environment jsdom
import { QueryClientProvider } from "@tanstack/react-query";
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
import { Suspense, useState } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { projectsQuery } from "../api/queries";
import { createAppRouter } from "../app/router";
import { createQueryClient } from "../app/queryClient";
import { ThemeProvider } from "../theme/ThemeProvider";
import {
  errorReply,
  shellRoutes,
  stubFetch,
  type StubReply,
  type StubRequest,
} from "../test/stubFetch";
import { workerList } from "../test/workers";
import { ProjectsScreen } from "./projects/ProjectsScreen";
import { RepositoryScreen } from "./repository/RepositoryScreen";

beforeEach(() => {
  // jsdom has no scrolling; the router restores the scroll position on every navigation.
  vi.spyOn(window, "scrollTo").mockImplementation(() => undefined);
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

function renderAt(path: string) {
  const queryClient = createQueryClient();
  const router = createAppRouter(queryClient);
  const history = createMemoryHistory({ initialEntries: [path] });
  router.update({ history, context: { queryClient } });
  render(
    <ThemeProvider>
      <QueryClientProvider client={queryClient}>
        <RouterProvider router={router} />
      </QueryClientProvider>
    </ThemeProvider>,
  );
  return router;
}

/** A project in fold as the list gives it, open, with no overview. */
function summary(slug: string, name: string) {
  return {
    slug,
    name,
    root: `/home/ana/${slug}`,
    available: true,
    last_used_at: 0,
    removed_at: null as number | null,
    folder: "present",
    overview: null,
  };
}

describe("Projects screen", () => {
  it("lists the projects with counts, agents and head, links an available one to its board and marks one that did not open", async () => {
    stubFetch({
      "GET /api/v1/session": { body: { human: "tanim", authenticated: true } },
      "GET /api/v1/projects": {
        body: {
          projects: [
            {
              ...summary("demo", "Demo"),
              overview: {
                counts: {
                  backlog: 2,
                  todo: 3,
                  in_progress: 4,
                  review: 1,
                  done: 7,
                },
                agents: [{ name: "frontend", presence: "active" }],
                head: "r5",
                last_change_at: Date.now() - 12_000,
                attention: [],
                cost: {
                  all_time_micro_usd: 12_340_000,
                  today_micro_usd: 1_200_000,
                  week_micro_usd: 5_670_000,
                },
              },
            },
            { ...summary("archive", "Archive"), available: false },
          ],
        },
      },
    });
    renderAt("/projects");
    const demo = await screen.findByRole("link", { name: /Demo/ });
    expect(demo.getAttribute("href")).toBe("/p/demo/board");
    expect(within(demo).getByText(/17 tasks/)).toBeTruthy();
    expect(within(demo).getByText(/main r5/)).toBeTruthy();
    expect(within(demo).getByText("@frontend")).toBeTruthy();
    expect(within(demo).getByText(/changed \d+ s ago/)).toBeTruthy();
    expect(within(demo).getByText("$12.34")).toBeTruthy();
    expect(within(demo).getByText("today $1.20 · 7 days $5.67")).toBeTruthy();
    expect(screen.getByText(/^All projects/).textContent).toContain("$12.34");
    expect(screen.getByText("Could not open this project.")).toBeTruthy();
    expect(screen.queryByRole("link", { name: /Archive/ })).toBeNull();
  });

  it("names three agents, says how many more, and gives a screen reader every name", async () => {
    stubFetch({
      "GET /api/v1/session": { body: { human: "tanim", authenticated: true } },
      "GET /api/v1/projects": {
        body: {
          projects: [
            {
              ...summary("demo", "Demo"),
              overview: {
                counts: {
                  backlog: 0,
                  todo: 0,
                  in_progress: 0,
                  review: 0,
                  done: 0,
                },
                agents: ["alpha", "beta", "gamma", "delta"].map((name) => ({
                  name,
                  presence: "active",
                })),
                head: "r1",
                last_change_at: Date.now(),
                attention: [],
                cost: {
                  all_time_micro_usd: 0,
                  today_micro_usd: 0,
                  week_micro_usd: 0,
                },
              },
            },
          ],
        },
      },
      "GET /api/v1/projects/demo/git": errorReply(404, "git_not_found"),
    });
    renderAt("/projects");
    const demo = await screen.findByRole("link", {
      name: /@alpha, @beta, @gamma and @delta/,
    });
    const more = within(demo).getByText(/and 1 more/);
    expect(more.getAttribute("aria-hidden")).toBe("true");
    expect(within(demo).queryByText("@delta")).toBeNull();
    expect(more.parentElement?.getAttribute("title")).toBe(
      "@alpha, @beta, @gamma and @delta",
    );
  });

  it("searches every project by name, slug or folder, the removed ones too, and adds a removed one back", async () => {
    let removed: number | null = 5;
    const calls = stubFetch({
      "GET /api/v1/session": { body: { human: "tanim", authenticated: true } },
      "GET /api/v1/projects": () => ({
        body: {
          projects: [
            { ...summary("web", "Web shop"), available: false },
            {
              ...summary("old", "Old site"),
              available: removed === null,
              removed_at: removed,
            },
            {
              ...summary("away", "Away"),
              available: false,
              folder: "folder_missing",
            },
          ],
        },
      }),
      "POST /api/v1/projects/old/restore": () => {
        removed = null;
        return { body: { ...summary("old", "Old site") } };
      },
      "GET /api/v1/projects/old/git": errorReply(404, "git_not_found"),
    });
    renderAt("/projects");
    const list = await screen.findByRole("list", { name: "Projects" });
    expect(within(list).getAllByRole("listitem")).toHaveLength(2);
    expect(within(list).queryByText("Old site")).toBeNull();
    expect(within(list).getByText("folder missing")).toBeTruthy();
    expect(within(list).getByText("/home/ana/away")).toBeTruthy();

    await userEvent.type(
      screen.getByRole("searchbox", { name: "Search projects" }),
      "OLD",
    );
    const results = screen.getByRole("list", { name: "Search results" });
    expect(within(results).getAllByRole("listitem")).toHaveLength(1);
    expect(within(results).getByText("removed")).toBeTruthy();
    expect(within(results).getByText("/home/ana/old")).toBeTruthy();
    await userEvent.click(
      within(results).getByRole("button", {
        name: "Add Old site back to fold",
      }),
    );
    expect(
      await within(results).findByRole("button", {
        name: "More for Old site",
      }),
    ).toBeTruthy();
    expect(calls.map((call) => call.method)).toContain("POST");

    await userEvent.clear(screen.getByRole("searchbox"));
    await userEvent.type(screen.getByRole("searchbox"), "nothing");
    expect(
      screen.getByRole("region", { name: "No project matches" }),
    ).toBeTruthy();
  });

  it("removes a project only after the dialog that says its data stays, and shows a refusal there", async () => {
    let refuse = true;
    let removed: number | null = null;
    stubFetch({
      "GET /api/v1/session": { body: { human: "tanim", authenticated: true } },
      "GET /api/v1/projects": () => ({
        body: {
          projects: [
            {
              ...summary("web", "Web shop"),
              available: false,
              removed_at: removed,
            },
          ],
        },
      }),
      "DELETE /api/v1/projects/web": () => {
        if (refuse) {
          return errorReply(409, "project_workers_running");
        }
        removed = 9;
        return { body: { ...summary("web", "Web shop"), removed_at: 9 } };
      },
    });
    renderAt("/projects");
    await userEvent.click(
      await screen.findByRole("button", { name: "More for Web shop" }),
    );
    await userEvent.click(
      await screen.findByRole("menuitem", { name: "Remove from fold…" }),
    );
    const dialog = await screen.findByRole("dialog", {
      name: "Remove Web shop from fold?",
    });
    expect(within(dialog).getByText("/home/ana/web")).toBeTruthy();
    expect(within(dialog).getByText("fld claude")).toBeTruthy();
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Remove from fold" }),
    );
    expect((await within(dialog).findByRole("alert")).textContent).toContain(
      "Pause the project's agents",
    );

    refuse = false;
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Remove from fold" }),
    );
    expect(
      await screen.findByRole("region", { name: "No projects yet" }),
    ).toBeTruthy();
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("shows the empty state with the next step when there are no projects", async () => {
    const queryClient = createQueryClient();
    queryClient.setQueryData(projectsQuery().queryKey, []);
    render(
      <ThemeProvider>
        <QueryClientProvider client={queryClient}>
          <Suspense>
            <ProjectsScreen />
          </Suspense>
        </QueryClientProvider>
      </ThemeProvider>,
    );
    expect(await screen.findByText("No projects yet")).toBeTruthy();
    expect(
      within(screen.getByRole("region", { name: "No projects yet" })).getByText(
        "fld init <dir>",
      ),
    ).toBeTruthy();
  });
});

const TIME = Date.UTC(2026, 9, 15, 14, 6, 0);

function revision(number: number, fields: Record<string, unknown> = {}) {
  return {
    id: `r${String(number)}`,
    created_at: TIME,
    origin: "proposal",
    author: "frontend/forms",
    parent: number > 1 ? `r${String(number - 1)}` : null,
    proposal: `P-${String(number)}`,
    pull: null,
    task: `T-${String(number)}`,
    title: `Change ${String(number)}`,
    tree: "t",
    ...fields,
  };
}

const REVISIONS = [
  revision(5),
  revision(4, { origin: "pull", pull: "9f2c1ab", proposal: null, task: null }),
  revision(3),
  revision(2),
  revision(1, { origin: "initial", proposal: null, task: null, author: null }),
];

function entry(
  name: string,
  kind: string,
  dir = "",
  size: number | null = 10,
  target?: string,
) {
  return {
    name,
    kind,
    path: dir === "" ? name : `${dir}/${name}`,
    hash: "h",
    size: kind === "directory" ? null : size,
    ...(target === undefined ? {} : { target }),
  };
}

const TREES: Record<string, ReturnType<typeof entry>[]> = {
  "": [
    entry("docs", "directory"),
    entry("README.md", "file"),
    entry("bad\uFFFD.txt", "file"),
    entry("assets", "directory"),
    entry("huge.dat", "file", "", 5_000_000),
  ],
  docs: [entry("latest", "symlink", "docs", 12, "../README.md")],
  assets: [entry("logo.png", "file", "assets", 18_432)],
};

const FILES: Record<string, Record<string, unknown>> = {
  "README.md": {
    kind: "file",
    binary: false,
    too_large: false,
    text: "A demo project.",
  },
  "docs/latest": {
    kind: "symlink",
    binary: false,
    too_large: false,
    text: "../README.md",
  },
  "assets/logo.png": {
    kind: "file",
    binary: true,
    too_large: false,
    text: null,
    size: 18_432,
  },
  "huge.dat": {
    kind: "file",
    binary: false,
    too_large: true,
    text: null,
    size: 5_000_000,
  },
};

function stubRepository(revisions = REVISIONS): StubRequest[] {
  return stubFetch({
    "GET /api/v1/projects/demo/revisions": {
      body: { revisions, before: null },
    },
    "GET /api/v1/projects/demo/repository": {
      body: {
        head: revisions[0]?.id ?? null,
        main_path: "/srv/demo",
        recent: revisions,
      },
    },
    "GET /api/v1/projects/demo/revisions/r1": {
      body: { revision: REVISIONS[4], files: [], totals: {} },
    },
    "GET /api/v1/projects/demo/revisions/r99": errorReply(
      404,
      "revision_not_found",
    ),
    "GET /api/v1/projects/demo/revisions/r1/tree": {
      body: { path: "", entries: [entry("OLD.md", "file")] },
    },
    "GET /api/v1/projects/demo/revisions/r5/tree": (request) => ({
      body: {
        path: request.search.get("path") ?? "",
        entries: TREES[request.search.get("path") ?? ""] ?? [],
      },
    }),
    "GET /api/v1/projects/demo/revisions/r5/file": (request) => {
      const path = request.search.get("path") ?? "";
      return {
        body: { path, size: 3, hash: "b3c1d2e4f5a60718", ...FILES[path] },
      };
    },
  });
}

/** The screen as the route mounts it: the chosen revision lives in the address. */
function renderRepository(
  initial?: string,
  revisions = REVISIONS,
  path?: string,
) {
  stubRepository(revisions);
  const queryClient = createQueryClient();
  const root = createRootRoute();
  function Harness() {
    const [revision, setRevision] = useState(initial);
    return (
      <RepositoryScreen
        slug="demo"
        revision={revision}
        path={path}
        onSelectRevision={setRevision}
      />
    );
  }
  const repository = createRoute({
    getParentRoute: () => root,
    path: "/",
    component: Harness,
  });
  const diff = createRoute({
    getParentRoute: () => root,
    path: "/p/$slug/diff",
    component: () => <p>Diff</p>,
  });
  const router = createRouter({
    routeTree: root.addChildren([repository, diff]),
    history: createMemoryHistory({ initialEntries: ["/"] }),
  });
  render(
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  return router;
}

describe("A project fold removed", () => {
  it("shows the project not-found state, never the load failure, and leads back to a list without it", async () => {
    stubFetch({
      "GET /api/v1/session": { body: { human: "tanim", authenticated: true } },
      "GET /api/v1/projects": {
        body: {
          projects: [{ ...summary("demo", "Demo"), available: false }],
        },
      },
      "GET /api/v1/projects/nobs": errorReply(404, "project_not_found"),
    });
    renderAt("/p/nobs/board");
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("Project not found");
    expect(screen.queryByText(/did not arrive/)).toBeNull();
    await userEvent.click(
      screen.getByRole("link", { name: "Back to projects" }),
    );
    expect(await screen.findByText("Demo")).toBeTruthy();
    expect(screen.queryByText("nobs")).toBeNull();
  });

  it("switches an open project screen to the not-found state when the event stream answers project_not_found", async () => {
    let gone = false;
    let endStream: (reply: StubReply) => void = () => undefined;
    const shell = shellRoutes(["demo"]);
    stubFetch({
      ...shell,
      "GET /api/v1/projects/demo": () =>
        gone
          ? errorReply(404, "project_not_found")
          : { body: { slug: "demo", name: "demo", available: true } },
      "GET /api/v1/projects/demo/agents": { body: { agents: [] } },
      "GET /api/v1/projects/demo/workers": { body: workerList() },
      "GET /api/v1/projects/demo/events": () =>
        new Promise<StubReply>((resolve) => {
          endStream = resolve;
        }),
    });
    renderAt("/p/demo/agents");
    expect(
      await screen.findByRole("region", { name: "No agents yet" }),
    ).toBeTruthy();
    expect(screen.queryByText("Project not found")).toBeNull();

    gone = true;
    endStream(errorReply(404, "project_not_found"));
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("Project not found");
    expect(screen.queryByText(/did not arrive/)).toBeNull();
    expect(screen.queryByRole("region", { name: "No agents yet" })).toBeNull();
  });
});

describe("A not-found inside an open project", () => {
  function stubProject(
    batch: StubReply = errorReply(404, "review_batch_not_found"),
  ) {
    stubFetch({
      ...shellRoutes(["demo"]),
      "GET /api/v1/projects/demo/workers": { body: workerList() },
      "GET /api/v1/projects/demo/events": () =>
        new Promise<StubReply>(() => undefined),
      "GET /api/v1/projects/demo/reviews/batches/rb-1": batch,
    });
  }

  /** The not-found state under the project's top bar and tabs, never the project not-found state, and no failure. */
  async function missingInShell(title: string, back: string) {
    const panel = await screen.findByRole("region", { name: title });
    expect(screen.queryByRole("alert")).toBeNull();
    expect(
      within(panel)
        .getAllByRole("link")
        .map((link) => [link.textContent, link.getAttribute("href")]),
    ).toEqual([[expect.stringContaining("Back to") as string, back]]);
    expect(screen.getByRole("navigation", { name: "Project" })).toBeTruthy();
    expect(screen.queryByText("Project not found")).toBeNull();
    return panel;
  }

  it("says a batch the daemon does not have is not found and leads to the proposals", async () => {
    stubProject();
    renderAt("/p/demo/batches/rb-1");
    const panel = await missingInShell(
      "Batch rb-1 not found",
      "/p/demo/proposals",
    );
    expect(panel.textContent).toContain("no review batch with this id");
  });

  it("shows the error state, not a missing batch, when the daemon fails to answer for the batch", async () => {
    stubProject(errorReply(500, "internal"));
    renderAt("/p/demo/batches/rb-1");
    expect(await screen.findByText("This screen could not load.")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Retry" })).toBeTruthy();
    expect(screen.getByRole("navigation", { name: "Project" })).toBeTruthy();
    expect(screen.queryByText(/not found/)).toBeNull();
  });

  it("says the same of an id that is no batch id", async () => {
    stubProject();
    renderAt("/p/demo/batches/P-1");
    await missingInShell("Batch P-1 not found", "/p/demo/proposals");
  });

  it("names a malformed task, proposal or request id and leads to the board or its list", async () => {
    stubProject();
    renderAt("/p/demo/t/abc");
    await missingInShell("Task abc not found", "/p/demo/board");
    cleanup();
    renderAt("/p/demo/proposals/xyz");
    await missingInShell("Proposal xyz not found", "/p/demo/proposals");
    cleanup();
    renderAt("/p/demo/requests/RQ-x");
    await missingInShell("Request RQ-x not found", "/p/demo/requests");
  });

  it("says a page the project does not have is not found", async () => {
    stubProject();
    renderAt("/p/demo/nowhere");
    await missingInShell("Page not found", "/p/demo/board");
    cleanup();
    renderAt("/p/demo/diff");
    await missingInShell("Page not found", "/p/demo/board");
  });

  it("keeps the project not-found state for an address that is no project slug", async () => {
    stubFetch(shellRoutes(["demo"]));
    renderAt("/p/Not_A_Slug/board");
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("Project not found");
    expect(screen.queryByRole("navigation", { name: "Project" })).toBeNull();
  });
});

describe("The top bar", () => {
  it("links the wordmark to the projects page and switches among the five most recent projects", async () => {
    const slugs = ["a", "b", "c", "d", "e", "f"];
    const shell = shellRoutes(slugs);
    const projects = slugs.map((slug, index) => ({
      ...summary(slug, slug === "c" ? "f" : slug),
      last_used_at: index,
    }));
    stubFetch({
      ...shell,
      "GET /api/v1/projects": { body: { projects } },
      "GET /api/v1/projects/demo": {
        body: { slug: "demo", name: "demo", available: true },
      },
      "GET /api/v1/projects/f/workers": { body: workerList() },
      "GET /api/v1/projects/f/events": () => new Promise<StubReply>(() => {}),
    });
    renderAt("/p/f/agents");
    const wordmark = await screen.findByRole("link", {
      name: "fold, all projects",
    });
    expect(wordmark.getAttribute("href")).toBe("/projects");

    await userEvent.click(
      screen.getByRole("button", { name: "Project f, switch project" }),
    );
    const items = await screen.findAllByRole("menuitemradio");
    // Two listed projects named f show their slugs beside the name.
    expect(items.map((item) => item.textContent)).toEqual([
      "ff",
      "e",
      "d",
      "fc",
      "b",
    ]);
    expect(items[0]?.getAttribute("aria-checked")).toBe("true");
    await userEvent.click(screen.getByRole("menuitem", { name: "See all" }));
    expect(
      await screen.findByRole("heading", { name: "Projects" }),
    ).toBeTruthy();
  });
});

describe("The avatar menu", () => {
  it("names the signed-in human, holds the theme and signs out to the sign-in screen", async () => {
    let signedIn = true;
    const calls = stubFetch({
      ...shellRoutes(["f"]),
      "GET /api/v1/session": () => ({
        body: signedIn
          ? { human: "tanim", authenticated: true }
          : { human: null, authenticated: false },
      }),
      "GET /api/v1/projects/f/workers": { body: workerList() },
      "GET /api/v1/projects/f/events": () => new Promise<StubReply>(() => {}),
      "DELETE /api/v1/session": () => {
        signedIn = false;
        return { status: 204 };
      },
    });
    const router = renderAt("/p/f/agents");
    const button = await screen.findByRole("button", {
      name: "Your menu, signed in as @tanim",
    });
    expect(button.textContent).toBe("TA");
    await userEvent.click(button);
    const menu = await screen.findByRole("dialog", { name: "Your menu" });
    expect(menu.textContent).toContain("Signed in as@tanim");
    const dark = within(menu).getByRole("button", { name: "Dark" });
    await userEvent.click(dark);
    expect(dark.getAttribute("aria-pressed")).toBe("true");
    expect(document.documentElement.dataset.theme).toBe("dark");
    await userEvent.click(within(menu).getByRole("button", { name: "System" }));
    expect(document.documentElement.dataset.theme).toBeUndefined();
    await userEvent.click(
      within(menu).getByRole("button", { name: "Sign out" }),
    );
    await waitFor(() => {
      expect(router.state.location.pathname).toBe("/sign-in");
    });
    expect(calls.some((c) => c.method === "DELETE")).toBe(true);
  });
});

describe("Repository screen", () => {
  it("opens a mentioned folder as the tree at that folder and a mentioned file as a file", async () => {
    renderRepository(undefined, REVISIONS, "assets");
    expect(await screen.findByText("logo.png")).toBeTruthy();
    expect(screen.queryByRole("region", { name: "File assets" })).toBeNull();
    expect(screen.queryByText("The file could not load.")).toBeNull();
    cleanup();
    renderRepository(undefined, REVISIONS, "README.md");
    expect(
      await screen.findByRole("region", { name: "File README.md" }),
    ).toBeTruthy();
    expect(await screen.findByText("A demo project.")).toBeTruthy();
  });

  it("lists the main revisions newest first with their source and task", async () => {
    renderRepository();
    const revisions = await screen.findByRole("list", {
      name: "Main revisions",
    });
    const rows = within(revisions).getAllByRole("button");
    expect(rows.map((r) => r.textContent.slice(0, 2))).toEqual([
      "r5",
      "r4",
      "r3",
      "r2",
      "r1",
    ]);
    expect(
      within(revisions).getByText("Pull of the remote commit 9f2c1ab"),
    ).toBeTruthy();
    expect(within(revisions).getByText("Change 5")).toBeTruthy();
    expect(within(revisions).getByText(/P-5 for T-/)).toBeTruthy();
    expect(within(rows[0] ?? revisions).getByText("head of main")).toBeTruthy();
    expect(within(revisions).getAllByText("head of main")).toHaveLength(1);
  });

  it("names main's head from the repository route under the tree", async () => {
    renderRepository("r3");
    expect(
      await screen.findByText(
        /The project folder \/srv\/demo shows r5, apart from your changes that are not recorded/,
      ),
    ).toBeTruthy();
  });

  it("opens a symlink to show its target without following it", async () => {
    renderRepository();
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: "docs" }));
    expect(await screen.findByText("→ ../README.md")).toBeTruthy();
    await user.click(await screen.findByRole("button", { name: /latest/ }));
    const link = await screen.findByRole("region", {
      name: "File docs/latest",
    });
    expect(await within(link).findByText("→ ../README.md")).toBeTruthy();
  });

  it("opens a text file", async () => {
    renderRepository();
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: /README\.md/ }));
    const text = await screen.findByRole("region", { name: "File README.md" });
    expect(await within(text).findByText(/A demo project/)).toBeTruthy();
  });

  it("opens a binary file with its size and hash", async () => {
    renderRepository();
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: "assets" }));
    await user.click(await screen.findByRole("button", { name: /logo\.png/ }));
    const binary = await screen.findByRole("region", {
      name: "File assets/logo.png",
    });
    expect(await within(binary).findByText(/Binary file/)).toBeTruthy();
    expect(within(binary).getByText(/18.0 KB · b3c1d2e4f5a60718/)).toBeTruthy();
  });

  it("shows the too-large state for a file above the inline cap", async () => {
    renderRepository();
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: /huge\.dat/ }));
    const big = await screen.findByRole("region", { name: "File huge.dat" });
    expect(await within(big).findByText(/File too large/)).toBeTruthy();
    expect(within(big).getByText(/4\.8 MB · b3c1d2e4f5a60718/)).toBeTruthy();
    expect(within(big).queryByRole("code")).toBeNull();
  });

  it("shows an entry whose name is not UTF-8 but never opens it", async () => {
    const calls = stubRepository();
    renderRepository();
    const row = await screen.findByText("bad\uFFFD.txt");
    expect(row).toBeTruthy();
    expect(screen.queryByRole("button", { name: /bad/ })).toBeNull();
    expect(screen.getByText(/not openable/)).toBeTruthy();
    expect(calls.some((c) => c.path.endsWith("/file"))).toBe(false);
  });

  it("compares two chosen revisions", async () => {
    const router = renderRepository();
    const user = userEvent.setup();
    expect(
      await screen.findByRole("button", { name: "Compare r4 → r5" }),
    ).toBeTruthy();
    await user.selectOptions(screen.getByLabelText("From"), "r5");
    await user.selectOptions(screen.getByLabelText("To"), "r2");
    await user.click(screen.getByRole("button", { name: "Compare r2 → r5" }));
    expect(router.state.location.href).toBe("/p/demo/diff?from=r2&to=r5");
  });

  it("rejects comparing a revision with itself", async () => {
    const router = renderRepository();
    const user = userEvent.setup();
    await screen.findByRole("list", { name: "Main revisions" });
    await user.selectOptions(screen.getByLabelText("From"), "r5");
    await user.click(screen.getByRole("button", { name: "Compare" }));
    expect(screen.getByText("Choose two different revisions.")).toBeTruthy();
    expect(screen.getByLabelText("To").getAttribute("aria-invalid")).toBe(
      "true",
    );
    expect(router.state.location.pathname).toBe("/");
  });

  it("has nothing to compare with only r1", async () => {
    renderRepository(undefined, [revision(1, { origin: "initial" })]);
    await screen.findByRole("list", { name: "Main revisions" });
    expect(screen.queryByRole("button", { name: /^Compare/ })).toBeNull();
    expect(screen.getByText(/Only r1 exists/)).toBeTruthy();
  });

  it("opens the revision named in the address and says an unknown one does not exist", async () => {
    renderRepository("r3");
    const list = await screen.findByRole("list", { name: "Main revisions" });
    const current = within(list)
      .getAllByRole("button")
      .find((b) => b.getAttribute("aria-current") === "true");
    expect(current?.textContent.startsWith("r3")).toBe(true);
    cleanup();
    renderRepository("r99");
    expect(await screen.findByText("r99 does not exist")).toBeTruthy();
    expect(screen.queryByRole("list", { name: "Main revisions" })).toBeNull();
  });

  it("reads a revision the listing did not reach and shows its tree", async () => {
    renderRepository("r1", REVISIONS.slice(0, 3));
    expect(await screen.findByText("OLD.md")).toBeTruthy();
    expect(screen.queryByText("README.md")).toBeNull();
    const list = screen.getByRole("list", { name: "Main revisions" });
    expect(
      within(list)
        .getAllByRole("button")
        .some((b) => b.getAttribute("aria-current") === "true"),
    ).toBe(false);
  });

  it("compares the chosen revision with the one before it", async () => {
    renderRepository();
    const user = userEvent.setup();
    const list = await screen.findByRole("list", { name: "Main revisions" });
    const r3 = within(list)
      .getAllByRole("button")
      .find((b) => b.textContent.startsWith("r3"));
    if (!r3) {
      throw new Error("no r3 row");
    }
    await user.click(r3);
    expect(
      await screen.findByRole("button", { name: "Compare r2 → r3" }),
    ).toBeTruthy();
    const current = within(screen.getByRole("list", { name: "Main revisions" }))
      .getAllByRole("button")
      .find((b) => b.getAttribute("aria-current") === "true");
    expect(current?.textContent.startsWith("r3")).toBe(true);
  });
});
