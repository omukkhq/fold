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
import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { keys } from "@/api/keys";
import type { NeedsYouItem } from "@/api/workers";
import { stubFetch } from "@/test/stubFetch";
import { needsItem } from "@/test/workers";
import { NeedsYou } from "./NeedsYou";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

async function renderBadge(items: NeedsYouItem[], human: string | null) {
  stubFetch({
    "GET /api/v1/session": { body: { human, authenticated: true } },
    "GET /api/v1/projects/demo/needs-you": { body: { items } },
  });
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const root = createRootRoute({
    component: () => (
      <>
        <NeedsYou slug="demo" />
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
  const router = createRouter({
    routeTree: root.addChildren([index, agents]),
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

describe("NeedsYou", () => {
  it("hides the badge while nothing waits", async () => {
    const { client } = await renderBadge([], "tanim");
    await vi.waitFor(() => {
      expect(client.getQueryData(keys.needsYou("demo"))).toEqual([]);
    });
    expect(screen.queryByRole("button", { name: /Needs you/ })).toBeNull();
  });

  it("counts the items for this human and opens each item's screen", async () => {
    const { router } = await renderBadge(
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
    const badge = await screen.findByRole("button", { name: /Needs you/ });
    expect(badge.textContent).toBe("Needs you1");
    await userEvent.click(badge);
    const list = screen.getByRole("dialog", { name: "Needs you" });
    expect(within(list).getByText("1 waiting")).toBeTruthy();
    expect(within(list).getByText("Permissions")).toBeTruthy();
    expect(within(list).queryByText("Reviews")).toBeNull();
    await userEvent.click(within(list).getByRole("link"));
    expect(await screen.findByText("Agents screen")).toBeTruthy();
    expect(router.state.location.search).toEqual({ question: "pq-3" });
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("names the item chips of reviews and drafts", async () => {
    await renderBadge(
      [
        needsItem({ kind: "human_review", proposal: "P-4" }),
        needsItem({ kind: "request_draft", request: "RQ-5" }),
      ],
      null,
    );
    await userEvent.click(
      await screen.findByRole("button", { name: /Needs you/ }),
    );
    const list = screen.getByRole("dialog", { name: "Needs you" });
    expect(within(list).getByText("P-4")).toBeTruthy();
    expect(within(list).getByText("RQ-5")).toBeTruthy();
    expect(within(list).getByText("needs your review")).toBeTruthy();
  });

  it("shows an unstash conflict as a line that opens nothing and counts it", async () => {
    await renderBadge(
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
    const badge = await screen.findByRole("button", { name: /Needs you/ });
    expect(badge.textContent).toBe("Needs you2");
    await userEvent.click(badge);
    const list = screen.getByRole("dialog", { name: "Needs you" });
    const group = within(list).getByRole("region", {
      name: "Unstash conflict",
    });
    expect(
      within(group).getByText(
        "Unstash conflicts. Fix the files and run fld record",
      ),
    ).toBeTruthy();
    expect(
      within(group).getByText("src/forms/index.ts and 2 more"),
    ).toBeTruthy();
    expect(within(group).queryByRole("link")).toBeNull();
    expect(within(list).getAllByRole("link")).toHaveLength(1);
  });

  it("shows main's invalid checks file as a line that opens nothing and counts it", async () => {
    await renderBadge(
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
    const badge = await screen.findByRole("button", { name: /Needs you/ });
    expect(badge.textContent).toBe("Needs you2");
    await userEvent.click(badge);
    const list = screen.getByRole("dialog", { name: "Needs you" });
    expect(within(list).getByText("2 waiting")).toBeTruthy();
    const group = within(list).getByRole("region", { name: "Checks file" });
    expect(
      within(group).getByText(
        "Integrations wait until the checks file on main is fixed",
      ),
    ).toBeTruthy();
    expect(
      within(group).getByText(
        "fold-checks.toml line 12: unknown field `tiemout`",
      ),
    ).toBeTruthy();
    expect(within(group).queryByRole("link")).toBeNull();
  });
});
