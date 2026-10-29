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
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Comparison } from "@/api/diff";
import {
  add,
  comparedFile,
  comparison,
  ctx,
  del,
  fileDiff,
  hunk,
  state,
} from "@/test/diff";
import { requestDetail, requestSummary } from "@/test/requests";
import { stubFetch } from "@/test/stubFetch";
import { DiffScreen } from "./DiffScreen";
import type { DiffSearch } from "./search";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const LOGIN = comparedFile("src/routes/login.tsx", "modified");
const WORKING = comparison([LOGIN]);
// Today, so the pill shows the time without a date.
const READ_AT = new Date().setHours(14, 12, 31, 0);

const WORKSPACE = {
  id: "ws-7",
  task: "T-12",
  owner: "frontend/ui",
  kind: "task",
  path: "ws/t-12",
  base: "r7",
  state: "active",
  proposal: null,
  changed_files: 1,
  conflicted: 0,
  last_activity_at: READ_AT,
  created_at: READ_AT,
  closed_at: null,
  live: {
    workspace: "ws-7",
    task: "T-12",
    worker: "frontend/ui",
    base: "r7",
    files: [],
    summary: "no changes",
    last_activity_at: READ_AT,
    checkpoints: [
      { id: "ck-31", label: null, automatic: true, created_at: READ_AT },
    ],
    proposal: null,
    stalled: false,
  },
  attention: [],
};

function daemon() {
  return stubFetch({
    "GET /api/v1/projects/demo/diff": (request) => ({
      body: {
        view: "file",
        ...fileDiff(LOGIN, [hunk(1, 1, [ctx("a"), add("b")])]),
        path: request.search.get("path") ?? "",
      },
    }),
    "GET /api/v1/projects/demo/workspaces/ws-7": { body: WORKSPACE },
    "GET /api/v1/projects/demo/repository": {
      body: {
        head: "r8",
        main_path: "main",
        recent: [revision("r8", "r7"), revision("r7", null)],
      },
    },
  });
}

function revision(id: string, parent: string | null) {
  return {
    id,
    parent,
    created_at: READ_AT,
    tree: "t",
    origin: "proposal",
    task: null,
    proposal: null,
    title: null,
    author: null,
    pull: null,
  };
}

