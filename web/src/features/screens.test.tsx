// @vitest-environment jsdom
import { QueryClientProvider } from "@tanstack/react-query";
import {
  RouterProvider,
  createMemoryHistory,
  createRootRoute,
  createRoute,
  createRouter,
} from "@tanstack/react-router";
import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Suspense, useState } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { keys } from "../api/keys";
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

describe("Projects screen", () => {
  it("lists the projects with counts, agents and head, links an available one to its board and marks one that did not open", async () => {
    stubFetch({
      "GET /api/v1/session": { body: { human: "tanim", authenticated: true } },
      "GET /api/v1/projects": {
        body: {
          projects: [
            {
              slug: "demo",
              name: "Demo",
              available: true,
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
              },
            },
            { slug: "archive", name: "Archive", available: false },
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
    expect(screen.getByText("Could not open this project.")).toBeTruthy();
    expect(screen.queryByRole("link", { name: /Archive/ })).toBeNull();
  });

  it("shows the empty state with the next step when there are no projects", async () => {
    const queryClient = createQueryClient();
    queryClient.setQueryData(keys.projects(), []);
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
        body: { projects: [{ slug: "demo", name: "Demo", available: false }] },
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
    expect(within(revisions).getByText(/pull · 9f2c1ab/)).toBeTruthy();
    expect(within(revisions).getByText(/P-5 · Change 5/)).toBeTruthy();
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
