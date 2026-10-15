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
  cleanup,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it } from "vitest";
import { keys } from "@/api/keys";
import type { ThreadView } from "@/api/messages";
import { MessagesScreen } from "./MessagesScreen";
import type { MessagesSearch } from "./search";

afterEach(cleanup);

async function setup(
  search: MessagesSearch,
  threads?: ThreadView[],
): Promise<QueryClient> {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  if (threads) {
    queryClient.setQueryData(keys.messages("demo"), threads);
  }
  const root = createRootRoute();
  const index = createRoute({
    getParentRoute: () => root,
    path: "/",
    component: () => <MessagesScreen slug="demo" search={search} />,
  });
  const router = createRouter({
    routeTree: root.addChildren([index]),
    history: createMemoryHistory({ initialEntries: ["/"] }),
  });
  render(
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  await screen.findByRole("heading", { name: "Messages" });
  return queryClient;
}

describe("MessagesScreen", () => {
  it("shows the empty state for a project without messages", async () => {
    await setup({}, []);
    expect(
      screen.getByRole("region", { name: "No messages yet" }),
    ).toBeTruthy();
  });

  it("says no thread matches when the filters hide every thread", async () => {
    await setup({ task: "T-999" });
    expect(
      screen.getByRole("region", { name: "No thread matches" }),
    ).toBeTruthy();
  });

  it("shows a reply as pending, then in the thread once the fixture answers", async () => {
    await setup({});
    const panel = screen.getAllByRole("region")[0];
    if (!panel) {
      throw new Error("no thread panel");
    }
    const before = within(panel).getAllByRole("article").length;
    await userEvent.type(within(panel).getByLabelText("Reply"), "Plain SVG.");
    await userEvent.click(within(panel).getByRole("button", { name: "Send" }));
    expect(
      within(panel)
        .getByRole("button", { name: "Send" })
        .getAttribute("aria-busy"),
    ).toBe("true");
    expect(within(panel).getAllByRole("article")).toHaveLength(before);
    await waitFor(() => {
      expect(within(panel).getAllByRole("article")).toHaveLength(before + 1);
    });
    expect(within(panel).getByText("Plain SVG.")).toBeTruthy();
    expect(
      within(panel).getByLabelText<HTMLTextAreaElement>("Reply").value,
    ).toBe("");
  });
});
