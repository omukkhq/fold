// @vitest-environment jsdom
import { QueryClientProvider } from "@tanstack/react-query";
import { RouterProvider, createMemoryHistory } from "@tanstack/react-router";
import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Suspense } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { keys } from "../api/keys";
import { createAppRouter } from "../app/router";
import { createQueryClient } from "../app/queryClient";
import { ThemeProvider } from "../theme/ThemeProvider";
import { ProjectsScreen } from "./projects/ProjectsScreen";

// The fixtures answer after a delay so that pending states show in development; these tests wait on content, so they answer at once.
vi.mock("../api/fixture", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../api/fixture")>();
  return {
    ...actual,
    fromFixture: <M, T>(
      load: (() => Promise<M>) | undefined,
      pick: (module: M) => T,
      empty: T,
    ) => actual.fromFixture(load, pick, empty, 0),
  };
});

beforeEach(() => {
  // jsdom has no scrolling; the router restores the scroll position on every navigation.
  vi.spyOn(window, "scrollTo").mockImplementation(() => undefined);
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
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
  it("lists the fixture projects with counts, agents and a link to the board", async () => {
    renderAt("/projects");
    const demo = await screen.findByRole("link", { name: /demo/ });
    expect(demo.getAttribute("href")).toBe("/p/demo/board");
    expect(within(demo).getByText(/17 tasks/)).toBeTruthy();
    expect(within(demo).getByText(/main r5/)).toBeTruthy();
    expect(within(demo).getByRole("img", { name: "frontend" })).toBeTruthy();
    expect(screen.getByText("Could not open this project.")).toBeTruthy();
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
        "fold init <dir>",
      ),
    ).toBeTruthy();
  });
});

describe("Repository screen", () => {
  it("lists the main revisions newest first with their source and task", async () => {
    renderAt("/p/demo/repository");
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
    expect(
      within(revisions).getByText(/P-5 · Form field components/),
    ).toBeTruthy();
  });

  it("expands a directory to show a symlink target", async () => {
    renderAt("/p/demo/repository");
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: "docs" }));
    expect(await screen.findByText("→ ../README.md")).toBeTruthy();
  });

  it("opens a text file", async () => {
    renderAt("/p/demo/repository");
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: /README\.md/ }));
    const text = await screen.findByRole("region", { name: "File README.md" });
    expect(await within(text).findByText(/A demo project/)).toBeTruthy();
  });

  it("opens a binary file with its size and hash", async () => {
    renderAt("/p/demo/repository");
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: "assets" }));
    await user.click(await screen.findByRole("button", { name: /logo\.png/ }));
    const binary = await screen.findByRole("region", {
      name: "File assets/logo.png",
    });
    expect(await within(binary).findByText(/Binary file/)).toBeTruthy();
    expect(within(binary).getByText(/18.0 KB · b3c1d2e4f5a60718/)).toBeTruthy();
  });

  it("compares two chosen revisions", async () => {
    const router = renderAt("/p/demo/repository");
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
    const router = renderAt("/p/demo/repository");
    const user = userEvent.setup();
    await screen.findByRole("list", { name: "Main revisions" });
    await user.selectOptions(screen.getByLabelText("From"), "r5");
    await user.click(screen.getByRole("button", { name: "Compare" }));
    expect(screen.getByText("Choose two different revisions.")).toBeTruthy();
    expect(screen.getByLabelText("To").getAttribute("aria-invalid")).toBe(
      "true",
    );
    expect(router.state.location.pathname).toBe("/p/demo/repository");
  });

  it("has nothing to compare with only r1", async () => {
    renderAt("/p/ledger/repository");
    await screen.findByRole("list", { name: "Main revisions" });
    expect(screen.queryByRole("button", { name: /^Compare/ })).toBeNull();
    expect(screen.getByText(/Only r1 exists/)).toBeTruthy();
  });

  it("opens the revision named in the address and ignores an unknown one", async () => {
    renderAt("/p/demo/repository?revision=r3");
    const list = await screen.findByRole("list", { name: "Main revisions" });
    const current = within(list)
      .getAllByRole("button")
      .find((b) => b.getAttribute("aria-current") === "true");
    expect(current?.textContent.startsWith("r3")).toBe(true);
    cleanup();
    renderAt("/p/demo/repository?revision=r99");
    const again = await screen.findByRole("list", { name: "Main revisions" });
    const newest = within(again)
      .getAllByRole("button")
      .find((b) => b.getAttribute("aria-current") === "true");
    expect(newest?.textContent.startsWith("r5")).toBe(true);
  });

  it("puts the chosen revision in the address and compares it with the one before", async () => {
    const router = renderAt("/p/demo/repository");
    const user = userEvent.setup();
    const list = await screen.findByRole("list", { name: "Main revisions" });
    const r3 = within(list)
      .getAllByRole("button")
      .find((b) => b.textContent.startsWith("r3"));
    if (!r3) {
      throw new Error("no r3 row");
    }
    await user.click(r3);
    expect(router.state.location.href).toBe("/p/demo/repository?revision=r3");
    expect(
      await screen.findByRole("button", { name: "Compare r2 → r3" }),
    ).toBeTruthy();
    const current = within(screen.getByRole("list", { name: "Main revisions" }))
      .getAllByRole("button")
      .find((b) => b.getAttribute("aria-current") === "true");
    expect(current?.textContent.startsWith("r3")).toBe(true);
  });
});
