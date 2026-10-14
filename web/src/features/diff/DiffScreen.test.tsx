// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  RouterProvider,
  createMemoryHistory,
  createRootRoute,
  createRouter,
} from "@tanstack/react-router";
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import type { Comparison } from "@/api/diff";
import { comparisons } from "../../../fixtures/diff";
import { DiffScreen } from "./DiffScreen";
import type { DiffSearch } from "./search";

afterEach(cleanup);

function show(search: DiffSearch, comparison: Comparison | null) {
  const root = createRootRoute({
    component: () => (
      <DiffScreen
        slug="demo"
        search={search}
        comparison={comparison}
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
    const entry = comparisons["r7..ws-7"];
    show(
      { from: "r7", to: "ws-7", path: "src/routes/login.tsx" },
      entry?.comparison ?? null,
    );
    expect(
      await screen.findByRole("heading", { name: "Working changes in ws-7" }),
    ).toBeTruthy();
    expect(screen.getByText(/They are not history/)).toBeTruthy();
    expect(screen.getByText("Working files")).toBeTruthy();
    expect(screen.getByText("base r7")).toBeTruthy();
    expect(
      await screen.findByRole("grid", { name: "Diff of src/routes/login.tsx" }),
    ).toBeTruthy();
  });

  it("shows the same working changes for to alone and reads the file without from", async () => {
    show(
      { to: "ws-7", path: "src/routes/login.tsx" },
      comparisons["ws-7"]?.comparison ?? null,
    );
    expect(
      await screen.findByRole("heading", { name: "Working changes in ws-7" }),
    ).toBeTruthy();
    expect(
      await screen.findByRole("grid", { name: "Diff of src/routes/login.tsx" }),
    ).toBeTruthy();
  });

  it("names two revisions and shows no working-changes note", async () => {
    show({ from: "r7", to: "r8" }, comparisons["r7..r8"]?.comparison ?? null);
    expect(
      await screen.findByRole("heading", { name: "Changes from r7 to r8" }),
    ).toBeTruthy();
    expect(screen.queryByText(/They are not history/)).toBeNull();
  });

  it("shows No changes for a comparison without files", async () => {
    show({ from: "r8", to: "r8" }, comparisons["r8..r8"]?.comparison ?? null);
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
});