function show(
  search: DiffSearch,
  data: Comparison | null,
  readAt: number | null = READ_AT,
) {
  const root = createRootRoute({
    component: () => (
      <DiffScreen
        slug="demo"
        search={search}
        comparison={data}
        readAt={readAt}
        onSelectFile={() => undefined}
      />
    ),
  });
  const router = createRouter({
    routeTree: root,
    history: createMemoryHistory({ initialEntries: ["/"] }),
  });
  render(
    <QueryClientProvider client={new QueryClient()}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
}

describe("DiffScreen", () => {
  it("labels a workspace against its base Working changes and says it is not history", async () => {
    const calls = daemon();
    show({ from: "r7", to: "ws-7", path: "src/routes/login.tsx" }, WORKING);
    expect(
      await screen.findByRole("heading", { name: "Working changes in ws-7" }),
    ).toBeTruthy();
    expect(screen.getByText(/They are not history/)).toBeTruthy();
    expect(screen.getByText(/and again on each change/)).toBeTruthy();
    expect(screen.getByText("Working files")).toBeTruthy();
    expect(screen.getByText("base r7")).toBeTruthy();
    expect(
      await screen.findByRole("grid", { name: "Diff of src/routes/login.tsx" }),
    ).toBeTruthy();
    const read = calls.find((c) => c.path.endsWith("/diff"));
    expect(Object.fromEntries(read?.search ?? [])).toEqual({
      from: "r7",
      to: "ws-7",
      path: "src/routes/login.tsx",
    });
  });

  it("shows the live pill with the time the working files were read", async () => {
    daemon();
    show({ from: "r7", to: "ws-7" }, WORKING);
    expect(await screen.findByText("live · read 14:12:31")).toBeTruthy();
  });

  it("fills Compare with from the base, main and the checkpoints", async () => {
    daemon();
    show({ from: "r7", to: "ws-7" }, WORKING);
    const list = await screen.findByRole("navigation", {
      name: "Compare with",
    });
    expect(
      await within(list).findByRole("link", {
        name: "main r8 · 1 revision ahead",
      }),
    ).toBeTruthy();
    const links = within(list).getAllByRole("link");
    expect(links.map((l) => l.textContent)).toEqual([
      "base r7 (default)",
      "main r8 · 1 revision ahead",
      "checkpoint ck-31",
    ]);
    expect(links[0]?.getAttribute("aria-current")).toBe("page");
    expect(links[2]?.getAttribute("href")).toBe(
      "/p/demo/diff?from=ck-31&to=ws-7",
    );
  });

  it("names two revisions older to newer in the title and the crumb alone", async () => {
    daemon();
    show(
      { from: "r7", to: "r8" },
      comparison([LOGIN], {
        source: "recorded",
        from: state("revision", "r7"),
        to: state("revision", "r8"),
      }),
    );
    const title = await screen.findByRole("heading", {
      name: "Changes from r7 to r8",
    });
    expect(title).toBeTruthy();
    const crumbs = screen.getByRole("navigation", { name: "Breadcrumb" });
    expect(within(crumbs).getByText("r7 → r8")).toBeTruthy();
    expect(screen.queryByText("r7", { selector: "span" })).toBeNull();
    expect(screen.queryByText("r8", { selector: "span" })).toBeNull();
    expect(screen.queryByText(/They are not history/)).toBeNull();
    expect(screen.queryByText(/live · read/)).toBeNull();
  });

  it("lists an unstable file of a workspace without other changes", async () => {
    daemon();
    show(
      { from: "r7", to: "ws-7" },
      comparison([], { warnings: [{ path: "noisy.log", reason: "unstable" }] }),
    );
    expect(
      await screen.findByText(/kept changing while fold read it/),
    ).toBeTruthy();
    expect(screen.queryByRole("heading", { name: "No changes" })).toBeNull();
  });

  it("shows No changes for a comparison without files", async () => {
    daemon();
    show(
      { from: "r8", to: "r8" },
      comparison([], {
        source: "recorded",
        from: state("revision", "r8"),
        to: state("revision", "r8"),
      }),
    );
    expect(
      await screen.findByRole("heading", { name: "No changes" }),
    ).toBeTruthy();
  });

  it("shows Nothing to compare when there is no comparison", async () => {
    show({ from: "r1", to: "r2" }, null);
    expect(
      await screen.findByRole("heading", { name: "Nothing to compare" }),
    ).toBeTruthy();
    expect(
      screen.getByText("fold has no comparison of r1 with r2."),
    ).toBeTruthy();
  });

  it("names only to when there is no comparison for to alone", async () => {
    show({ to: "C-99" }, null);
    expect(
      await screen.findByText("fold has no comparison for C-99."),
    ).toBeTruthy();
  });

  it("makes a request about lines of a revision side, and offers none on a workspace side", async () => {
    const calls = stubFetch({
      "GET /api/v1/projects/demo/diff": () => ({
        body: {
          view: "file",
          ...fileDiff(LOGIN, [hunk(1, 1, [ctx("a"), add("b")])]),
          path: LOGIN.path,
        },
      }),
      "GET /api/v1/session": { body: { human: "tanim", authenticated: true } },
      "GET /api/v1/projects/demo/requests": {
        body: {
          items: [
            requestSummary("RQ-2", {
              code: {
                path: LOGIN.path,
                start_line: 1,
                end_line: 1,
                excerpt: "a",
                excerpt_cut: false,
                revision: "r7",
                proposal: null,
                version: null,
                side: null,
              },
            }),
          ],
        },
      },
      "POST /api/v1/projects/demo/requests/from-code": {
        status: 201,
        body: requestDetail(requestSummary("RQ-3")),
      },
    });
    const user = userEvent.setup();
    show(
      { from: "r7", to: "r8", path: LOGIN.path },
      comparison([LOGIN], {
        source: "recorded",
        from: state("revision", "r7"),
        to: state("revision", "r8"),
      }),
    );
    const grid = await screen.findByRole("grid", {
      name: `Diff of ${LOGIN.path}`,
    });
    // The request on r7 marks its line, which the old side of this comparison holds.
    expect(
      await within(grid).findByRole("link", {
        name: "Request RQ-2 about line 1, open",
      }),
    ).toBeTruthy();
    await user.click(
      within(grid).getByRole("gridcell", { name: /^Added line/ }),
    );
    await user.click(
      await within(grid).findByRole("button", { name: "Make a request" }),
    );
    const form = await within(grid).findByRole("form", {
      name: "Make a request about line 2 · r8",
    });
    await user.type(within(form).getByLabelText("Request"), "Why b?");
    await user.click(
      within(form).getByRole("button", { name: "Make the request" }),
    );
    expect(await within(grid).findByText(/with these lines/)).toBeTruthy();
    expect(
      calls.find((c) => c.path.endsWith("/requests/from-code"))?.body,
    ).toEqual({
      body: "Why b?",
      path: LOGIN.path,
      start_line: 2,
      end_line: 2,
      revision: "r8",
    });

    cleanup();
    show({ from: "r7", to: "ws-7", path: LOGIN.path }, WORKING);
    const working = await screen.findByRole("grid", {
      name: `Diff of ${LOGIN.path}`,
    });
    const rows = within(working).getAllByRole("row").length;
    await user.click(
      within(working).getByRole("gridcell", { name: /^Added line/ }),
    );
    expect(
      within(working).queryByRole("button", { name: "Make a request" }),
    ).toBeNull();
    expect(within(working).getAllByRole("row")).toHaveLength(rows);
  });

  it("takes the old side of a renamed file at its previous path, and puts focus back on the line", async () => {
    const renamed = comparedFile("src/routes/signin.tsx", "renamed", {
      previous_path: "src/routes/login.tsx",
    });
    const onOld = (id: string, path: string) =>
      requestSummary(id, {
        code: {
          path,
          start_line: 1,
          end_line: 1,
          excerpt: "a",
          excerpt_cut: false,
          revision: "r7",
          proposal: null,
          version: null,
          side: null,
        },
      });
    const calls = stubFetch({
      "GET /api/v1/projects/demo/diff": () => ({
        body: {
          view: "file",
          ...fileDiff(renamed, [hunk(1, 1, [del("a"), add("b")])]),
          path: renamed.path,
        },
      }),
      "GET /api/v1/session": { body: { human: "tanim", authenticated: true } },
      "GET /api/v1/projects/demo/requests": {
        body: {
          items: [
            onOld("RQ-2", "src/routes/login.tsx"),
            onOld("RQ-4", "src/routes/signin.tsx"),
          ],
        },
      },
      "POST /api/v1/projects/demo/requests/from-code": {
        status: 201,
        body: requestDetail(requestSummary("RQ-3")),
      },
    });
    const user = userEvent.setup();
    show(
      { from: "r7", to: "r8", path: renamed.path },
      comparison([renamed], {
        source: "recorded",
        from: state("revision", "r7"),
        to: state("revision", "r8"),
      }),
    );
    const grid = await screen.findByRole("grid", {
      name: `Diff of ${renamed.path}`,
    });
    expect(
      await within(grid).findByRole("link", {
        name: "Request RQ-2 about line 1, open",
      }),
    ).toBeTruthy();
    expect(
      within(grid).queryByRole("link", { name: /^Request RQ-4/ }),
    ).toBeNull();
    const removed = within(grid).getByRole("gridcell", {
      name: /^Removed line/,
    });
    await user.click(removed);
    await user.click(
      await within(grid).findByRole("button", { name: "Make a request" }),
    );
    const form = await within(grid).findByRole("form", {
      name: "Make a request about line 1 · r7",
    });
    await user.click(within(form).getByRole("button", { name: "Cancel" }));
    expect(document.activeElement).toBe(removed);
    await user.click(
      await within(grid).findByRole("button", { name: "Make a request" }),
    );
    await user.type(
      within(
        await within(grid).findByRole("form", {
          name: "Make a request about line 1 · r7",
        }),
      ).getByLabelText("Request"),
      "Why a?",
    );
    await user.click(
      within(grid).getByRole("button", { name: "Make the request" }),
    );
    expect(
      calls.find((c) => c.path.endsWith("/requests/from-code"))?.body,
    ).toEqual({
      body: "Why a?",
      path: "src/routes/login.tsx",
      start_line: 1,
      end_line: 1,
      revision: "r7",
    });
  });
});
